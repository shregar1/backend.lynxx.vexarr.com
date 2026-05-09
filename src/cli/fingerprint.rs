//! Client hardware fingerprinting logic.

use machine_uid;
use sha2::{Digest, Sha256};

pub fn get_hardware_id() -> String {
    // machine_uid is a common crate for getting unique HW IDs.
    // For this demonstration, we'll use it or fallback to a placeholder.
    let uid = machine_uid::get().unwrap_or_else(|_| "unknown-device".to_string());
    
    let mut hasher = Sha256::new();
    hasher.update(uid.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn get_os_info() -> String {
    format!(
        "{} {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH,
        whoami::distro()
    )
}
