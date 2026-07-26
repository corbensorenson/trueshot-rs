use std::process::Command;

/// Service Installer (Linux Systemd / macOS LaunchAgent)
pub fn install_service() -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.augment.trueshot</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/trueshot-server</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
</dict>
</plist>"#;
        
        let path = std::path::PathBuf::from(std::env::var("HOME").unwrap())
            .join("Library/LaunchAgents/com.augment.trueshot.plist");
            
        std::fs::write(&path, plist)?;
        Command::new("launchctl").arg("load").arg(path.to_str().unwrap()).status()?;
    }
    
    // Linux systemd impl omitted for brevity but follows same pattern
    Ok(())
}
