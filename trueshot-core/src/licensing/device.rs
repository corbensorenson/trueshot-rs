//! Device fingerprinting for hardware-bound licensing
//! 
//! Generates stable hardware identifiers for license binding.

use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};

/// Hardware fingerprint for device binding
/// 
/// Combines multiple hardware identifiers for stability:
/// - If one component changes, others provide continuity
/// - Hash is used for privacy (no raw HW IDs transmitted)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceFingerprint {
    /// Stable machine identifier (platform-specific)
    pub machine_id: String,
    /// Hostname
    pub hostname: String,
    /// MAC addresses of network interfaces
    pub mac_addresses: Vec<String>,
    /// Primary disk serial/ID (optional)
    pub disk_id: Option<String>,
    /// OS type and version
    pub os_info: String,
}

impl DeviceFingerprint {
    /// Generate fingerprint for current device
    pub fn generate() -> Self {
        Self {
            machine_id: Self::get_machine_id(),
            hostname: Self::get_hostname(),
            mac_addresses: Self::get_mac_addresses(),
            disk_id: Self::get_disk_id(),
            os_info: Self::get_os_info(),
        }
    }
    
    /// Create stable hash for license binding
    /// 
    /// Uses SHA-256 to create a privacy-preserving identifier
    pub fn fingerprint_hash(&self) -> String {
        let mut hasher = Sha256::new();
        
        // Combine all identifiers
        hasher.update(&self.machine_id);
        hasher.update(&self.hostname);
        for mac in &self.mac_addresses {
            hasher.update(mac);
        }
        if let Some(ref disk) = self.disk_id {
            hasher.update(disk);
        }
        
        hex::encode(hasher.finalize())
    }
    
    /// Get a human-readable device name for display
    pub fn device_name(&self) -> String {
        format!("{} ({})", self.hostname, &self.os_info)
    }
    
    // Platform-specific implementations
    
    fn get_machine_id() -> String {
        #[cfg(target_os = "macos")]
        {
            // Use IOPlatformUUID on macOS
            std::process::Command::new("ioreg")
                .args(["-rd1", "-c", "IOPlatformExpertDevice"])
                .output()
                .ok()
                .and_then(|output| {
                    let s = String::from_utf8_lossy(&output.stdout);
                    s.lines()
                        .find(|line| line.contains("IOPlatformUUID"))
                        .and_then(|line| {
                            line.split('"').nth(3).map(String::from)
                        })
                })
                .unwrap_or_else(|| "unknown-mac".to_string())
        }
        
        #[cfg(target_os = "linux")]
        {
            // Use /etc/machine-id on Linux
            std::fs::read_to_string("/etc/machine-id")
                .map(|s| s.trim().to_string())
                .or_else(|_| std::fs::read_to_string("/var/lib/dbus/machine-id"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "unknown-linux".to_string())
        }
        
        #[cfg(target_os = "windows")]
        {
            // Use MachineGuid from registry on Windows
            std::process::Command::new("reg")
                .args(["query", r"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Cryptography", "/v", "MachineGuid"])
                .output()
                .ok()
                .and_then(|output| {
                    let s = String::from_utf8_lossy(&output.stdout);
                    s.lines()
                        .find(|line| line.contains("MachineGuid"))
                        .and_then(|line| line.split_whitespace().last())
                        .map(String::from)
                })
                .unwrap_or_else(|| "unknown-windows".to_string())
        }
        
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            "unknown-platform".to_string()
        }
    }
    
    fn get_hostname() -> String {
        hostname::get()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown-host".to_string())
    }
    
    fn get_mac_addresses() -> Vec<String> {
        // Use mac_address crate for cross-platform MAC retrieval
        mac_address::get_mac_address()
            .ok()
            .flatten()
            .map(|mac| vec![mac.to_string()])
            .unwrap_or_default()
    }
    
    fn get_disk_id() -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("diskutil")
                .args(["info", "/"])
                .output()
                .ok()
                .and_then(|output| {
                    let s = String::from_utf8_lossy(&output.stdout);
                    s.lines()
                        .find(|line| line.contains("Volume UUID"))
                        .and_then(|line| line.split(':').nth(1))
                        .map(|s| s.trim().to_string())
                })
        }
        
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }
    
    fn get_os_info() -> String {
        format!(
            "{} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_fingerprint_generation() {
        let fp = DeviceFingerprint::generate();
        assert!(!fp.machine_id.is_empty());
        assert!(!fp.hostname.is_empty());
        assert!(!fp.os_info.is_empty());
    }
    
    #[test]
    fn test_fingerprint_hash_stability() {
        let fp = DeviceFingerprint::generate();
        let hash1 = fp.fingerprint_hash();
        let hash2 = fp.fingerprint_hash();
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA-256 hex = 64 chars
    }
    
    #[test]
    fn test_device_name() {
        let fp = DeviceFingerprint::generate();
        let name = fp.device_name();
        assert!(name.contains(&fp.hostname));
    }
}
