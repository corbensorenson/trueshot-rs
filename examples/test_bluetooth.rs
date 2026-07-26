// Standalone Bluetooth LE scanner and turntable controller
// Run with: cargo run --bin test_bluetooth --features tethering

use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager};
use std::error::Error;
use std::time::Duration;
use tokio::time;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    println!("🔍 Bluetooth LE Scanner - Searching for turntable...\n");

    // Get the Bluetooth adapter
    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    
    if adapters.is_empty() {
        eprintln!("❌ No Bluetooth adapters found!");
        return Ok(());
    }

    let adapter = &adapters[0];
    println!("✅ Using Bluetooth adapter: {:?}\n", adapter.adapter_info().await?);

    // Start scanning
    println!("🔍 Scanning for BLE devices for 10 seconds...\n");
    adapter.start_scan(ScanFilter::default()).await?;
    time::sleep(Duration::from_secs(10)).await;
    adapter.stop_scan().await?;

    // Get all discovered peripherals
    let peripherals = adapter.peripherals().await?;
    
    if peripherals.is_empty() {
        println!("❌ No BLE devices found!");
        return Ok(());
    }

    println!("📱 Found {} BLE device(s):\n", peripherals.len());
    println!("{:<40} {:<20} {:<10}", "NAME", "ADDRESS", "RSSI");
    println!("{}", "=".repeat(70));

    let mut turntable_candidates = Vec::new();

    for peripheral in peripherals.iter() {
        let properties = peripheral.properties().await?;
        let local_name = properties
            .as_ref()
            .and_then(|p| p.local_name.clone())
            .unwrap_or_else(|| "Unknown".to_string());

        let address = properties
            .as_ref()
            .map(|p| p.address.to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let rssi_value = properties
            .as_ref()
            .and_then(|p| p.rssi)
            .unwrap_or(-999);

        let rssi_str = if rssi_value == -999 {
            "N/A".to_string()
        } else {
            rssi_value.to_string()
        };

        println!("{:<40} {:<20} {:<10}", local_name, address, rssi_str);

        // Check if this might be a turntable
        let name_lower = local_name.to_lowercase();
        if name_lower.contains("foldio")
            || name_lower.contains("turntable")
            || name_lower.contains("360")
            || name_lower.contains("orangemonkie") {
            turntable_candidates.push((peripheral.clone(), local_name.clone(), address.clone(), rssi_value));
        }
    }

    println!("\n{}", "=".repeat(70));

    if turntable_candidates.is_empty() {
        println!("\n⚠️  No turntable devices found automatically.");
        println!("Please select a device from the list above to test connection.");
        return Ok(());
    }

    // Sort by RSSI (strongest signal first = highest/least negative number)
    turntable_candidates.sort_by(|a, b| b.3.cmp(&a.3));

    println!("\n🎯 Found {} potential turntable device(s) (sorted by signal strength):", turntable_candidates.len());
    for (i, (_, name, addr, rssi)) in turntable_candidates.iter().enumerate() {
        let rssi_str = if *rssi == -999 { "N/A".to_string() } else { rssi.to_string() };
        println!("  {}. {} ({}) - RSSI: {}", i + 1, name, addr, rssi_str);
    }

    // Try to connect to each candidate until one succeeds
    let mut connected = false;

    for (i, (peripheral, name, addr, rssi)) in turntable_candidates.iter().enumerate() {
        let rssi_str = if *rssi == -999 { "N/A".to_string() } else { rssi.to_string() };
        println!("\n🔌 Attempt {}/{}: Connecting to {} (RSSI: {})...",
                 i + 1, turntable_candidates.len(), name, rssi_str);

        // Try to connect with timeout
        match tokio::time::timeout(
            Duration::from_secs(10),
            peripheral.connect()
        ).await {
            Ok(Ok(())) => {
                println!("✅ Successfully connected to {}!", name);
                connected = true;

                // Discover services
                println!("🔍 Discovering services...");
                match tokio::time::timeout(
                    Duration::from_secs(10),
                    peripheral.discover_services()
                ).await {
                    Ok(Ok(())) => {
                        println!("✅ Services discovered!");

                        let services = peripheral.services();
                        println!("📋 Found {} service(s):", services.len());

                        for service in services.iter() {
                            println!("  Service UUID: {}", service.uuid);
                            for characteristic in service.characteristics.iter() {
                                println!("    Characteristic UUID: {}", characteristic.uuid);
                                println!("      Properties: {:?}", characteristic.properties);
                            }
                        }

                        // Disconnect
                        println!("\n🔌 Disconnecting...");
                        peripheral.disconnect().await?;
                        println!("✅ Disconnected successfully");
                    }
                    Ok(Err(e)) => {
                        println!("❌ Service discovery failed: {}", e);
                    }
                    Err(_) => {
                        println!("⏱️  Service discovery timeout (10s)");
                    }
                }

                break; // Successfully connected, stop trying other devices
            }
            Ok(Err(e)) => {
                println!("❌ Connection failed: {}", e);
                if i < turntable_candidates.len() - 1 {
                    println!("   Trying next device...");
                }
            }
            Err(_) => {
                println!("⏱️  Connection timeout (10s)");
                if i < turntable_candidates.len() - 1 {
                    println!("   Trying next device...");
                }
            }
        }
    }

    if !connected {
        println!("\n❌ Failed to connect to any turntable device!");
        println!("\n💡 Troubleshooting:");
        println!("   1. Make sure the turntable is in pairing mode (usually blinking LED)");
        println!("   2. Try power cycling the turntable (turn off and on)");
        println!("   3. Move the turntable closer to your computer");
        println!("   4. Check if another device is already connected to it");
        println!("   5. Try forgetting the device in Bluetooth settings and re-pairing");
    }

    Ok(())
}

