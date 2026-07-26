use sysinfo::{System, SystemExt, DiskExt};
use std::time::{Instant, Duration};

pub struct StorageEstimator {
    sys: System,
    write_speed_mbps: f64, // Moving average
}

impl StorageEstimator {
    pub fn new() -> Self {
        let mut sys = System::new_all();
        sys.refresh_disks();
        Self {
            sys,
            write_speed_mbps: 500.0, // Assume SSD default
        }
    }

    pub fn estimate_remaining(&mut self, current_project_size: u64) -> (String, String) {
        self.sys.refresh_disks();
        
        let mut total_free = 0;
        for disk in self.sys.disks() {
            if disk.mount_point() == std::path::Path::new("/") {
                total_free = disk.available_space();
                break;
            }
        }

        // Time Estimate @ 500 MB/s (approx raw burst)
        // Or calculate based on project capture rate
        let time_secs = total_free as f64 / (self.write_speed_mbps * 1_000_000.0);
        let time_str = if time_secs > 3600.0 {
            format!("{:.1}h", time_secs / 3600.0)
        } else {
            format!("{:.0}m", time_secs / 60.0)
        };

        // Size Str
        let size_str = if total_free > 1_000_000_000 {
            format!("{:.1}GB", total_free as f64 / 1_000_000_000.0)
        } else {
             format!("{:.0}MB", total_free as f64 / 1_000_000.0)
        };

        (size_str, time_str)
    }
}
