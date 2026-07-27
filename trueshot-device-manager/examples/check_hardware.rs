use trueshot_device_manager::CameraManager;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    println!("=== TrueShot Hardware Verification ===");

    // 1. Check Serial Ports (Turntable)
    println!("\n[1/3] Checking Serial Ports (Turntable Connection)...");
    match serialport::available_ports() {
        Ok(ports) => {
            if ports.is_empty() {
                println!("    No serial ports found.");
            } else {
                for p in ports {
                    println!("    Found Port: {}", p.port_name);
                }
            }
        }
        Err(e) => println!("    Error listing serial ports: {}", e),
    }

    // 2. Check Cameras (Nikon Z9 + Webcams)
    println!("\n[2/3] Checking Cameras (via CameraManager)...");
    println!("      Note: Ensure Nikon Z9 is connected via USB and turned ON.");

    let mut manager = CameraManager::new();
    // Reconcile performs discovery
    match manager.reconcile_cameras(false).await {
        Ok(report) => {
            println!("    Discovery Complete.");
            println!("    New Devices Added: {:?}", report.added);

            println!("\n    --- Connected Cameras ---");
            for cam in &manager.cameras {
                let id = cam.id();
                // We can't easily get the friendly name back from Box<dyn Camera> unless cast or registry lookup.
                // But we can check registry.
                if let Some(profile) = manager.registry.get_profile(&id) {
                    println!(
                        "    ID: {:<20} | Name: {:<20} | Role: {:?}",
                        id, profile.name, profile.role
                    );
                } else {
                    println!("    ID: {:<20} | Name: Unknown", id);
                }

                // Extra check for Nikon
                if id.to_lowercase().contains("nikon") || id.to_lowercase().contains("dslr") {
                    println!("    *** VALIDATED: High-End Camera Detected! ***");
                }
            }
        }
        Err(e) => println!("    Error discovering cameras: {}", e),
    }

    println!("[3/3] Checking Bluetooth (Foldio360)...");
    println!("      Scanning for 5 seconds...");
    // Create a temporary Foldio360 instance just to scan?
    // Or simpler: Use btleplug directly.
    // Actually, let's use the Turntable trait or implementation if possible.
    // But Foldio360::connect scans internaly?
    // Let's just list devices using btleplug code for verification.

    use btleplug::api::{Central, Manager as _, Peripheral, ScanFilter};
    use btleplug::platform::Manager;
    use std::time::Duration;

    // Use a timeout
    let manager_result = Manager::new().await;
    match manager_result {
        Ok(manager) => {
            if let Ok(mut adapters) = manager.adapters().await {
                if let Some(central) = adapters.pop() {
                    let _ = central.start_scan(ScanFilter::default()).await;
                    tokio::time::sleep(Duration::from_secs(5)).await;

                    let peripherals = central.peripherals().await.unwrap_or_default();
                    if peripherals.is_empty() {
                        println!("      No Bluetooth devices found.");
                    } else {
                        for p in peripherals {
                            let props = p.properties().await.unwrap_or_default();
                            let local_name = props
                                .clone()
                                .and_then(|pr| pr.local_name)
                                .unwrap_or_else(|| "Unknown".to_string());
                            if local_name.to_lowercase().contains("foldio") {
                                println!("      ✅ Found Turntable: {}", local_name);
                                println!("      ✅ Found Turntable: {}", local_name);
                            } else {
                                println!(
                                    "      Found Other Device: {} (Address: {})",
                                    local_name,
                                    p.address()
                                );
                                if let Some(services) = &props.map(|p| p.services) {
                                    println!("          Services: {:?}", services);
                                }
                            }
                        }
                    }
                } else {
                    println!("      No Bluetooth adapter found.");
                }
            } else {
                println!("      Failed to get adapters.");
            }
        }
        Err(e) => println!("      Bluetooth Manager Error: {}", e),
    }

    println!("\n=== Verification Complete ===");
    Ok(())
}
