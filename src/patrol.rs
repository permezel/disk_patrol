//#![allow(unused_imports)]
//#![allow(dead_code)]
//#![allow(unused_variables)]
//#![allow(unused_mut)]
//#![allow(deprecated)]

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use rand::{Rng, SeedableRng};
use rand::rngs::SmallRng;
use tokio::sync::Mutex;
use tokio::time::interval;

use crate::SECTOR_SIZE;
use crate::PatrolConfig;
use crate::device::DeviceInfo;
use crate::device::PatrolError;
use crate::SharedBuffer;
use crate::logger::Logger;

// Context struct to hold commonly passed parameters
struct PatrolContext {
    uniq: String,
    device_path: PathBuf,
    device_size: u64,
    read_position: u64,
}

pub struct PatrolReader {
    pub config: PatrolConfig,
    device_states: Arc<Mutex<HashMap<String, DeviceInfo>>>,
    logger: Arc<Logger>,
    pub shared_buffer: Arc<Mutex<SharedBuffer>>,  // Single shared buffer
}

impl PatrolReader {
    pub fn new(config: PatrolConfig) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let logger = Arc::new(Logger::new(config.clone())?);

        // Create the single shared buffer
        let buffer_size = config.read_size as usize;
        let alignment = 4 * 1024;
        let shared_buffer = Arc::new(Mutex::new(SharedBuffer::new(buffer_size, alignment)?));

        logger.log_info(&format!(
            "Created shared buffer: {} bytes ({}KB) with {} byte alignment",
            buffer_size,
            buffer_size / 1024,
            alignment
        ));

        Ok(Self {
            config,
            device_states: Arc::new(Mutex::new(HashMap::new())),
            logger,
            shared_buffer,
        })
    }

    pub async fn initialize(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut states = self.device_states.lock().await;

        // Load existing state if available
        if self.config.state_file.exists() {
            match self.load_state().await {
                Ok(loaded_states) => {
                    *states = loaded_states;
                    self.logger.log_info(&format!("Loaded patrol state from {:?}", self.config.state_file));
                }
                Err(e) => {
                    self.logger.log_warning(&format!("Could not load state file: {}", e));
                }
            }
        }

        let mut new_devices = Vec::new();

        // sync device state
        for device_path in &self.config.devices {
            match self.get_device_info(device_path).await {
                Ok(info) => {
                    if !states.contains_key(&info.uniq) {
                        new_devices.push(info.uniq.clone());
                        states.insert(info.uniq.clone(), info);
                    }
                },

                Err(e) => {
                    self.logger.log_warning(&format!("Cannot get info on {:?}: {}", device_path, e));
                    continue;
                }
            }
        }

        self.save_state(&states).await?;

        // verify that there are no aliases
        match crate::device::verify_unique(&states).await {
            Ok(_) => {},
            Err(e) => {
                dbg!(&e);
                eprintln!("Duplicates exist.  Please fix {} first.", self.config.state_file.display());
                return Err(e);
            }
        }

        if self.config.show_progress {
            for (id, info) in states.iter() {
                self.logger.create_progress_bar(id, &info).await;
                if new_devices.contains(&id) {
                    self.logger.log_info(&format!("Resuming device: {:?} {} ({}TB)",
                                                  info.path, info.uniq,
                                                  info.size_bytes / (1024 * 1024 * 1024 * 1024)));
                } else {
                    self.logger.log_info(&format!("Initialized device: {:?} {} ({}TB)",
                                                  info.path, info.uniq,
                                                  info.size_bytes / (1024 * 1024 * 1024 * 1024)));
                }
            }
        }
        Ok(())
    }

    async fn get_device_info(&self, device_path: &Path) -> Result<DeviceInfo, Box<dyn std::error::Error + Send + Sync>> {
        Ok(crate::get_device_info(device_path).await?)
    }

    async fn load_state(&self) -> Result<HashMap<String, DeviceInfo>, Box<dyn std::error::Error + Send + Sync>> {
        let content = tokio::fs::read_to_string(&self.config.state_file).await?;
        let states: HashMap<String, DeviceInfo> = serde_json::from_str(&content)?;
        Ok(states)
    }

    async fn save_state(&self, states: &HashMap<String, DeviceInfo>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let content = serde_json::to_string_pretty(states)?;
        tokio::fs::write(&self.config.state_file, content).await?;
        Ok(())
    }

    pub async fn print_status(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let states = self.device_states.lock().await;

        println!("\nDisk Patrol Status:");
        println!("{:-<80}", "");

        for (path, info) in states.iter() {
            let progress = (info.last_position as f64 / info.size_bytes as f64) * 100.0;
            let error_count = info.errors.len();

            println!("Device: {:?}", path);
            println!("  Size: {:.2} GB", info.size_bytes as f64 / (1024.0 * 1024.0 * 1024.0));
            println!("  Progress: {:.1}%", progress);
            println!("  Errors: {}", error_count);

            if error_count > 0 {
                println!("  Recent errors:");
                for error in info.errors.iter().rev().take(3) {
                    println!("    Sector {:x}: {}", error.sector, error.error);
                }
            }
            println!();
        }

        Ok(())
    }

    pub async fn run(&self, seek: u8) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Spawn individual patrol tasks for each device
        let mut handles = Vec::new();

        // Clone the necessary data from states before spawning tasks
        let device_info_for_spawn = {
            let states = self.device_states.lock().await;
            states.iter().map(|(uniq, info)| {
                let pos = info.size_bytes / self.config.read_size;
                let pos1 = seek as u64 * pos;
                let pos2 = pos1 / 100;
                let pos3 = pos2 * self.config.read_size;

                (
                    uniq.clone(),
                    info.path.clone(),
                    info.size_bytes,
                    info.last_position.max(pos3)
                )
            }).collect::<Vec<_>>()
        };

        for (uniq, device_path, size_bytes, last_position) in device_info_for_spawn {
            let device_states_clone = self.device_states.clone();
            let config_clone = self.config.clone();
            let logger_clone = self.logger.clone();
            let shared_buffer_clone = self.shared_buffer.clone();

            logger_clone.log_info(&format!("run {} size {:16x} seek {:16x}", device_path.display(), size_bytes, last_position));

            let handle = tokio::spawn(async move {
                let patrol_reader = Arc::new(PatrolReaderHandle {
                    device_states: device_states_clone,
                    config: config_clone,
                    logger: logger_clone,
                    shared_buffer: shared_buffer_clone,
                });

                patrol_reader.run_single(uniq, device_path, size_bytes, last_position).await
            });

            handles.push(handle);
        }

        // Spawn periodic maintenance tasks
        let _state_saver_handle = self.spawn_periodic_tasks().await;

        // Wait for all device patrols (they run forever)
        for handle in handles {
            if let Err(e) = handle.await {
                self.logger.log_error(&format!("Device patrol task failed: {}", e));
            }
        }

        Ok(())
    }

    /// Spawn periodic maintenance tasks (state saving, error checking)
    async fn spawn_periodic_tasks(&self) -> tokio::task::JoinHandle<()> {
        let device_states = self.device_states.clone();
        let config = self.config.clone();
        let logger = self.logger.clone();

        tokio::spawn(async move {
            let mut save_interval = interval(Duration::from_secs(60)); // Save state every minute
            let mut alert_interval = interval(Duration::from_secs(300)); // Check alerts every 5 minutes

            loop {
                tokio::select! {
                    _ = save_interval.tick() => {
                        // Periodic state save
                        let states = device_states.lock().await;
                        if let Err(e) = Self::save_state_to_file(&config.state_file, &states).await {
                            logger.log_error(&format!("Failed to save state: {}", e));
                        }
                    }

                    _ = alert_interval.tick() => {
                        // Periodic error threshold checking
                        if let Err(e) = Self::check_error_alerts(&device_states, &config, &logger).await {
                            logger.log_error(&format!("Error checking alerts: {}", e));
                        }
                    }
                }
            }
        })
    }

    /// Static version of check_error_alerts for use in spawned tasks
    async fn check_error_alerts(
        device_states: &Arc<Mutex<HashMap<String, DeviceInfo>>>,
        config: &PatrolConfig,
        logger: &Arc<Logger>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut states = device_states.lock().await;

        for (device_path, device_info) in states.iter_mut() {
            if device_info.errors.len() - device_info.reported_errors >= config.error_threshold {
                device_info.reported_errors = device_info.errors.len();

                let subject = format!("Disk Patrol Alert: {} errors on {:?}",
                                    device_info.errors.len(), device_path);

                let mut body = format!("Device: {:?}\n", device_path);
                body.push_str(&format!("Total errors: {}\n\n", device_info.errors.len()));
                body.push_str("Recent errors:\n");

                for error in device_info.errors.iter().rev().take(10) {
                    let timestamp = SystemTime::UNIX_EPOCH + Duration::from_secs(error.timestamp);
                    body.push_str(&format!("  {:?} - Sector {:x}: {}\n",
                                           timestamp, error.sector, error.error));
                }

                logger.send_email_alert(&subject, &body).await?;
            }
        }

        Ok(())
    }

    /// Helper method for saving state (static version)
    async fn save_state_to_file(
        state_file: &PathBuf,
        states: &HashMap<String, DeviceInfo>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let content = serde_json::to_string_pretty(states)?;
        tokio::fs::write(state_file, content).await?;
        Ok(())
    }

    /// Reset device state (for --reset option)
    pub async fn reset_device_state(&self, reset_all: bool, specific_devices: &[PathBuf]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut states = self.device_states.lock().await;

        if reset_all {
            self.logger.log_info("Resetting all device states");
            states.clear();
        } else {
            for device_path in specific_devices {
                match self.get_device_info(device_path).await {
                    Ok(info) => {
                        if states.remove(&info.uniq).is_some() {
                            self.logger.log_info(&format!("Reset state for device: {:?}-{}", device_path, info.uniq));
                        } else {
                            self.logger.log_warning(&format!("Device not found in state: {:?}", device_path));
                        }
                    },
                    Err(e) => {
                        self.logger.log_error(&format!("Error {:?}: {}", device_path, e));
                    }
                }
            }
        }

        // Re-initialize devices
        for device_path in &self.config.devices {
            match self.get_device_info(device_path).await {
                Ok(info) => {
                    if !states.contains_key(&info.uniq) {
                        self.logger.log_info(&format!("Re-initialized device: {:?}-{} ({}TB)",
                                                          device_path, info.uniq, info.size_bytes / (1024 * 1024 * 1024 * 1024)));
                        states.insert(info.uniq.clone(), info);
                    }
                },
                Err(e) => {
                    self.logger.log_error(&format!("Error re-initializing device {:?}: {}", device_path, e));
                }
            }
        }

        self.save_state(&states).await?;
        self.logger.log_info("Device state reset complete");
        Ok(())
    }

    pub async fn send_email_alert(&self, subject: &str, body: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.config.email_alerts {
            return Ok(());
        }
        self.logger.send_email_alert(subject, body).await
    }
}

// Separate struct to handle patrol operations with reduced parameter passing
struct PatrolReaderHandle {
    device_states: Arc<Mutex<HashMap<String, DeviceInfo>>>,
    config: PatrolConfig,
    logger: Arc<Logger>,
    shared_buffer: Arc<Mutex<SharedBuffer>>,
}

impl PatrolReaderHandle {
    /// Enhanced single device patrol using shared buffer with jitter
    async fn run_single(
        &self,
        uniq: String,
        device_path: PathBuf,
        device_size: u64,
        mut read_position: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {

        // calculate optimal interval
        let patrol_period_ms = 1000 * (self.config.patrol_period_days * 24 * 3600);
        let total_reads = (device_size + self.config.read_size - 1) / self.config.read_size;
        let optimal_sleep_ms = (patrol_period_ms / total_reads.max(1)).max(1);
        let jitter_info = if self.config.enable_jitter {
            format!(" (±{}% jitter)", self.config.max_jitter_percent)
        } else {
            String::new()
        };

        self.logger.log_info(&format!(
            "Device {:?}-{}: {} reads over {}d = {}ms intervals{}",
            device_path, uniq,
            total_reads,
            self.config.patrol_period_days,
            optimal_sleep_ms,
            jitter_info
        ));

        // Create a Send-safe RNG seeded from entropy
        let mut rng = SmallRng::from_entropy();

        loop {
            let start_time = tokio::time::Instant::now();

            // Calculate jittered timing for this iteration
            let (pre_sleep_ms, post_sleep_ms) = if self.config.enable_jitter {
                calculate_jittered_timing(optimal_sleep_ms, self.config.max_jitter_percent, &mut rng)
            } else {
                (0, optimal_sleep_ms)
            };

            // Random pre-sleep (jitter the start time)
            if pre_sleep_ms > 0 {
                tokio::time::sleep(Duration::from_millis(pre_sleep_ms)).await;
            }

            // Perform the I/O operation
            let io_start = tokio::time::Instant::now();
            let ctx = PatrolContext {
                uniq: uniq.clone(),
                device_path: device_path.clone(),
                device_size,
                read_position,
            };

            if let Err(e) = self.patrol_device(&ctx).await {
                self.logger.log_error(&format!("Error patrolling {:?}: {}", device_path, e));
            }
            read_position += self.config.read_size;
            let io_duration = io_start.elapsed();

            // Calculate remaining sleep time, accounting for actual I/O duration
            let total_elapsed = start_time.elapsed().as_millis() as u64;
            let target_total_time = if self.config.enable_jitter {
                pre_sleep_ms + post_sleep_ms
            } else {
                optimal_sleep_ms
            };

            if total_elapsed < target_total_time {
                let remaining_sleep = target_total_time - total_elapsed;
                tokio::time::sleep(Duration::from_millis(remaining_sleep)).await;
            }

            if self.config.verbose && self.config.enable_jitter {
                self.logger.log_info(&format!(
                    "Device {:?}: pre={}ms, I/O={}ms, post={}ms, total={}ms",
                    device_path.file_name().unwrap_or_default().to_string_lossy(),
                    pre_sleep_ms,
                    io_duration.as_millis(),
                    if total_elapsed < target_total_time { target_total_time - total_elapsed } else { 0 },
                    start_time.elapsed().as_millis()
                ));
            }
        }
    }

    async fn patrol_device(
        &self,
        ctx: &PatrolContext,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {

        // Perform the read with the shared buffer
        match self.read_device_chunk(&ctx.device_path, ctx.read_position).await {
            Ok(_) => {
                // Successful read - update state and progress
                self.update_device_state_after_read(ctx, ctx.read_position + self.config.read_size).await?;

                if self.config.verbose {
                    let sector_start = ctx.read_position / SECTOR_SIZE;
                    let sector_end = (ctx.read_position + self.config.read_size - 1) / SECTOR_SIZE;
                    self.logger.log_info(&format!("✓ Read sectors {:x}-{:x} from {:?}-{}",
                                           sector_start, sector_end, ctx.device_path, ctx.uniq));
                }
            }
            Err(e) => {
                // Handle read error
                self.handle_read_error(ctx, e).await?;
            }
        }

        Ok(())
    }

    /// Update device state after successful read
    async fn update_device_state_after_read(
        &self,
        ctx: &PatrolContext,
        new_position: u64,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut states = self.device_states.lock().await;
        if let Some(device_info) = states.get_mut(&ctx.uniq) {
            device_info.last_position = new_position;

            // Update progress bar
            if self.config.show_progress {
                let progress_bars = self.logger.progress_bars.lock().await;
                if let Some(pb) = progress_bars.get(&ctx.uniq) {
                    let progress_pct = (new_position as f64 / ctx.device_size as f64) * 100.0;
                    if self.config.debug {
                        self.logger.progress_msg(&pb, &ctx.uniq, device_info,
                                            format!("{:4.1}% {:16x} {} {}",
                                                    progress_pct, new_position / SECTOR_SIZE,
                                                    ctx.device_path.display(), ctx.uniq));
                    } else {
                        self.logger.progress_msg(&pb, &ctx.uniq, device_info, format!("{:4.1}% {:16x}",
                                                                                   progress_pct, new_position));
                    }
                }
            }

            // Check if patrol cycle is complete
            if device_info.last_position >= ctx.device_size {
                device_info.last_position = 0;
                device_info.patrol_start = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

                self.logger.log_info(&format!("✅ Completed full patrol of {:?}, restarting cycle", ctx.device_path));

                // Reset progress bar
                if self.config.show_progress {
                    let progress_bars = self.logger.progress_bars.lock().await;
                    if let Some(pb) = progress_bars.get(&ctx.uniq) {
                        pb.set_position(0);
                        self.logger.progress_msg(&pb, &ctx.uniq, device_info, format!("Cycle complete! Restarting..."));
                    }
                }
            }
        } else {
            panic!("missing device info for {:?}-{}", ctx.device_path, ctx.uniq);
        }
        Ok(())
    }

    /// Handle read errors
    async fn handle_read_error(
        &self,
        ctx: &PatrolContext,
        error: std::io::Error,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let error_msg = format!("I/O Error reading {:?} at sector {:x}: {}",
                               ctx.device_path, ctx.read_position / SECTOR_SIZE, error);
        self.logger.log_error(&error_msg);

        // Record error in device state
        let mut states = self.device_states.lock().await;
        if let Some(device_info) = states.get_mut(&ctx.uniq) {
            device_info.errors.push(PatrolError {
                timestamp: now,
                sector: ctx.read_position / SECTOR_SIZE,
                error: error.to_string(),
            });

            // Update progress bar with error indicator
            if self.config.show_progress {
                let progress_bars = self.logger.progress_bars.lock().await;
                if let Some(pb) = progress_bars.get(&ctx.uniq) {
                    self.logger.progress_msg(&pb, &ctx.uniq, device_info,
                                      format!("❌ {} errors total", device_info.errors.len()));
                }
            }

            // Still advance position to avoid getting stuck
            device_info.last_position += self.config.read_size;
        }
        Ok(())
    }

    /// Read device chunk using the single shared buffer
    async fn read_device_chunk(
        &self,
        device_path: &Path,
        position: u64,
    ) -> Result<(), std::io::Error> {
        let device_path = device_path.to_path_buf();
        let read_size = self.config.read_size;
        let shared_buffer = self.shared_buffer.clone();

        tokio::task::spawn_blocking(move || {
            // Acquire the shared buffer lock - this is where serialization happens
            let rt = tokio::runtime::Handle::current();
            let mut buffer = rt.block_on(shared_buffer.lock());

            if buffer.len() < read_size as usize {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("Buffer size {} too small for read size {}", buffer.len(), read_size)
                ));
            }

            // Open device with O_DIRECT on Linux, normally on other platforms
            #[cfg(target_os = "linux")]
            use std::os::unix::fs::OpenOptionsExt;
            #[cfg(target_os = "linux")]
            let mut file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECT)
                .open(&device_path)
                .map_err(|e| {
                    std::io::Error::new(
                        e.kind(),
                        format!("Failed to open device '{}': {}", device_path.display(), e)
                    )
                })?;

            #[cfg(not(target_os = "linux"))]
            let mut file = {
                let file = OpenOptions::new()
                    .read(true)
                    .open(&device_path)
                    .map_err(|e| {
                        std::io::Error::new(
                            e.kind(),
                            format!("Failed to open device '{}': {}", device_path.display(), e)
                        )
                    })?;

                // On macOS, attempt to disable caching
                #[cfg(target_os = "macos")]
                {
                    use std::os::unix::io::AsRawFd;
                    use libc::{fcntl, F_NOCACHE, F_SETFL};

                    let fd = file.as_raw_fd();
                    unsafe {
                        fcntl(fd, F_SETFL, F_NOCACHE);
                    }
                }

                file
            };

            // Seek to position
            file.seek(SeekFrom::Start(position))
                .map_err(|e| {
                    std::io::Error::new(
                        e.kind(),
                        format!("Failed to seek to position {}: {}", position, e)
                    )
                })?;

            // Read into the shared buffer
            let buffer_slice = buffer.as_mut_slice();
            let bytes_read = file.read(&mut buffer_slice[..read_size as usize])
                .map_err(|e| {
                    std::io::Error::new(
                        e.kind(),
                        format!("Failed to read {} bytes at position {}: {}", read_size, position, e)
                    )
                })?;

            // Verify we read the expected amount
            if bytes_read < read_size as usize {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!("Expected to read {} bytes, but only read {}", read_size, bytes_read)
                ));
            }

            // Note: We don't need to copy data out since we're just checking for read errors
            // The buffer content is discarded after this operation
            Ok(())
        }).await.unwrap()
    }
}

/// Calculate jittered timing: split the interval into random pre-sleep + post-sleep
fn calculate_jittered_timing(
    base_interval_ms: u64,
    max_jitter_percent: u8,
    rng: &mut impl Rng
) -> (u64, u64) {
    let max_jitter_percent = max_jitter_percent.min(100) as f64 / 100.0;
    let max_jitter_ms = (base_interval_ms as f64 * max_jitter_percent) as u64;

    // Random jitter from 0 to max_jitter_ms
    let total_jitter = rng.gen_range(0..=max_jitter_ms);

    // Split the jitter randomly between pre and post
    let pre_jitter = rng.gen_range(0..=total_jitter);
    let post_jitter = total_jitter - pre_jitter;

    // Base timing: start with some delay, then I/O, then remaining sleep
    let base_post_sleep = base_interval_ms.saturating_sub(total_jitter);

    (pre_jitter, base_post_sleep + post_jitter)
}
