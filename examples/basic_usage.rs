//! Basic usage example for TrueShot
//!
//! This example demonstrates how to use TrueShot to process a directory
//! of RAW images with different configuration options.

use anyhow::Result;
use std::path::PathBuf;
use trueshot_core::{TrueShot, ProcessingConfig};

fn main() -> Result<()> {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    println!("🎯 TrueShot Basic Usage Example");
    println!("================================");

    // Example 1: Default configuration
    example_default_processing()?;

    // Example 2: Custom configuration
    example_custom_processing()?;

    // Example 3: Profile-based processing
    example_profile_processing()?;

    // Example 4: Batch processing
    example_batch_processing()?;

    println!("\n✅ All examples completed successfully!");
    Ok(())
}

/// Example 1: Process images with default configuration
fn example_default_processing() -> Result<()> {
    println!("\n📁 Example 1: Default Processing");
    println!("---------------------------------");

    let config = ProcessingConfig::default();
    let processor = TrueShot::new(config);

    let input_dir = PathBuf::from("realTest");
    let output_dir = PathBuf::from("output/default");

    if input_dir.exists() {
        println!("Processing {} with default settings...", input_dir.display());
        
        match processor.process_directory(&input_dir, &output_dir) {
            Ok(()) => println!("✅ Default processing completed successfully"),
            Err(e) => println!("⚠️  Default processing failed: {}", e),
        }
    } else {
        println!("⚠️  Input directory {} not found, skipping", input_dir.display());
    }

    Ok(())
}

/// Example 2: Process images with custom configuration
fn example_custom_processing() -> Result<()> {
    println!("\n⚙️  Example 2: Custom Configuration");
    println!("-----------------------------------");

    let mut config = ProcessingConfig::default();
    
    // Customize processing options
    config.pre_cropping = true;
    config.background_removal = true;
    config.early_burst_collapse = true;
    config.fusion_mode = trueshot_core::config::FusionMode::Qif;
    config.output_format = trueshot_core::config::OutputFormat::Tiff32;
    config.tone_mapping = trueshot_core::config::ToneMappingMethod::Reinhard;
    config.parallel_workers = 4;

    println!("Custom configuration:");
    println!("  - Pre-cropping: {}", config.pre_cropping);
    println!("  - Background removal: {}", config.background_removal);
    println!("  - Fusion mode: {:?}", config.fusion_mode);
    println!("  - Output format: {:?}", config.output_format);
    println!("  - Tone mapping: {:?}", config.tone_mapping);

    let processor = TrueShot::new(config);

    let input_dir = PathBuf::from("realTest");
    let output_dir = PathBuf::from("output/custom");

    if input_dir.exists() {
        println!("Processing {} with custom settings...", input_dir.display());
        
        match processor.process_directory(&input_dir, &output_dir) {
            Ok(()) => println!("✅ Custom processing completed successfully"),
            Err(e) => println!("⚠️  Custom processing failed: {}", e),
        }
    } else {
        println!("⚠️  Input directory {} not found, skipping", input_dir.display());
    }

    Ok(())
}

/// Example 3: Process images using a profile file
fn example_profile_processing() -> Result<()> {
    println!("\n📋 Example 3: Profile-based Processing");
    println!("--------------------------------------");

    let profile_path = PathBuf::from("profiles/default.json");
    
    if profile_path.exists() {
        println!("Loading profile from: {}", profile_path.display());
        
        match TrueShot::from_profile(&profile_path) {
            Ok(processor) => {
                let input_dir = PathBuf::from("realTest");
                let output_dir = PathBuf::from("output/profile");

                if input_dir.exists() {
                    println!("Processing {} with profile settings...", input_dir.display());
                    
                    match processor.process_directory(&input_dir, &output_dir) {
                        Ok(()) => println!("✅ Profile processing completed successfully"),
                        Err(e) => println!("⚠️  Profile processing failed: {}", e),
                    }
                } else {
                    println!("⚠️  Input directory {} not found, skipping", input_dir.display());
                }
            }
            Err(e) => println!("⚠️  Failed to load profile: {}", e),
        }
    } else {
        println!("⚠️  Profile file {} not found, skipping", profile_path.display());
    }

    Ok(())
}

/// Example 4: Batch processing multiple directories
fn example_batch_processing() -> Result<()> {
    println!("\n📦 Example 4: Batch Processing");
    println!("-------------------------------");

    let config = ProcessingConfig::default();
    let processor = TrueShot::new(config);

    // List of input directories to process
    let batch_dirs = vec![
        ("realTest", "output/batch/set1"),
        // Add more directories as needed
        // ("another_test_dir", "output/batch/set2"),
    ];

    for (input_dir, output_dir) in batch_dirs {
        let input_path = PathBuf::from(input_dir);
        let output_path = PathBuf::from(output_dir);

        if input_path.exists() {
            println!("Processing batch: {} -> {}", input_dir, output_dir);
            
            match processor.process_directory(&input_path, &output_path) {
                Ok(()) => println!("  ✅ Batch {} completed", input_dir),
                Err(e) => println!("  ⚠️  Batch {} failed: {}", input_dir, e),
            }
        } else {
            println!("  ⚠️  Batch directory {} not found, skipping", input_dir);
        }
    }

    Ok(())
}

/// Example helper: Create a custom profile and save it
#[allow(dead_code)]
fn create_custom_profile() -> Result<()> {
    println!("\n📝 Creating Custom Profile");
    println!("---------------------------");

    let mut config = ProcessingConfig::default();
    
    // High-quality settings
    config.pre_cropping = true;
    config.background_removal = false;
    config.early_burst_collapse = true;
    config.fusion_mode = trueshot_core::config::FusionMode::Qif;
    config.output_format = trueshot_core::config::OutputFormat::Tiff32;
    config.tone_mapping = trueshot_core::config::ToneMappingMethod::Reinhard;
    config.alignment_method = trueshot_core::config::AlignmentMethod::PhaseCorrelation;
    config.quality_threshold = 0.9;
    config.parallel_workers = 0; // Auto-detect

    let profile_path = PathBuf::from("profiles/high_quality.json");
    
    // Create profiles directory if it doesn't exist
    if let Some(parent) = profile_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    config.save_to_file(&profile_path)?;
    println!("✅ Custom profile saved to: {}", profile_path.display());

    Ok(())
}

/// Example helper: Analyze images without processing
#[allow(dead_code)]
fn analyze_images_example() -> Result<()> {
    println!("\n🔍 Image Analysis Example");
    println!("--------------------------");

    let input_dir = PathBuf::from("realTest");
    
    if input_dir.exists() {
        let config = ProcessingConfig::default();
        
        match trueshot_core::grouping::group_images(&input_dir, &config) {
            Ok(grouping_result) => {
                println!("Analysis Results:");
                println!("  Total images: {}", grouping_result.total_images());
                println!("  Exposure groups: {}", grouping_result.exposure_groups.len());
                
                for (i, exposure_group) in grouping_result.exposure_groups.iter().enumerate() {
                    println!("  Exposure group {}: {} focus groups", 
                             i + 1, exposure_group.focus_groups.len());
                    
                    for (j, focus_group) in exposure_group.focus_groups.iter().enumerate() {
                        println!("    Focus group {}: {} images (focus: {:?})", 
                                 j + 1, 
                                 focus_group.images.len(),
                                 focus_group.focus_distance);
                    }
                }
                
                if let Some(ref_image) = &grouping_result.reference_image {
                    println!("  Reference image: {}", ref_image.display());
                }
            }
            Err(e) => println!("⚠️  Analysis failed: {}", e),
        }
    } else {
        println!("⚠️  Input directory {} not found", input_dir.display());
    }

    Ok(())
}

/// Example helper: Performance monitoring
#[allow(dead_code)]
fn performance_monitoring_example() -> Result<()> {
    println!("\n⏱️  Performance Monitoring Example");
    println!("-----------------------------------");

    let start_time = std::time::Instant::now();
    
    let config = ProcessingConfig::default();
    let processor = TrueShot::new(config);

    let input_dir = PathBuf::from("realTest");
    let output_dir = PathBuf::from("output/performance_test");

    if input_dir.exists() {
        println!("Starting performance test...");
        
        match processor.process_directory(&input_dir, &output_dir) {
            Ok(()) => {
                let elapsed = start_time.elapsed();
                println!("✅ Processing completed in {:.2}s", elapsed.as_secs_f64());
                
                // Additional performance metrics could be collected here
                println!("Performance metrics:");
                println!("  - Total time: {:.2}s", elapsed.as_secs_f64());
                println!("  - Memory usage: {} MB", get_memory_usage_mb());
            }
            Err(e) => println!("⚠️  Performance test failed: {}", e),
        }
    } else {
        println!("⚠️  Input directory {} not found", input_dir.display());
    }

    Ok(())
}

/// Get current memory usage in MB (simplified)
fn get_memory_usage_mb() -> u64 {
    // This is a placeholder - real implementation would use system APIs
    // or process monitoring libraries
    0
}
