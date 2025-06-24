//#![allow(unused_imports)]
//#![allow(dead_code)]
//#![allow(unused_variables)]
//#![allow(unused_mut)]
//#![allow(deprecated)]

use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::Mutex;
use syslog::{BasicLogger, Facility, Formatter3164};
use log::{LevelFilter, info, warn, error};
use lettre::{Message, SmtpTransport, Transport};
use lettre::transport::smtp::authentication::Credentials;
use indicatif::{ProgressBar, ProgressStyle, MultiProgress};
use crate::config::PatrolConfig;
use crate::config::SECTOR_SIZE;
use crate::device::DeviceInfo;

pub struct Logger {
    config: PatrolConfig,
    pub progress_bars: Arc<Mutex<HashMap<String, ProgressBar>>>,
    multi_progress: Arc<MultiProgress>,
}

impl Logger {
    pub fn new(config: PatrolConfig) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(ref path) = config.log_file {
            match simple_logging::log_to_file(path, LevelFilter::Info) {
                Err(e) => { panic!("cannot log to {} - {}", path.display(), e); },
                Ok(_) => { info!("logging to {}", path.display()); },
            }
        } else if config.use_syslog {
            let facility = match config.syslog_facility.as_str() {
                "daemon" => Facility::LOG_DAEMON,
                "user" => Facility::LOG_USER,
                "local0" => Facility::LOG_LOCAL0,
                "local1" => Facility::LOG_LOCAL1,
                "local2" => Facility::LOG_LOCAL2,
                "local3" => Facility::LOG_LOCAL3,
                "local4" => Facility::LOG_LOCAL4,
                "local5" => Facility::LOG_LOCAL5,
                "local6" => Facility::LOG_LOCAL6,
                "local7" => Facility::LOG_LOCAL7,
                _ => Facility::LOG_DAEMON,
            };

            let formatter = Formatter3164 {
                facility,
                hostname: None,
                process: "disk_patrol".into(),
                pid: std::process::id(),
            };

            let logger = match syslog::unix(formatter) {
                Err(e) => { panic!("impossible to connect to syslog: {:?}", e); },
                Ok(logger) => logger,
            };
            log::set_boxed_logger(Box::new(BasicLogger::new(logger)))
                .map(|()| log::set_max_level(LevelFilter::Info))?;
        }

        Ok(Logger {
            config,
            progress_bars: Arc::new(Mutex::new(HashMap::new())),
            multi_progress: Arc::new(MultiProgress::new()),
        })
    }

    pub fn log_info(&self, message: &str) {
        if !self.config.show_progress && self.config.verbose {
            println!("INFO: {}", message);
        }

        info!("{}", message);
    }

    pub fn log_error(&self, message: &str) {
        if !self.config.show_progress {
            eprintln!("ERROR: {}", message);
        }

        error!("{}", message);
    }

    pub fn log_warning(&self, message: &str) {
        if !self.config.show_progress {
            println!("WARNING: {}", message);
        }

        warn!("{}", message);
    }

    pub async fn send_email_alert(&self, subject: &str, body: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !self.config.email_alerts {
            return Ok(());
        }

        let smtp_server = self.config.smtp_server.as_ref()
            .ok_or("SMTP server not configured")?;
        let from_addr = self.config.alert_from.as_ref()
            .ok_or("Alert from address not configured")?;

        if self.config.alert_to.is_empty() {
            return Err("No alert recipients configured".into());
        }

        for to_addr in &self.config.alert_to {
            let email = Message::builder()
                .from(from_addr.parse()?)
                .to(to_addr.parse()?)
                .subject(subject)
                .body(body.to_string())?;

            let mut mailer_builder = if self.config.smtp_use_starttls {
                SmtpTransport::starttls_relay(smtp_server)?
            } else {
                SmtpTransport::builder_dangerous(smtp_server)
            };

            // Configure authentication only if credentials are provided
            if let (Some(username), Some(password)) = (
                &self.config.smtp_username,
                &self.config.smtp_password
            ) {
                if username.len() > 0 {
                    let creds = Credentials::new(username.clone(), password.clone());
                    mailer_builder = mailer_builder.credentials(creds);
                }
            }

            // Configure STARTTLS and port
            let mailer = mailer_builder
                .port(self.config.smtp_port)
                .build();

            match mailer.send(&email) {
                Ok(_) => self.log_info(&format!("Alert email sent to {}", to_addr)),
                Err(e) => {
                    self.log_error(&format!("Failed to send email to {}: {}", to_addr, e));
                    if self.config.verbose {
                        self.log_error(&format!("SMTP Debug: server={:?}, port={}, starttls={}, auth={}",
                                               self.config.smtp_server, self.config.smtp_port,
                                               self.config.smtp_use_starttls,
                                               self.config.smtp_username.is_some()));
                    }
                }
            }
        }

        Ok(())
    }

    pub fn progress_msg(&self, pb: &ProgressBar, _uniq: &String, device_info: &DeviceInfo, msg: String) {
        let device_name = device_info.path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let size_tb = device_info.size_bytes as f64 / (1024.0 * 1024.0 * 1024.0 * 1024.0);
        let error_cnt = device_info.errors.len();
        if error_cnt > 0 {
            pb.set_message(format!("{:5.1}TB {:8} ❌ {} ({} errors)", size_tb, device_name, msg, error_cnt));
        } else {
            pb.set_message(format!("{:5.1}TB {:8} - {}", size_tb, device_name, msg));
        }
        pb.set_position(device_info.last_position / SECTOR_SIZE);
    }

    pub async fn create_progress_bar(&self, uniq: &String, device_info: &DeviceInfo) {
        let pb = self.multi_progress.add(ProgressBar::new(device_info.sectors));

        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] {bar:20.cyan/blue} {msg}")
                .unwrap()
                .progress_chars("█▉▊▋▌▍▎▏  ")
        );

        self.progress_msg(&pb, uniq, device_info, format!("- Starting patrol at {:16x}", device_info.size_bytes / SECTOR_SIZE));

        let mut progress_bars = self.progress_bars.lock().await;
        progress_bars.insert(uniq.clone(), pb);
    }
}
