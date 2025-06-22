// src/lib.rs
pub mod config;

pub use config::{ConfigBuilder, ConfigFile, PatrolConfig};
pub use config::{verify_config_paths, generate_example_config};
pub use config::SECTOR_SIZE;
pub use config::DEFAULT_CONFIG_PATH;

pub mod device;
pub use device::get_device_info;
