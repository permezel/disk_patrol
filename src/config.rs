/*
#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(deprecated)]
*/
use clap::{Arg, ArgMatches, Command};
use clap::parser::ValueSource;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::Path;

pub const SECTOR_SIZE: u64 = 512;

// Default values as constants
pub const DEFAULT_CONFIG_PATH: &str = "/etc/disk_patrol/config.toml";
pub const DEFAULT_PATROL_DAYS: u64 = 30;
pub const DEFAULT_READ_SIZE: u64 = 8 * 1024 * 1024; // 8MB
pub const DEFAULT_STATE_FILE: &str = "/var/lib/disk_patrol/state.json";
pub const DEFAULT_LOG_FILE: &str = "/var/log/disk_patrol.log";
pub const DEFAULT_VERBOSE: bool = false;
pub const DEFAULT_DEBUG: bool = false;
pub const DEFAULT_USE_SYSLOG: bool = true;
pub const DEFAULT_SYSLOG_FACILITY: &str = "daemon";
pub const DEFAULT_EMAIL_ALERTS: bool = true;
pub const DEFAULT_SMTP_SERVER: &str = "smtp.example.com";
pub const DEFAULT_SMTP_PORT: u16 = 587;
pub const DEFAULT_SMTP_USE_STARTTLS: bool = true;
pub const DEFAULT_SMTP_USERNAME: &str = "disk_patrol@example.com";
pub const DEFAULT_SMTP_PASSWORD: &str = "your_password_here";
pub const DEFAULT_ALERT_FROM: &str = "disk_patrol@example.com";
pub const DEFAULT_ERROR_THRESHOLD: usize = 5;
pub const DEFAULT_SHOW_PROGRESS: bool = true;
pub const DEFAULT_MIN_SLEEP_SECONDS: u64 = 5;
pub const DEFAULT_MAX_SLEEP_SECONDS: u64 = 300;
pub const DEFAULT_ENABLE_JITTER: bool = true;
pub const DEFAULT_MAX_JITTER_PERCENT: u8 = 25;

// String representations for numeric defaults (for clap)
const DEFAULT_PATROL_DAYS_STR: &str = "30";
const DEFAULT_READ_SIZE_STR: &str = "8MB";
const DEFAULT_SMTP_PORT_STR: &str = "587";
const DEFAULT_ERROR_THRESHOLD_STR: &str = "5";
const DEFAULT_MIN_SLEEP_SECONDS_STR: &str = "5";
const DEFAULT_MAX_SLEEP_SECONDS_STR: &str = "300";
const DEFAULT_MAX_JITTER_PERCENT_STR: &str = "25";

pub struct Config;

impl Config {
    pub fn from_args() -> Result<PatrolConfig, Box<dyn std::error::Error + Send + Sync>> {
        let matches = build_cli().get_matches();

        let mut builder = ConfigBuilder::new();

        if let Some(config_path) = matches.get_one::<String>("config") {
            builder = builder.load_config_file(config_path)?;
        }

        builder.merge_command_line(&matches).build()
    }

    pub fn generate_example(path: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let content = generate_example_config()?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

/// File access modes for verification
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileAccessMode {
    /// File must exist and be readable
    Read,
    /// File must be writable (will be created if it doesn't exist)
    Write,
    /// File must be both readable and writable
    ReadWrite,
}

/// Verify file accessibility
pub fn verify_file_access(path: &Path, mode: FileAccessMode) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match mode {
        FileAccessMode::Read => {
            // Check if file exists and is readable
            match File::open(path) {
                Ok(_) => Ok(()),
                Err(e) => {
                    let msg = match e.kind() {
                        ErrorKind::NotFound => format!("File not found: {}", path.display()),
                        ErrorKind::PermissionDenied => format!("Permission denied reading file: {}", path.display()),
                        _ => format!("Cannot read file {}: {}", path.display(), e),
                    };
                    Err(msg.into())
                }
            }
        }
        FileAccessMode::Write => {
            // Check if we can write to the file (create if it doesn't exist)
            // First check if parent directory exists and is writable
            if let Some(parent) = path.parent() {
                if !parent.exists() {
                    return Err(format!("Parent directory does not exist: {}", parent.display()).into());
                }

                // Check if parent directory is writable by trying to create a temp file
                let temp_name = format!(".{}.tmp", path.file_name().unwrap_or_default().to_string_lossy());
                let temp_path = parent.join(&temp_name);

                match File::create(&temp_path) {
                    Ok(_) => {
                        // Clean up temp file
                        let _ = std::fs::remove_file(&temp_path);
                    }
                    Err(e) => {
                        return Err(format!("Cannot write to directory {}: {}", parent.display(), e).into());
                    }
                }
            }

            // Now check the actual file
            match OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
            {
                Ok(_) => Ok(()),
                Err(e) => {
                    let msg = match e.kind() {
                        ErrorKind::PermissionDenied => format!("Permission denied writing to file: {}", path.display()),
                        _ => format!("Cannot write to file {}: {}", path.display(), e),
                    };
                    Err(msg.into())
                }
            }
        }
        FileAccessMode::ReadWrite => {
            // For read-write, file must exist and be both readable and writable
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create(false)  // Don't create if it doesn't exist
                .open(path)
            {
                Ok(_) => Ok(()),
                Err(e) => {
                    let msg = match e.kind() {
                        ErrorKind::NotFound => format!("File not found: {}", path.display()),
                        ErrorKind::PermissionDenied => format!("Permission denied accessing file: {}", path.display()),
                        _ => format!("Cannot access file {}: {}", path.display(), e),
                    };
                    Err(msg.into())
                }
            }
        }
    }
}

/// Verify configuration paths are accessible
pub fn verify_config_paths(config: &PatrolConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Verify state file is writable (will be created if it doesn't exist)
    verify_file_access(&config.state_file, FileAccessMode::Write)
        .map_err(|e| format!("State file error: {}", e))?;

    // Verify log file is writable if specified
    if let Some(log_file) = &config.log_file {
        verify_file_access(log_file, FileAccessMode::Write)
            .map_err(|e| format!("Log file error: {}", e))?;
    }

    // Verify all devices are readable block devices
    for device in &config.devices {
        // First check basic file access
        verify_file_access(device, FileAccessMode::Read)
            .map_err(|e| format!("Device error: {}", e))?;

        // Additional check: verify it's a block device
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;
            let metadata = std::fs::metadata(device)
                .map_err(|e| format!("Cannot read device metadata for {}: {}", device.display(), e))?;

            if !metadata.file_type().is_block_device() {
                return Err(format!("{} is not a block device", device.display()).into());
            }
        }
    }

    Ok(())
}


/// Macro to check and override config values from command line
macro_rules! override_if_present {
    // For string values
    ($matches:expr, $option:expr, $target:expr, string) => {
        if $matches.value_source($option) == Some(ValueSource::CommandLine) {
            if let Some(value) = $matches.get_one::<String>($option) {
                $target = Some(value.to_string());
            }
        }
    };

    // For boolean flags
    ($matches:expr, $option:expr, $target:expr, flag) => {
        if $matches.value_source($option) == Some(ValueSource::CommandLine) {
            $target = Some($matches.get_flag($option));
        }
    };

    // For boolean flags with 'no-' version
    ($matches:expr, $option:expr, $target:expr, flag-yn) => {
        if $matches.value_source($option) == Some(ValueSource::CommandLine) {
            $target = Some(true);
        } else if $matches.value_source(concat!("no-", $option)) == Some(ValueSource::CommandLine) {
            $target = Some(false);
        }
    };

    // For Vec<String>
    ($matches:expr, $option:expr, $target:expr, vec_string) => {
        if $matches.value_source($option) == Some(ValueSource::CommandLine) {
            if let Some(values) = $matches.get_many::<String>($option) {
                $target = Some(values.map(|s| s.to_string()).collect());
            }
        }
    };

    // For parsed values
    ($matches:expr, $option:expr, $target:expr, $type:ty) => {
        if $matches.value_source($option) == Some(ValueSource::CommandLine) {
            if let Some(value) = $matches.get_one::<String>($option) {
                if let Ok(parsed) = value.parse::<$type>() {
                    $target = Some(parsed);
                }
            }
        }
    };
}

/// Configuration file structure for TOML parsing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    pub devices: Option<Vec<String>>,
    pub patrol_period_days: Option<u64>,
    pub read_size: Option<String>,
    pub state_file: Option<String>,
    pub log_file: Option<String>,
    pub verbose: Option<bool>,
    pub debug: Option<bool>,
    pub use_syslog: Option<bool>,
    pub syslog_facility: Option<String>,
    pub email_alerts: Option<bool>,
    pub smtp_server: Option<String>,
    pub smtp_port: Option<u16>,
    pub smtp_use_starttls: Option<bool>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub alert_from: Option<String>,
    pub alert_to: Option<Vec<String>>,
    pub error_threshold: Option<usize>,
    pub show_progress: Option<bool>,
    pub min_sleep_seconds: Option<u64>,
    pub max_sleep_seconds: Option<u64>,
    pub enable_jitter: Option<bool>,
    pub max_jitter_percent: Option<u8>,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            devices: Some(vec!["/dev/sda".to_string(), "/dev/nvme0n1".to_string()]),
            patrol_period_days: Some(DEFAULT_PATROL_DAYS),
            read_size: Some(DEFAULT_READ_SIZE_STR.to_string()),
            state_file: Some(DEFAULT_STATE_FILE.to_string()),
            log_file: Some(DEFAULT_LOG_FILE.to_string()),
            verbose: Some(DEFAULT_VERBOSE),
            debug: Some(DEFAULT_DEBUG),
            use_syslog: Some(DEFAULT_USE_SYSLOG),
            syslog_facility: Some(DEFAULT_SYSLOG_FACILITY.to_string()),
            email_alerts: Some(DEFAULT_EMAIL_ALERTS),
            smtp_server: Some(DEFAULT_SMTP_SERVER.to_string()),
            smtp_port: Some(DEFAULT_SMTP_PORT),
            smtp_use_starttls: Some(DEFAULT_SMTP_USE_STARTTLS),
            smtp_username: Some(DEFAULT_SMTP_USERNAME.to_string()),
            smtp_password: Some(DEFAULT_SMTP_PASSWORD.to_string()),
            alert_from: Some(DEFAULT_ALERT_FROM.to_string()),
            alert_to: Some(vec!["admin@example.com".to_string(), "ops@example.com".to_string()]),
            error_threshold: Some(DEFAULT_ERROR_THRESHOLD),
            show_progress: Some(DEFAULT_SHOW_PROGRESS),
            min_sleep_seconds: Some(DEFAULT_MIN_SLEEP_SECONDS),
            max_sleep_seconds: Some(DEFAULT_MAX_SLEEP_SECONDS),
            enable_jitter: Some(DEFAULT_ENABLE_JITTER),
            max_jitter_percent: Some(DEFAULT_MAX_JITTER_PERCENT),
        }
    }
}

/// Runtime configuration for the patrol reader
#[derive(Debug, Clone)]
pub struct PatrolConfig {
    pub devices: Vec<PathBuf>,
    pub patrol_period_days: u64,
    pub read_size: u64,
    pub state_file: PathBuf,
    pub log_file: Option<PathBuf>,
    pub verbose: bool,
    pub debug: bool,
    pub use_syslog: bool,
    pub syslog_facility: String,
    pub email_alerts: bool,
    pub smtp_server: Option<String>,
    pub smtp_port: u16,
    pub smtp_use_starttls: bool,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub alert_from: Option<String>,
    pub alert_to: Vec<String>,
    pub error_threshold: usize,
    pub show_progress: bool,
    pub min_sleep_seconds: Option<u64>,
    pub max_sleep_seconds: Option<u64>,
    pub enable_jitter: bool,
    pub max_jitter_percent: u8,
}

/// Builder for merging configuration from file and command line
pub struct ConfigBuilder {
    config_file: Option<ConfigFile>,
}

impl ConfigBuilder {
    pub fn new() -> Self {
        Self {
            config_file: None,
        }
    }

    pub fn load_config_file(mut self, path: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let content = std::fs::read_to_string(path)?;
        let config: ConfigFile = toml::from_str(&content)?;
        self.config_file = Some(config);
        Ok(self)
    }

    pub fn save_config_file(self, path: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut output = String::from("# Disk Patrol Configuration File\n");
        output.push_str("# \n");
        output.push_str(&toml::to_string_pretty(&self.config_file)?);
        fs::write(path, output)?;
        Ok(())
    }

    pub fn merge_command_line(mut self, matches: &ArgMatches) -> Self {
        if let Some(file_config) = &mut self.config_file {
            // Override config file values with command line arguments if provided

            // String overrides
            override_if_present!(matches, "read-size", file_config.read_size, string);
            override_if_present!(matches, "state-file", file_config.state_file, string);
            override_if_present!(matches, "syslog-facility", file_config.syslog_facility, string);
            override_if_present!(matches, "smtp-server", file_config.smtp_server, string);
            override_if_present!(matches, "smtp-username", file_config.smtp_username, string);
            override_if_present!(matches, "smtp-password", file_config.smtp_password, string);
            override_if_present!(matches, "alert-from", file_config.alert_from, string);

            // Numeric overrides
            override_if_present!(matches, "period", file_config.patrol_period_days, u64);
            override_if_present!(matches, "smtp-port", file_config.smtp_port, u16);
            override_if_present!(matches, "error-threshold", file_config.error_threshold, usize);
            override_if_present!(matches, "min-sleep", file_config.min_sleep_seconds, u64);
            override_if_present!(matches, "max-sleep", file_config.max_sleep_seconds, u64);
            override_if_present!(matches, "max-jitter", file_config.max_jitter_percent, u8);

            // Boolean flags
            override_if_present!(matches, "verbose", file_config.verbose, flag-yn);
            override_if_present!(matches, "debug", file_config.debug, flag-yn);
            override_if_present!(matches, "syslog", file_config.use_syslog, flag-yn);
            override_if_present!(matches, "email-alerts", file_config.email_alerts, flag-yn);
            override_if_present!(matches, "smtp-starttls", file_config.smtp_use_starttls, flag);
            override_if_present!(matches, "progress", file_config.show_progress, flag-yn);
            override_if_present!(matches, "enable-jitter", file_config.enable_jitter, flag);

            // Vector overrides
            override_if_present!(matches, "devices", file_config.devices, vec_string);
            override_if_present!(matches, "alert-to", file_config.alert_to, vec_string);
        } else {
            // No config file, create one from command line args
            let mut file_config = ConfigFile::default();

            if let Some(devices) = matches.get_many::<String>("devices") {
                file_config.devices = Some(devices.map(|s| s.to_string()).collect());
            }

            if let Some(period) = matches.get_one::<String>("period") {
                file_config.patrol_period_days = Some(period.parse().unwrap_or(DEFAULT_PATROL_DAYS));
            }

            if let Some(size) = matches.get_one::<String>("read-size") {
                file_config.read_size = Some(size.parse().unwrap_or(DEFAULT_READ_SIZE.to_string()));
            }

            if let Some(path) = matches.get_one::<String>("state-file") {
                file_config.state_file = Some(path.to_string());
            }

            if matches.get_flag("verbose") {
                file_config.verbose = Some(true);
            }

            if matches.get_flag("debug") {
                file_config.debug = Some(true);
            }

            if matches.get_flag("syslog") {
                file_config.use_syslog = Some(true);
            }

            if let Some(facility) = matches.get_one::<String>("syslog-facility") {
                file_config.syslog_facility = Some(facility.to_string());
            }

            if matches.get_flag("email-alerts") {
                file_config.email_alerts = Some(true);
            }

            if let Some(server) = matches.get_one::<String>("smtp-server") {
                file_config.smtp_server = Some(server.to_string());
            }

            if let Some(port) = matches.get_one::<String>("smtp-port") {
                file_config.smtp_port = Some(port.parse().unwrap_or(25));
            }

            if matches.get_flag("smtp-starttls") {
                file_config.smtp_use_starttls = Some(true);
            }

            if let Some(username) = matches.get_one::<String>("smtp-username") {
                file_config.smtp_username = Some(username.to_string());
            }

            if let Some(password) = matches.get_one::<String>("smtp-password") {
                file_config.smtp_password = Some(password.to_string());
            }

            if let Some(from) = matches.get_one::<String>("alert-from") {
                file_config.alert_from = Some(from.to_string());
            }

            if let Some(recipients) = matches.get_many::<String>("alert-to") {
                file_config.alert_to = Some(recipients.map(|s| s.to_string()).collect());
            }

            if let Some(threshold) = matches.get_one::<String>("error-threshold") {
                file_config.error_threshold = Some(threshold.parse().unwrap_or(5));
            }

            if matches.get_flag("progress") {
                file_config.show_progress = Some(true);
            }

            if let Some(min) = matches.get_one::<String>("min-sleep") {
                file_config.min_sleep_seconds = Some(min.parse().unwrap_or(5));
            }

            if let Some(max) = matches.get_one::<String>("max-sleep") {
                file_config.max_sleep_seconds = Some(max.parse().unwrap_or(300));
            }

            if matches.get_flag("enable-jitter") {
                file_config.enable_jitter = Some(true);
            }

            if let Some(jitter) = matches.get_one::<String>("max-jitter") {
                file_config.max_jitter_percent = Some(jitter.parse().unwrap_or(25));
            }

            self.config_file = Some(file_config);
        }

        self
    }

    pub fn build(self) -> Result<PatrolConfig, Box<dyn std::error::Error + Send + Sync>> {
        let file_config = self.config_file.unwrap_or_default();

        // Build PatrolConfig from file_config and defaults
        let config = PatrolConfig {
            devices: file_config.devices
                .map(|d| d.into_iter().map(PathBuf::from).collect())
                .unwrap_or_else(Vec::new),
            patrol_period_days: file_config.patrol_period_days.unwrap_or(DEFAULT_PATROL_DAYS),
            read_size: parse_read_size(file_config.read_size.unwrap_or(DEFAULT_READ_SIZE_STR.to_string()).as_str())?,
            state_file: file_config.state_file
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_FILE)),
            log_file: file_config.log_file.map(PathBuf::from),
            verbose: file_config.verbose.unwrap_or(DEFAULT_VERBOSE),
            debug: file_config.debug.unwrap_or(DEFAULT_DEBUG),
            use_syslog: file_config.use_syslog.unwrap_or(DEFAULT_USE_SYSLOG),
            syslog_facility: file_config.syslog_facility.unwrap_or_else(|| DEFAULT_SYSLOG_FACILITY.to_string()),
            email_alerts: file_config.email_alerts.unwrap_or(DEFAULT_EMAIL_ALERTS),
            smtp_server: file_config.smtp_server,
            smtp_port: file_config.smtp_port.unwrap_or(DEFAULT_SMTP_PORT),
            smtp_use_starttls: file_config.smtp_use_starttls.unwrap_or(DEFAULT_SMTP_USE_STARTTLS),
            smtp_username: file_config.smtp_username,
            smtp_password: file_config.smtp_password,
            alert_from: file_config.alert_from,
            alert_to: file_config.alert_to.unwrap_or_else(Vec::new),
            error_threshold: file_config.error_threshold.unwrap_or(DEFAULT_ERROR_THRESHOLD),
            show_progress: file_config.show_progress.unwrap_or(DEFAULT_SHOW_PROGRESS),
            min_sleep_seconds: file_config.min_sleep_seconds,
            max_sleep_seconds: file_config.max_sleep_seconds,
            enable_jitter: file_config.enable_jitter.unwrap_or(DEFAULT_ENABLE_JITTER),
            max_jitter_percent: file_config.max_jitter_percent.unwrap_or(DEFAULT_MAX_JITTER_PERCENT),
        };

        // Validate jitter percentage
        if config.max_jitter_percent > 100 {
            return Err("Jitter percentage cannot exceed 100%".into());
        }
        Ok(config)
    }
}

/// Parse read size with KB/MB units
pub fn parse_read_size(size_str: &str) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let size_str = size_str.trim().to_uppercase();

    if size_str.ends_with("KB") || size_str.ends_with("K") {
        let num_str = size_str.trim_end_matches("KB").trim_end_matches("K");
        let kb: u64 = num_str.parse()?;
        Ok(kb * 1024)
    } else if size_str.ends_with("MB") || size_str.ends_with("M") {
        let num_str = size_str.trim_end_matches("MB").trim_end_matches("M");
        let mb: u64 = num_str.parse()?;
        Ok(mb * 1024 * 1024)
    } else {
        // Assume bytes if no unit specified
        Ok(size_str.parse()?)
    }
}

/// Generate example configuration file content
pub fn generate_example_config() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    // Use the Default implementation which now contains all our defaults
    let example_config = ConfigFile::default();

    // Add a header comment to the generated TOML
    let mut output = String::from("# Disk Patrol Configuration File\n");
    output.push_str("# \n");
    output.push_str("# This is an example configuration with default values.\n");
    output.push_str("# Uncomment and modify settings as needed.\n");
    output.push_str("# Command line arguments will override these settings.\n\n");

    output.push_str(&toml::to_string_pretty(&example_config)?);

    Ok(output)
}

/// Build the command line interface
pub fn build_cli() -> Command {
    Command::new("disk_patrol")
        .version("1.0")
        .about("Multi-threaded disk patrol reader for monitoring disk health")
        .arg(Arg::new("devices")
             .help("Block devices to patrol (e.g., /dev/sda /dev/nvme0n1)")
             .action(clap::ArgAction::Append)
             //             .required_unless_present_any(["config", "generate-config", "status", "reset", "reset-all"])
             .value_name("DEVICE"))
        .arg(Arg::new("config")
             .short('c')
             .long("config")
             .value_name("FILE")
             .help("Configuration file path (TOML format)")
             .default_value(DEFAULT_CONFIG_PATH))
        .arg(Arg::new("generate-config")
             .long("generate-config")
             .value_name("FILE")
             .help(format!("Generate example configuration file (defaults to {})", DEFAULT_CONFIG_PATH))
             .num_args(0..=1)  // Accept 0 or 1 argument
             .default_missing_value(DEFAULT_CONFIG_PATH)  // Default when flag is used without value
             .action(clap::ArgAction::Set))
        .arg(Arg::new("merge-config")
             .long("merge-config")
             .help("Merge configuration file with command line and exit")
             .action(clap::ArgAction::SetTrue))
        .arg(Arg::new("reset")
             .long("reset")
             .help("Reset device state")
             .action(clap::ArgAction::SetTrue))
        .arg(Arg::new("reset-all")
             .long("reset-all")
             .help("Reset state for all devices")
             .action(clap::ArgAction::SetTrue))
        .arg(Arg::new("period")
             .short('p')
             .long("period")
             .value_name("DAYS")
             .help("Patrol period in days")
             .default_value(DEFAULT_PATROL_DAYS_STR))
        .arg(Arg::new("read-size")
             .short('r')
             .long("read-size")
             .value_name("BYTES")
             .help("Read size per operation")
             .default_value(DEFAULT_READ_SIZE_STR))
        .arg(Arg::new("seek")
             .long("seek")
             .value_name("PERCENT")
             .help("Percentage to offset starting position")
            .default_value("0"))
        .arg(Arg::new("state-file")
             .short('s')
             .long("state-file")
             .value_name("FILE")
             .help("State file path")
             .default_value(DEFAULT_STATE_FILE))
        .arg(Arg::new("verbose")
             .short('v')
             .long("verbose")
             .help("Verbose output")
             .action(clap::ArgAction::SetTrue))
        .arg(Arg::new("no-verbose")
             .long("no-verbose")
             .help("Disable verbose output")
             .action(clap::ArgAction::SetTrue)
             .conflicts_with("verbose"))
        .arg(Arg::new("debug")
             .short('d')
             .long("debug")
             .help("Debug output")
             .action(clap::ArgAction::SetTrue))
        .arg(Arg::new("no-debug")
             .long("no-debug")
             .help("Disable debug output")
             .conflicts_with("debug")
             .action(clap::ArgAction::SetTrue))
        .arg(Arg::new("status")
             .long("status")
             .help("Show status and exit")
             .action(clap::ArgAction::SetTrue))
        .arg(Arg::new("syslog")
             .long("syslog")
             .help("Enable syslog logging")
             .action(clap::ArgAction::SetTrue))
        .arg(Arg::new("no-syslog")
             .long("no-syslog")
             .help("Disable syslog logging")
             .action(clap::ArgAction::SetTrue)
             .conflicts_with("syslog"))
        .arg(Arg::new("syslog-facility")
             .long("syslog-facility")
             .value_name("FACILITY")
             .help("Syslog facility (daemon, user, local0-local7)")
             .default_value(DEFAULT_SYSLOG_FACILITY))
        .arg(Arg::new("email-alerts")
             .long("email-alerts")
             .help("Enable email alerts")
             .action(clap::ArgAction::SetTrue))
        .arg(Arg::new("no-email-alerts")
             .long("no-email-alerts")
             .help("Disable email alerts")
             .action(clap::ArgAction::SetTrue)
             .conflicts_with("email-alerts"))
        .arg(Arg::new("smtp-server")
             .long("smtp-server")
             .value_name("SERVER")
             .help("SMTP server hostname"))
        .arg(Arg::new("smtp-port")
             .long("smtp-port")
             .value_name("PORT")
             .help("SMTP server port")
             .default_value(DEFAULT_SMTP_PORT_STR))
        .arg(Arg::new("smtp-starttls")
             .long("smtp-starttls")
             .help("Use STARTTLS for SMTP connection")
             .action(clap::ArgAction::SetTrue))
        .arg(Arg::new("smtp-username")
             .long("smtp-username")
             .value_name("USERNAME")
             .help("SMTP username"))
        .arg(Arg::new("smtp-password")
             .long("smtp-password")
             .value_name("PASSWORD")
             .help("SMTP password"))
        .arg(Arg::new("alert-from")
             .long("alert-from")
             .value_name("EMAIL")
             .help("Alert sender email address"))
        .arg(Arg::new("alert-to")
             .long("alert-to")
             .value_name("EMAIL")
             .help("Alert recipient email address")
             .action(clap::ArgAction::Append))
        .arg(Arg::new("test-email")
             .long("test-email")
             .help("Test email")
             .action(clap::ArgAction::SetTrue))
        .arg(Arg::new("error-threshold")
             .long("error-threshold")
             .value_name("COUNT")
             .help("Number of errors before sending alert")
             .default_value(DEFAULT_ERROR_THRESHOLD_STR))
        .arg(Arg::new("progress")
             .long("progress")
             .help("Show progress bars for each device")
             .action(clap::ArgAction::SetTrue))
        .arg(Arg::new("no-progress")
             .long("no-progress")
             .help("Disable progress bars")
             .action(clap::ArgAction::SetTrue)
             .conflicts_with("progress"))
        .arg(Arg::new("min-sleep")
             .long("min-sleep")
             .value_name("SECONDS")
             .help("Minimum sleep interval between reads")
             .default_value(DEFAULT_MIN_SLEEP_SECONDS_STR))
        .arg(Arg::new("max-sleep")
             .long("max-sleep")
             .value_name("SECONDS")
             .help("Maximum sleep interval between reads")
             .default_value(DEFAULT_MAX_SLEEP_SECONDS_STR))
        .arg(Arg::new("enable-jitter")
             .long("enable-jitter")
             .help("Enable random timing jitter to spread I/O operations")
             .action(clap::ArgAction::SetTrue))
        .arg(Arg::new("max-jitter")
             .long("max-jitter")
             .value_name("PERCENT")
             .help("Maximum jitter as percentage of sleep interval (0-100)")
             .default_value(DEFAULT_MAX_JITTER_PERCENT_STR))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Helper function to create a test config file
    fn create_test_config_file(dir: &TempDir, content: &str) -> PathBuf {
        let config_path = dir.path().join("test_config.toml");
        fs::write(&config_path, content).unwrap();
        config_path
    }

    /// Helper function to parse command line args
    fn parse_args(args: Vec<&str>) -> ArgMatches {
        let cmd = build_cli();
        cmd.try_get_matches_from(args).unwrap()
    }

    #[test]
    fn test_string_override() {
        let temp_dir = TempDir::new().unwrap();
        let config_content = r#"
            state_file = "/original/state.json"
            smtp_server = "original.smtp.com"
            smtp_username = "original_user"
        "#;
        let config_path = create_test_config_file(&temp_dir, config_content);

        // Test command line override
        let args = vec![
            "disk_patrol",
            "--config", config_path.to_str().unwrap(),
            "--state-file", "/new/state.json",
            "--smtp-server", "new.smtp.com",
        ];
        let matches = parse_args(args);

        let config = ConfigBuilder::new()
            .load_config_file(config_path.to_str().unwrap())
            .unwrap()
            .merge_command_line(&matches)
            .build()
            .unwrap();

        assert_eq!(config.state_file, PathBuf::from("/new/state.json"));
        assert_eq!(config.smtp_server, Some("new.smtp.com".to_string()));
        // smtp_username should remain unchanged
        assert_eq!(config.smtp_username, Some("original_user".to_string()));
    }

    #[test]
    fn test_numeric_override() {
        let temp_dir = TempDir::new().unwrap();
        let config_content = r#"
            patrol_period_days = 30
            smtp_port = 25
            error_threshold = 5
            max_jitter_percent = 10
        "#;
        let config_path = create_test_config_file(&temp_dir, config_content);

        let args = vec![
            "disk_patrol",
            "--config", config_path.to_str().unwrap(),
            "--period", "60",
            "--smtp-port", "587",
            "--error-threshold", "10",
        ];
        let matches = parse_args(args);

        let config = ConfigBuilder::new()
            .load_config_file(config_path.to_str().unwrap())
            .unwrap()
            .merge_command_line(&matches)
            .build()
            .unwrap();

        assert_eq!(config.patrol_period_days, 60);
        assert_eq!(config.smtp_port, 587);
        assert_eq!(config.error_threshold, 10);
        // max_jitter_percent should remain unchanged
        assert_eq!(config.max_jitter_percent, 10);
    }

    #[test]
    fn test_boolean_flag_override_true() {
        let temp_dir = TempDir::new().unwrap();
        let config_content = r#"
            verbose = false
            email_alerts = false
            show_progress = false
            enable_jitter = true
            debug = false
        "#;
        let config_path = create_test_config_file(&temp_dir, config_content);

        let args = vec![
            "disk_patrol",
            "--config", config_path.to_str().unwrap(),
            "--verbose",
            "--email-alerts",
            "--progress",
            "--debug",
        ];
        let matches = parse_args(args);

        let config = ConfigBuilder::new()
            .load_config_file(config_path.to_str().unwrap())
            .unwrap()
            .merge_command_line(&matches)
            .build()
            .unwrap();

        assert_eq!(config.verbose, true);
        assert_eq!(config.email_alerts, true);
        assert_eq!(config.show_progress, true);
        // enable_jitter should remain unchanged
        assert_eq!(config.enable_jitter, true);
    }

    #[test]
    fn test_boolean_flag_override_false() {
        let temp_dir = TempDir::new().unwrap();
        let config_content = r#"
            verbose = true
            email_alerts = true
            show_progress = true
            enable_jitter = true
            debug = true
        "#;
        let config_path = create_test_config_file(&temp_dir, config_content);

        let args = vec![
            "disk_patrol",
            "--config", config_path.to_str().unwrap(),
            "--no-verbose",
            "--no-email-alerts",
            "--no-progress",
            "--no-debug",
        ];
        let matches = parse_args(args);

        let config = ConfigBuilder::new()
            .load_config_file(config_path.to_str().unwrap())
            .unwrap()
            .merge_command_line(&matches)
            .build()
            .unwrap();

        assert_eq!(config.verbose, false);
        assert_eq!(config.email_alerts, false);
        assert_eq!(config.show_progress, false);
        // enable_jitter should remain unchanged
        assert_eq!(config.enable_jitter, true);
    }

    #[test]
    fn test_vec_string_override() {
        let temp_dir = TempDir::new().unwrap();
        let config_content = r#"
            devices = ["/dev/sda", "/dev/sdb"]
            alert_to = ["admin@example.com"]
        "#;
        let config_path = create_test_config_file(&temp_dir, config_content);

        let args = vec![
            "disk_patrol",
            "--config", config_path.to_str().unwrap(),
            "/dev/nvme0n1",
            "/dev/nvme1n1",
            "--alert-to", "ops@example.com",
            "--alert-to", "security@example.com",
        ];
        let matches = parse_args(args);

        let config = ConfigBuilder::new()
            .load_config_file(config_path.to_str().unwrap())
            .unwrap()
            .merge_command_line(&matches)
            .build()
            .unwrap();

        assert_eq!(config.devices, vec![PathBuf::from("/dev/nvme0n1"), PathBuf::from("/dev/nvme1n1")]);
        assert_eq!(config.alert_to, vec!["ops@example.com", "security@example.com"]);
    }

    #[test]
    fn test_read_size_parsing() {
        let temp_dir = TempDir::new().unwrap();
        let config_content = r#"
            read_size = "4MB"
        "#;
        let config_path = create_test_config_file(&temp_dir, config_content);

        let args = vec![
            "disk_patrol",
            "--config", config_path.to_str().unwrap(),
            "--read-size", "16MB",
        ];
        let matches = parse_args(args);

        let config = ConfigBuilder::new()
            .load_config_file(config_path.to_str().unwrap())
            .unwrap()
            .merge_command_line(&matches)
            .build()
            .unwrap();

        assert_eq!(config.read_size, 16 * 1024 * 1024);
    }

    #[test]
    fn test_no_config_file_with_cli_args() {
        let args = vec![
            "disk_patrol",
            "/dev/sda",
            "--period", "45",
            "--verbose",
            "--smtp-server", "test.smtp.com",
            "--error-threshold", "3",
        ];
        let matches = parse_args(args);

        let config = ConfigBuilder::new()
            .merge_command_line(&matches)
            .build()
            .unwrap();

        assert_eq!(config.devices, vec![PathBuf::from("/dev/sda")]);
        assert_eq!(config.patrol_period_days, 45);
        assert_eq!(config.verbose, true);
        assert_eq!(config.smtp_server, Some("test.smtp.com".to_string()));
        assert_eq!(config.error_threshold, 3);
    }

    #[test]
    fn test_mixed_overrides() {
        let temp_dir = TempDir::new().unwrap();
        let config_content = r#"
            devices = ["/dev/sda"]
            patrol_period_days = 30
            read_size = "4MB"
            verbose = false
            email_alerts = true
            smtp_server = "config.smtp.com"
            smtp_port = 25
            error_threshold = 5
        "#;
        let config_path = create_test_config_file(&temp_dir, config_content);

        let args = vec![
            "disk_patrol",
            "--config", config_path.to_str().unwrap(),
            "/dev/nvme0n1",  // Override devices
            "--period", "60",  // Override period
            "--verbose",  // Set verbose to true
            "--no-email-alerts",  // Set email_alerts to false
            "--smtp-port", "587",  // Override port
            // Leave smtp_server and error_threshold unchanged
        ];
        let matches = parse_args(args);

        let config = ConfigBuilder::new()
            .load_config_file(config_path.to_str().unwrap())
            .unwrap()
            .merge_command_line(&matches)
            .build()
            .unwrap();

        // Overridden values
        assert_eq!(config.devices, vec![PathBuf::from("/dev/nvme0n1")]);
        assert_eq!(config.patrol_period_days, 60);
        assert_eq!(config.verbose, true);
        assert_eq!(config.email_alerts, false);
        assert_eq!(config.smtp_port, 587);

        // Unchanged values
        assert_eq!(config.read_size, 4 * 1024 * 1024);
        assert_eq!(config.smtp_server, Some("config.smtp.com".to_string()));
        assert_eq!(config.error_threshold, 5);
    }

    #[test]
    fn test_optional_values_remain_none() {
        let temp_dir = TempDir::new().unwrap();
        let config_content = r#"
            # Minimal config with many optional values missing
            devices = ["/dev/sda"]
        "#;
        let config_path = create_test_config_file(&temp_dir, config_content);

        let args = vec![
            "disk_patrol",
            "--config", config_path.to_str().unwrap(),
        ];
        let matches = parse_args(args);

        let config = ConfigBuilder::new()
            .load_config_file(config_path.to_str().unwrap())
            .unwrap()
            .merge_command_line(&matches)
            .build()
            .unwrap();

        // These should use defaults when not specified
        assert_eq!(config.patrol_period_days, DEFAULT_PATROL_DAYS);
        assert_eq!(config.verbose, DEFAULT_VERBOSE);
        assert_eq!(config.email_alerts, DEFAULT_EMAIL_ALERTS);

        // Optional values that can be None
        assert_eq!(config.log_file, None);
        assert_eq!(config.min_sleep_seconds, None);
        assert_eq!(config.max_sleep_seconds, None);
    }

    #[test]
    fn test_parse_read_size_units() {
        assert_eq!(parse_read_size("1024").unwrap(), 1024);
        assert_eq!(parse_read_size("1K").unwrap(), 1024);
        assert_eq!(parse_read_size("1KB").unwrap(), 1024);
        assert_eq!(parse_read_size("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_read_size("1MB").unwrap(), 1024 * 1024);
        assert_eq!(parse_read_size("8MB").unwrap(), 8 * 1024 * 1024);
        assert_eq!(parse_read_size("  16KB  ").unwrap(), 16 * 1024);

        // Test case insensitivity
        assert_eq!(parse_read_size("1mb").unwrap(), 1024 * 1024);
        assert_eq!(parse_read_size("1Mb").unwrap(), 1024 * 1024);
    }

    #[test]
    fn test_value_source_detection() {
        let temp_dir = TempDir::new().unwrap();
        let config_content = r#"
            state_file = "/from/config.json"
            verbose = true
        "#;
        let config_path = create_test_config_file(&temp_dir, config_content);

        // Test 1: Value from config file only
        let args = vec![
            "disk_patrol",
            "--config", config_path.to_str().unwrap(),
        ];
        let matches = parse_args(args);

        // These should NOT have CommandLine as their source
        assert_ne!(matches.value_source("state-file"), Some(ValueSource::CommandLine));
        assert_ne!(matches.value_source("verbose"), Some(ValueSource::CommandLine));

        // Test 2: Value from command line
        let args = vec![
            "disk_patrol",
            "--config", config_path.to_str().unwrap(),
            "--state-file", "/from/cli.json",
            "--no-verbose",
        ];
        let matches = parse_args(args);

        // These SHOULD have CommandLine as their source
        assert_eq!(matches.value_source("state-file"), Some(ValueSource::CommandLine));
        assert_eq!(matches.value_source("no-verbose"), Some(ValueSource::CommandLine));
    }
}
