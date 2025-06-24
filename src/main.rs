//#![allow(unused_imports)]
//#![allow(dead_code)]
//#![allow(unused_variables)]
//#![allow(unused_mut)]
//#![allow(deprecated)]

#[cfg(target_os = "linux")]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use disk_patrol::{ConfigBuilder, generate_example_config, DEFAULT_CONFIG_PATH};
use disk_patrol::verify_config_paths;
use disk_patrol::config::build_cli;
use disk_patrol::PatrolReader;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let matches = build_cli().get_matches();

    // Handle config file generation
    if let Some(path) = matches.get_one::<String>("generate-config") {
        let example_config = generate_example_config()?;
        let path = Path::new(path);

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create directory {}: {}", parent.display(), e))?;
            }
        }

        // Write the config file
        std::fs::write(path, example_config)
            .map_err(|e| format!("Failed to write config file {}: {}", path.display(), e))?;

        println!("Generated example configuration file: {}", path.display());
        println!("Edit this file to configure your devices and settings.");

        return Ok(());
    }

    // Build configuration from config file + command line
    let mut config_builder = ConfigBuilder::new();

    if let Some(config_path) = matches.get_one::<String>("config") {
        config_builder = config_builder.load_config_file(config_path)
            .map_err(|e| format!("Failed to load config file '{}': {}", config_path, e))?;
        println!("Loaded configuration from: {}", config_path);
    }

    if matches.get_flag("merge-config") {
        let path = if let Some(config_path) = matches.get_one::<String>("config") {
            config_path
        } else {
            &DEFAULT_CONFIG_PATH.to_string()
        };
        config_builder.merge_command_line(&matches).save_config_file(path)?;
        return Ok(());
    }

    let config = config_builder
        .merge_command_line(&matches)
        .build()?;

    verify_config_paths(&config)?;

    let patrol_reader = PatrolReader::new(config)?;

    if matches.get_flag("test-email") {
        let subject = format!("test email");
        let body    = format!("test email");
        patrol_reader.send_email_alert(&subject, &body).await?;
        return Ok(());
    }

    // Handle reset operations
    if matches.get_flag("reset-all") {
        patrol_reader.reset_device_state(true, &[]).await?;
        println!("All device states have been reset.");
        return Ok(());
    }

    if matches.get_flag("reset") {
        let devices_to_reset: Vec<PathBuf> = if let Some(devices) = matches.get_many::<String>("devices") {
            devices.map(|s| PathBuf::from(s)).collect()
        } else {
            patrol_reader.config.devices.clone()
        };
        patrol_reader.reset_device_state(false, &devices_to_reset).await?;
        println!("Reset state for {} device(s).", devices_to_reset.len());
        return Ok(());
    }

    // Initialize normally
    patrol_reader.initialize().await?;

    if matches.get_flag("status") {
        patrol_reader.print_status().await?;
    } else {
        let mut seek = 0u8;

        if let Some(arg) = matches.get_one::<String>("seek") {
            seek = arg.parse()?;
            if seek >= 100 {
                eprintln!("seek must be within [0-100)");
                std::process::exit(1);
            }
        }
        patrol_reader.run(seek).await?;
    }

    Ok(())
}
