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
use std::os::unix::fs::OpenOptionsExt;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use std::io::{Seek, SeekFrom};

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
            .custom_flags(libc::O_DIRECT)
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
                format!("wwn.{}", wwid)
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
