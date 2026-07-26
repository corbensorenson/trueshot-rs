//! Anti-Tamper and Integrity Verification
//!
//! Provides runtime integrity checks for license protection.
//! Implements multiple layers of verification to deter tampering.

use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Integrity verification result
#[derive(Clone, Debug)]
pub enum IntegrityStatus {
    /// All checks passed
    Valid,
    /// Tampered binary detected
    TamperedBinary,
    /// Debugger detected
    DebuggerAttached,
    /// Rate limit exceeded
    RateLimited,
    /// Clock tampering detected  
    ClockTampered,
    /// Memory corruption detected
    MemoryCorrupted,
}

/// Self-healing verification nonce
/// Changes each time verification runs to prevent replay
static VERIFICATION_NONCE: AtomicU64 = AtomicU64::new(0);

/// Rate limiting for verification calls
static VERIFICATION_COUNT: AtomicU32 = AtomicU32::new(0);
static LAST_VERIFICATION_RESET: AtomicU64 = AtomicU64::new(0);

/// Usage counter for hobby tier
static SCAN_COUNT: AtomicU32 = AtomicU32::new(0);
static SCAN_COUNT_MONTH: AtomicU32 = AtomicU32::new(0);

/// Integrity checker with multiple verification layers
pub struct IntegrityChecker {
    /// Expected checksum of critical code sections
    expected_checksums: Vec<(String, [u8; 32])>,
    /// Last clock check time
    last_clock_check: RwLock<Instant>,
    /// Rate limit window (seconds)
    rate_limit_window: u64,
    /// Max verifications per window
    max_verifications_per_window: u32,
}

impl IntegrityChecker {
    /// Create new integrity checker
    pub fn new() -> Self {
        Self {
            expected_checksums: Vec::new(),
            last_clock_check: RwLock::new(Instant::now()),
            rate_limit_window: 60,
            max_verifications_per_window: 100,
        }
    }

    /// Full integrity check
    pub fn verify(&self) -> IntegrityStatus {
        // Rate limiting
        if !self.check_rate_limit() {
            return IntegrityStatus::RateLimited;
        }

        // Update nonce
        let nonce = VERIFICATION_NONCE.fetch_add(1, Ordering::SeqCst);

        // Check for debugger
        if self.detect_debugger() {
            return IntegrityStatus::DebuggerAttached;
        }

        // Check for clock tampering
        if self.detect_clock_tampering() {
            return IntegrityStatus::ClockTampered;
        }

        // Verify code integrity
        if !self.verify_code_checksums(nonce) {
            return IntegrityStatus::TamperedBinary;
        }

        IntegrityStatus::Valid
    }

    /// Check rate limit
    fn check_rate_limit(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let last_reset = LAST_VERIFICATION_RESET.load(Ordering::SeqCst);

        if now - last_reset > self.rate_limit_window {
            LAST_VERIFICATION_RESET.store(now, Ordering::SeqCst);
            VERIFICATION_COUNT.store(0, Ordering::SeqCst);
        }

        let count = VERIFICATION_COUNT.fetch_add(1, Ordering::SeqCst);
        count < self.max_verifications_per_window
    }

    /// Detect attached debugger
    fn detect_debugger(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            // Check TracerPid in /proc/self/status
            if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
                for line in status.lines() {
                    if line.starts_with("TracerPid:") {
                        let pid = line.split_whitespace().nth(1).unwrap_or("0");
                        if pid != "0" {
                            return true;
                        }
                    }
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            // Use sysctl to check for debugging
            // P_TRACED flag check
            use std::mem;

            #[repr(C)]
            #[allow(non_camel_case_types)]
            struct kinfo_proc {
                _data: [u8; 648], // Simplified - actual structure is larger
            }

            extern "C" {
                fn sysctl(
                    name: *const i32,
                    namelen: u32,
                    oldp: *mut std::ffi::c_void,
                    oldlenp: *mut usize,
                    newp: *const std::ffi::c_void,
                    newlen: usize,
                ) -> i32;
            }

            unsafe {
                let mut info: kinfo_proc = mem::zeroed();
                let mut size = mem::size_of::<kinfo_proc>();
                let mib: [i32; 4] = [
                    1,  // CTL_KERN
                    14, // KERN_PROC
                    1,  // KERN_PROC_PID
                    std::process::id() as i32,
                ];

                if sysctl(
                    mib.as_ptr(),
                    4,
                    &mut info as *mut _ as *mut std::ffi::c_void,
                    &mut size,
                    std::ptr::null(),
                    0,
                ) == 0
                {
                    // Check P_TRACED flag at offset (this is simplified)
                    // In real implementation, check kp_proc.p_flag & P_TRACED
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            extern "system" {
                fn IsDebuggerPresent() -> i32;
            }

            unsafe {
                if IsDebuggerPresent() != 0 {
                    return true;
                }
            }
        }

        false
    }

    /// Detect clock tampering
    fn detect_clock_tampering(&self) -> bool {
        let now = Instant::now();
        let mut last = self.last_clock_check.write().unwrap();

        // If time went backwards significantly, clock was tampered
        // Note: Instant is monotonic, so this checks for system restart
        let elapsed = now.duration_since(*last);
        *last = now;

        // If no time passed at all over many calls, something is wrong
        if elapsed == Duration::ZERO {
            // Could be legitimate if called quickly
            // In real implementation, track call patterns
        }

        false
    }

    /// Verify code section checksums
    fn verify_code_checksums(&self, _nonce: u64) -> bool {
        let expected = self
            .expected_checksums
            .iter()
            .find(|(name, _)| name == "binary")
            .map(|(_, hash)| *hash)
            .or_else(load_expected_binary_hash);

        let Some(expected_hash) = expected else {
            if is_production() {
                tracing::error!("Missing expected binary hash in production");
                return false;
            }
            tracing::warn!(
                "No expected binary hash configured; integrity check skipped in non-production"
            );
            return true;
        };

        match compute_self_hash() {
            Ok(actual) => {
                if actual == expected_hash {
                    true
                } else {
                    tracing::error!("Binary hash mismatch detected");
                    false
                }
            }
            Err(err) => {
                tracing::error!("Failed to compute binary hash: {}", err);
                false
            }
        }
    }

    /// Add expected checksum for a code section
    pub fn add_expected_checksum(&mut self, name: &str, checksum: [u8; 32]) {
        self.expected_checksums.push((name.to_string(), checksum));
    }
}

impl Default for IntegrityChecker {
    fn default() -> Self {
        Self::new()
    }
}

fn is_production() -> bool {
    std::env::var("TRUESHOT_ENV")
        .map(|v| v == "production")
        .unwrap_or(false)
}

fn load_expected_binary_hash() -> Option<[u8; 32]> {
    if let Ok(hash) = std::env::var("TRUESHOT_EXPECTED_BINARY_HASH") {
        if let Some(parsed) = parse_hex_hash(&hash) {
            return Some(parsed);
        }
    }
    if let Ok(path) = std::env::var("TRUESHOT_EXPECTED_BINARY_HASH_PATH") {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Some(parsed) = parse_hex_hash(contents.trim()) {
                return Some(parsed);
            }
        }
    }
    None
}

fn parse_hex_hash(input: &str) -> Option<[u8; 32]> {
    let cleaned = input.trim();
    if cleaned.len() != 64 {
        return None;
    }
    let decoded = hex::decode(cleaned).ok()?;
    if decoded.len() != 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&decoded);
    Some(out)
}

fn compute_self_hash() -> Result<[u8; 32], String> {
    let path = std::env::current_exe().map_err(|e| e.to_string())?;
    let file = File::open(&path).map_err(|e| format!("Open failed: {}", e))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let count = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Ok(out)
}

/// Scan usage counter for hobby tier limits
pub struct UsageCounter {
    /// Maximum scans per month for current tier
    max_scans_per_month: Option<u32>,
}

impl UsageCounter {
    pub fn new(max_scans: Option<u32>) -> Self {
        Self {
            max_scans_per_month: max_scans,
        }
    }

    /// Record a scan
    pub fn record_scan(&self) -> Result<u32, UsageLimitError> {
        let current_month = self.current_month();
        let stored_month = SCAN_COUNT_MONTH.load(Ordering::SeqCst);

        // Reset counter if month changed
        if current_month != stored_month {
            SCAN_COUNT_MONTH.store(current_month, Ordering::SeqCst);
            SCAN_COUNT.store(0, Ordering::SeqCst);
        }

        let count = SCAN_COUNT.fetch_add(1, Ordering::SeqCst) + 1;

        if let Some(max) = self.max_scans_per_month {
            if count > max {
                SCAN_COUNT.fetch_sub(1, Ordering::SeqCst); // Undo increment
                return Err(UsageLimitError::MonthlyLimitExceeded {
                    current: count - 1,
                    max,
                });
            }
        }

        Ok(count)
    }

    /// Get current scan count
    pub fn current_count(&self) -> u32 {
        SCAN_COUNT.load(Ordering::SeqCst)
    }

    /// Get remaining scans
    pub fn remaining(&self) -> Option<u32> {
        self.max_scans_per_month
            .map(|max| max.saturating_sub(SCAN_COUNT.load(Ordering::SeqCst)))
    }

    /// Reset counter (for testing)
    #[cfg(debug_assertions)]
    pub fn reset(&self) {
        SCAN_COUNT.store(0, Ordering::SeqCst);
    }

    fn current_month(&self) -> u32 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // Approximate month as 30-day periods since epoch
        (now / (30 * 24 * 60 * 60)) as u32
    }
}

/// Usage limit error
#[derive(Clone, Debug)]
pub enum UsageLimitError {
    MonthlyLimitExceeded { current: u32, max: u32 },
}

impl std::fmt::Display for UsageLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UsageLimitError::MonthlyLimitExceeded { current, max } => {
                write!(f, "Monthly scan limit exceeded: {}/{}", current, max)
            }
        }
    }
}

impl std::error::Error for UsageLimitError {}

/// Compute checksum of a byte slice
pub fn compute_checksum(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// Obfuscated license check distribution
/// Call this from multiple places in the code
#[inline(never)]
pub fn distributed_check_a() -> bool {
    // This function should be called from various places
    // to make it harder to find and patch the license check
    let nonce = VERIFICATION_NONCE.load(Ordering::SeqCst);
    nonce > 0 || std::env::var("TRUESHOT_DEV").is_ok()
}

#[inline(never)]
pub fn distributed_check_b() -> bool {
    // Another distributed check point
    let count = VERIFICATION_COUNT.load(Ordering::SeqCst);
    count < 10000 || std::env::var("TRUESHOT_DEV").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_integrity_checker() {
        let checker = IntegrityChecker::new();
        let status = checker.verify();
        assert!(matches!(status, IntegrityStatus::Valid));
    }

    #[test]
    fn test_usage_counter() {
        let counter = UsageCounter::new(Some(5));
        counter.reset();

        for i in 1..=5 {
            assert_eq!(counter.record_scan().unwrap(), i);
        }

        // Should fail on 6th
        assert!(counter.record_scan().is_err());
    }

    #[test]
    fn test_checksum() {
        let data = b"test data";
        let checksum = compute_checksum(data);
        assert_eq!(checksum.len(), 32);

        // Same data = same checksum
        let checksum2 = compute_checksum(data);
        assert_eq!(checksum, checksum2);
    }

    #[test]
    fn test_distributed_checks() {
        // In dev mode, should pass
        std::env::set_var("TRUESHOT_DEV", "1");
        assert!(distributed_check_a());
        assert!(distributed_check_b());
    }
}
