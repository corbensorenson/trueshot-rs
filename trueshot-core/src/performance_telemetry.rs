//! Process-scoped performance evidence for release qualification.
//!
//! macOS uses `proc_pid_rusage` for counters and `NSProcessInfo` for current
//! thermal/power state. Unsupported platforms retain an explicit unavailable
//! record rather than fabricating zero-valued measurements.

use serde::{Deserialize, Serialize};

pub const PROCESS_TELEMETRY_SCHEMA: &str = "trueshot.process-telemetry.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThermalState {
    Nominal,
    Fair,
    Serious,
    Critical,
    Unknown,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProcessResourceCounters {
    pub user_cpu_time_ns: u64,
    pub system_cpu_time_ns: u64,
    pub disk_bytes_read: u64,
    pub disk_bytes_written: u64,
    pub logical_writes: u64,
    pub pageins: u64,
    pub instructions: u64,
    pub cycles: u64,
    pub energy_nj: u64,
    pub performance_energy_nj: u64,
}

impl ProcessResourceCounters {
    fn saturating_delta(&self, before: &Self) -> Self {
        Self {
            user_cpu_time_ns: self
                .user_cpu_time_ns
                .saturating_sub(before.user_cpu_time_ns),
            system_cpu_time_ns: self
                .system_cpu_time_ns
                .saturating_sub(before.system_cpu_time_ns),
            disk_bytes_read: self.disk_bytes_read.saturating_sub(before.disk_bytes_read),
            disk_bytes_written: self
                .disk_bytes_written
                .saturating_sub(before.disk_bytes_written),
            logical_writes: self.logical_writes.saturating_sub(before.logical_writes),
            pageins: self.pageins.saturating_sub(before.pageins),
            instructions: self.instructions.saturating_sub(before.instructions),
            cycles: self.cycles.saturating_sub(before.cycles),
            energy_nj: self.energy_nj.saturating_sub(before.energy_nj),
            performance_energy_nj: self
                .performance_energy_nj
                .saturating_sub(before.performance_energy_nj),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProcessTelemetrySnapshot {
    pub available: bool,
    pub error: Option<String>,
    pub thermal_state: Option<ThermalState>,
    pub low_power_mode: Option<bool>,
    pub resident_size_bytes: Option<u64>,
    pub physical_footprint_bytes: Option<u64>,
    pub lifetime_peak_physical_footprint_bytes: Option<u64>,
    pub interval_peak_physical_footprint_bytes: Option<u64>,
    pub maximum_resident_set_size_bytes: Option<u64>,
    pub counters: ProcessResourceCounters,
}

impl ProcessTelemetrySnapshot {
    pub fn capture() -> Self {
        platform::capture()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProcessTelemetryWindow {
    pub schema: &'static str,
    pub available: bool,
    pub error: Option<String>,
    pub start_thermal_state: Option<ThermalState>,
    pub end_thermal_state: Option<ThermalState>,
    pub maximum_thermal_state: Option<ThermalState>,
    pub low_power_mode_observed: Option<bool>,
    pub peak_physical_footprint_bytes: Option<u64>,
    pub maximum_resident_set_size_bytes: Option<u64>,
    pub energy_measurement_available: bool,
    pub counters: ProcessResourceCounters,
}

impl ProcessTelemetryWindow {
    pub fn between(before: &ProcessTelemetrySnapshot, after: &ProcessTelemetrySnapshot) -> Self {
        let available = before.available && after.available;
        let error = match (&before.error, &after.error) {
            (Some(before), Some(after)) if before != after => Some(format!("{before}; {after}")),
            (Some(error), _) | (_, Some(error)) => Some(error.clone()),
            _ => None,
        };
        let maximum_thermal_state = match (before.thermal_state, after.thermal_state) {
            (Some(before), Some(after)) => Some(before.max(after)),
            (state @ Some(_), None) | (None, state @ Some(_)) => state,
            (None, None) => None,
        };
        let low_power_mode_observed = match (before.low_power_mode, after.low_power_mode) {
            (Some(before), Some(after)) => Some(before || after),
            (state @ Some(_), None) | (None, state @ Some(_)) => state,
            (None, None) => None,
        };
        let peak_physical_footprint_bytes = [
            before.lifetime_peak_physical_footprint_bytes,
            before.interval_peak_physical_footprint_bytes,
            after.lifetime_peak_physical_footprint_bytes,
            after.interval_peak_physical_footprint_bytes,
        ]
        .into_iter()
        .flatten()
        .max();
        let maximum_resident_set_size_bytes = [
            before.maximum_resident_set_size_bytes,
            after.maximum_resident_set_size_bytes,
        ]
        .into_iter()
        .flatten()
        .max();
        let counters = after.counters.saturating_delta(&before.counters);
        let energy_measurement_available =
            available && (counters.energy_nj > 0 || counters.performance_energy_nj > 0);

        Self {
            schema: PROCESS_TELEMETRY_SCHEMA,
            available,
            error,
            start_thermal_state: before.thermal_state,
            end_thermal_state: after.thermal_state,
            maximum_thermal_state,
            low_power_mode_observed,
            peak_physical_footprint_bytes,
            maximum_resident_set_size_bytes,
            energy_measurement_available,
            counters,
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{ProcessResourceCounters, ProcessTelemetrySnapshot, ThermalState};
    use std::ffi::{c_char, c_void};
    use std::mem::MaybeUninit;

    const RUSAGE_INFO_V6: libc::c_int = 6;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RusageInfoV6 {
        uuid: [u8; 16],
        user_time: u64,
        system_time: u64,
        pkg_idle_wkups: u64,
        interrupt_wkups: u64,
        pageins: u64,
        wired_size: u64,
        resident_size: u64,
        phys_footprint: u64,
        proc_start_abstime: u64,
        proc_exit_abstime: u64,
        child_user_time: u64,
        child_system_time: u64,
        child_pkg_idle_wkups: u64,
        child_interrupt_wkups: u64,
        child_pageins: u64,
        child_elapsed_abstime: u64,
        diskio_bytesread: u64,
        diskio_byteswritten: u64,
        cpu_time_qos_default: u64,
        cpu_time_qos_maintenance: u64,
        cpu_time_qos_background: u64,
        cpu_time_qos_utility: u64,
        cpu_time_qos_legacy: u64,
        cpu_time_qos_user_initiated: u64,
        cpu_time_qos_user_interactive: u64,
        billed_system_time: u64,
        serviced_system_time: u64,
        logical_writes: u64,
        lifetime_max_phys_footprint: u64,
        instructions: u64,
        cycles: u64,
        billed_energy: u64,
        serviced_energy: u64,
        interval_max_phys_footprint: u64,
        runnable_time: u64,
        flags: u64,
        user_ptime: u64,
        system_ptime: u64,
        pinstructions: u64,
        pcycles: u64,
        energy_nj: u64,
        penergy_nj: u64,
        secure_time_in_system: u64,
        secure_ptime_in_system: u64,
        neural_footprint: u64,
        lifetime_max_neural_footprint: u64,
        interval_max_neural_footprint: u64,
        reserved: [u64; 9],
    }

    extern "C" {
        fn proc_pid_rusage(
            pid: libc::c_int,
            flavor: libc::c_int,
            buffer: *mut c_void,
        ) -> libc::c_int;
    }

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}

    pub(super) fn capture() -> ProcessTelemetrySnapshot {
        let mut usage = MaybeUninit::<RusageInfoV6>::zeroed();
        // SAFETY: `usage` points to a correctly sized writable V6 structure and
        // is only assumed initialized after `proc_pid_rusage` reports success.
        let status = unsafe {
            proc_pid_rusage(
                libc::getpid(),
                RUSAGE_INFO_V6,
                usage.as_mut_ptr().cast::<c_void>(),
            )
        };
        if status != 0 {
            return unavailable(format!(
                "proc_pid_rusage failed with errno {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: status zero guarantees that the V6 output was initialized.
        let usage = unsafe { usage.assume_init() };

        let mut standard_usage = MaybeUninit::<libc::rusage>::zeroed();
        // SAFETY: getrusage initializes the supplied structure on success.
        let standard_status =
            unsafe { libc::getrusage(libc::RUSAGE_SELF, standard_usage.as_mut_ptr()) };
        let maximum_resident_set_size_bytes = if standard_status == 0 {
            // macOS reports ru_maxrss in bytes.
            Some(
                // SAFETY: status zero guarantees initialization.
                unsafe { standard_usage.assume_init() }.ru_maxrss.max(0) as u64,
            )
        } else {
            None
        };
        let (thermal_state, low_power_mode) = process_environment();

        ProcessTelemetrySnapshot {
            available: true,
            error: None,
            thermal_state: Some(thermal_state),
            low_power_mode: Some(low_power_mode),
            resident_size_bytes: Some(usage.resident_size),
            physical_footprint_bytes: Some(usage.phys_footprint),
            lifetime_peak_physical_footprint_bytes: Some(usage.lifetime_max_phys_footprint),
            interval_peak_physical_footprint_bytes: Some(usage.interval_max_phys_footprint),
            maximum_resident_set_size_bytes,
            counters: ProcessResourceCounters {
                user_cpu_time_ns: usage.user_time,
                system_cpu_time_ns: usage.system_time,
                disk_bytes_read: usage.diskio_bytesread,
                disk_bytes_written: usage.diskio_byteswritten,
                logical_writes: usage.logical_writes,
                pageins: usage.pageins,
                instructions: usage.instructions,
                cycles: usage.cycles,
                energy_nj: usage.energy_nj,
                performance_energy_nj: usage.penergy_nj,
            },
        }
    }

    fn process_environment() -> (ThermalState, bool) {
        type SendObject = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
        type SendInteger = unsafe extern "C" fn(*mut c_void, *mut c_void) -> isize;
        type SendBool = unsafe extern "C" fn(*mut c_void, *mut c_void) -> i8;

        // SAFETY: selectors are NUL-terminated static ASCII, NSProcessInfo is a
        // process-lifetime singleton, and each typed call matches the declared
        // Objective-C method ABI and scalar return type on 64-bit macOS.
        unsafe {
            let class = objc_getClass(c"NSProcessInfo".as_ptr());
            let process_info_selector = sel_registerName(c"processInfo".as_ptr());
            let thermal_selector = sel_registerName(c"thermalState".as_ptr());
            let low_power_selector = sel_registerName(c"isLowPowerModeEnabled".as_ptr());
            let send_object: SendObject = std::mem::transmute(objc_msgSend as *const ());
            let send_integer: SendInteger = std::mem::transmute(objc_msgSend as *const ());
            let send_bool: SendBool = std::mem::transmute(objc_msgSend as *const ());
            let process_info = send_object(class, process_info_selector);
            let thermal = send_integer(process_info, thermal_selector);
            let low_power = send_bool(process_info, low_power_selector);
            let thermal = match thermal {
                0 => ThermalState::Nominal,
                1 => ThermalState::Fair,
                2 => ThermalState::Serious,
                3 => ThermalState::Critical,
                _ => ThermalState::Unknown,
            };
            (thermal, low_power != 0)
        }
    }

    fn unavailable(error: String) -> ProcessTelemetrySnapshot {
        ProcessTelemetrySnapshot {
            available: false,
            error: Some(error),
            thermal_state: None,
            low_power_mode: None,
            resident_size_bytes: None,
            physical_footprint_bytes: None,
            lifetime_peak_physical_footprint_bytes: None,
            interval_peak_physical_footprint_bytes: None,
            maximum_resident_set_size_bytes: None,
            counters: ProcessResourceCounters::default(),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::{ProcessResourceCounters, ProcessTelemetrySnapshot};

    pub(super) fn capture() -> ProcessTelemetrySnapshot {
        ProcessTelemetrySnapshot {
            available: false,
            error: Some("native process telemetry is currently supported on macOS".to_string()),
            thermal_state: None,
            low_power_mode: None,
            resident_size_bytes: None,
            physical_footprint_bytes: None,
            lifetime_peak_physical_footprint_bytes: None,
            interval_peak_physical_footprint_bytes: None,
            maximum_resident_set_size_bytes: None,
            counters: ProcessResourceCounters::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(counters: ProcessResourceCounters) -> ProcessTelemetrySnapshot {
        ProcessTelemetrySnapshot {
            available: true,
            error: None,
            thermal_state: Some(ThermalState::Nominal),
            low_power_mode: Some(false),
            resident_size_bytes: Some(10),
            physical_footprint_bytes: Some(20),
            lifetime_peak_physical_footprint_bytes: Some(30),
            interval_peak_physical_footprint_bytes: Some(25),
            maximum_resident_set_size_bytes: Some(40),
            counters,
        }
    }

    #[test]
    fn telemetry_window_uses_counter_deltas_and_peak_values() {
        let before = snapshot(ProcessResourceCounters {
            user_cpu_time_ns: 100,
            disk_bytes_read: 50,
            energy_nj: 7,
            ..Default::default()
        });
        let mut after = snapshot(ProcessResourceCounters {
            user_cpu_time_ns: 160,
            disk_bytes_read: 90,
            energy_nj: 19,
            ..Default::default()
        });
        after.thermal_state = Some(ThermalState::Fair);
        after.low_power_mode = Some(true);
        after.lifetime_peak_physical_footprint_bytes = Some(80);
        after.maximum_resident_set_size_bytes = Some(70);

        let window = ProcessTelemetryWindow::between(&before, &after);
        assert_eq!(window.counters.user_cpu_time_ns, 60);
        assert_eq!(window.counters.disk_bytes_read, 40);
        assert_eq!(window.counters.energy_nj, 12);
        assert_eq!(window.maximum_thermal_state, Some(ThermalState::Fair));
        assert_eq!(window.low_power_mode_observed, Some(true));
        assert_eq!(window.peak_physical_footprint_bytes, Some(80));
        assert_eq!(window.maximum_resident_set_size_bytes, Some(70));
        assert!(window.energy_measurement_available);
    }

    #[test]
    fn counter_reset_never_underflows() {
        let before = snapshot(ProcessResourceCounters {
            instructions: 100,
            ..Default::default()
        });
        let after = snapshot(ProcessResourceCounters {
            instructions: 10,
            ..Default::default()
        });
        assert_eq!(
            ProcessTelemetryWindow::between(&before, &after)
                .counters
                .instructions,
            0
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_native_snapshot_is_available_and_serializable() {
        let before = ProcessTelemetrySnapshot::capture();
        assert!(before.available, "{:?}", before.error);
        assert!(before.thermal_state.is_some());
        assert!(before.low_power_mode.is_some());
        assert!(before.maximum_resident_set_size_bytes.is_some());
        assert!(before.lifetime_peak_physical_footprint_bytes.is_some());

        let after = ProcessTelemetrySnapshot::capture();
        let window = ProcessTelemetryWindow::between(&before, &after);
        assert!(window.available);
        assert_eq!(window.schema, PROCESS_TELEMETRY_SCHEMA);
        serde_json::to_string(&window).unwrap();
    }
}
