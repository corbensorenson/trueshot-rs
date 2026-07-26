use notify_rust::Notification;

pub fn notify_scan_complete(project_name: &str) {
    #[cfg(target_os = "macos")]
    {
        // MacOS often blocks notifications from unsigned binaries.
        // We fallback to simple sound or assume user enabled it.
        Notification::new()
            .summary("Scan Complete")
            .body(&format!("Project '{}' is ready for processing.", project_name))
            .sound_name("Glass")
            .show()
            .ok();
    }

    #[cfg(not(target_os = "macos"))]
    {
        Notification::new()
            .summary("Scan Complete")
            .body(&format!("Project '{}' is ready.", project_name))
            .show()
            .ok();
    }
}
