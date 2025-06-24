/*
#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(deprecated)]
*/

use std::fs;
use std::path::{Path, PathBuf};
use std::fs::OpenOptions;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use std::io::{Seek, SeekFrom};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatrolError {
    pub timestamp: u64,
    pub sector: u64,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sectors: u64,
    pub last_position: u64,
    pub errors: Vec<PatrolError>,
    pub reported_errors: usize,
    pub patrol_start: u64,
    pub serial: String,
    pub wwid: String,
    pub eui: String,
    pub uniq: String,
}

fn get_disk_info(device_name: &Path) -> (std::io::Result<String>, std::io::Result<String>, std::io::Result<String>) {
    // Convert Path to string and remove /dev/ prefix if present
    let device_str = device_name.to_string_lossy();
    let dev_name = device_str.strip_prefix("/dev/").unwrap_or(&device_str);

    let serial_path = format!("/sys/block/{}/device/serial", dev_name);
    let serial = fs::read_to_string(serial_path).map(|s| s.trim().to_string());

    let wwid_path = format!("/sys/block/{}/device/wwid", dev_name);
    let wwid = fs::read_to_string(wwid_path).map(|s| s.trim().to_string());

    let eui_path = format!("/sys/block/{}/eui", dev_name);
    let eui = fs::read_to_string(eui_path).map(|s| s.trim().to_string());

    (serial, wwid, eui)
}

impl DeviceInfo {
    // async fn get_device_info(&self, device_path: &Path) -> Result<DeviceInfo, Box<dyn std::error::Error + Send + Sync>> {
    pub fn from_path(device_path: &Path) -> Result<DeviceInfo, Box<dyn std::error::Error + Send + Sync>> {
        let mut file = OpenOptions::new()
            .read(true)
            .open(device_path)?;

        // Get device size
        file.seek(SeekFrom::End(0))?;
        let size_bytes = file.stream_position()?;
        let sectors = size_bytes / crate::SECTOR_SIZE;
        let serial;
        let wwid;
        let eui;

        match get_disk_info(device_path) {
            (x, y, z) => {
                serial = x.unwrap_or("".to_string());
                wwid   = y.unwrap_or("".to_string());
                eui    = z.unwrap_or("".to_string());
            },
        }

        let uniq = {
            if wwid.len() > 0 {
                format!("{}", wwid)
            } else if eui.len() > 0 {
                format!("eui.{}", eui)
            } else if serial.len() > 0 {
                format!("sn.{}", serial)
            } else {
                panic!("cannot obtain uniq identifier for {:?}", device_path);
            }
        }.chars().filter(|c| !c.is_whitespace()).collect();


        Ok(DeviceInfo {
            path: device_path.to_path_buf(),
            size_bytes,
            sectors,
            last_position: 0,
            errors: Vec::new(),
            reported_errors: 0,
            patrol_start: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            serial,
            wwid,
            eui,
            uniq,
        })
    }
}

pub async fn get_device_info(device_path: &Path) -> Result<DeviceInfo, Box<dyn std::error::Error + Send + Sync>> {
    DeviceInfo::from_path(device_path)
}

#[derive(Debug)]
pub enum DeviceValidationError {
    DuplicatePath { path: PathBuf, existing_device: String },
    DuplicateSerial { serial: String, existing_device: String },
    DuplicateWwid { wwid: String, existing_device: String },
    DuplicateEui { eui: String, existing_device: String },
    DuplicateUniq { uniq: String, existing_device: String },
    Duplicates {},
}

impl std::fmt::Display for DeviceValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceValidationError::DuplicatePath { path, existing_device } => {
                write!(f, "Duplicate path '{}' already used by device '{}'", path.display(), existing_device)
            }
            DeviceValidationError::DuplicateSerial { serial, existing_device } => {
                write!(f, "Duplicate serial '{}' already used by device '{}'", serial, existing_device)
            }
            DeviceValidationError::DuplicateWwid { wwid, existing_device } => {
                write!(f, "Duplicate WWID '{}' already used by device '{}'", wwid, existing_device)
            }
            DeviceValidationError::DuplicateEui { eui, existing_device } => {
                write!(f, "Duplicate EUI '{}' already used by device '{}'", eui, existing_device)
            }
            DeviceValidationError::DuplicateUniq { uniq, existing_device } => {
                write!(f, "Duplicate unique ID '{}' already used by device '{}'", uniq, existing_device)
            }
            DeviceValidationError::Duplicates {} => {
                write!(f, "Duplicates exist in state file")

            }
        }
    }
}

impl std::error::Error for DeviceValidationError {}

pub async fn verify_unique(states: &HashMap<String, DeviceInfo>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut errors = Vec::new();
    let mut paths:   HashMap<PathBuf, String> = HashMap::new();
    let mut serials: HashMap<String, String> = HashMap::new();
    let mut wwids:   HashMap<String, String> = HashMap::new();
    let mut euis:    HashMap<String, String> = HashMap::new();
    let mut uniqs:   HashMap<String, String> = HashMap::new();

    for (id, info) in states.iter() {
        // Check path uniqueness
        if let Some(existing) = paths.get(&info.path) {
            errors.push(DeviceValidationError::DuplicatePath {
                path: info.path.clone(),
                existing_device: existing.clone(),
            });
        } else {
            paths.insert(info.path.clone(), id.clone());
        }

        // Check serial uniqueness (skip if empty)
        if !info.serial.is_empty() {
            if let Some(existing) = serials.get(&info.serial) {
                errors.push(DeviceValidationError::DuplicateSerial {
                    serial: info.serial.clone(),
                    existing_device: existing.clone(),
                });
            } else {
                serials.insert(info.serial.clone(), id.clone());
            }
        }

        // Check WWID uniqueness (skip if empty)
        if !info.wwid.is_empty() {
            if let Some(existing) = wwids.get(&info.wwid) {
                errors.push(DeviceValidationError::DuplicateWwid {
                    wwid: info.wwid.clone(),
                    existing_device: existing.clone(),
                });
            } else {
                wwids.insert(info.wwid.clone(), id.clone());
            }
        }

        // Check EUI uniqueness (skip if empty)
        if !info.eui.is_empty() {
            if let Some(existing) = euis.get(&info.eui) {
                errors.push(DeviceValidationError::DuplicateEui {
                    eui: info.eui.clone(),
                    existing_device: existing.clone(),
                });
            } else {
                euis.insert(info.eui.clone(), id.clone());
            }
        }

        // Check unique ID uniqueness (skip if empty)
        if !info.uniq.is_empty() {
            if let Some(existing) = uniqs.get(&info.uniq) {
                errors.push(DeviceValidationError::DuplicateUniq {
                    uniq: info.uniq.clone(),
                    existing_device: existing.clone(),
                });
            } else {
                uniqs.insert(info.uniq.clone(), id.clone());
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        for e in errors {
            eprintln!("{}", e);
        }
        Err(Box::new(DeviceValidationError::Duplicates {  }))
    }
}
