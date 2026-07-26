#!/usr/bin/env python3
"""
TrueShot Python API Example

This example demonstrates how to use TrueShot from Python for
RAW image processing workflows.
"""

import os
import sys
import json
import time
from pathlib import Path

# Add the Python module to path (adjust as needed)
# sys.path.append('target/release')

try:
    import trueshot_py
    print("✅ TrueShot Python module imported successfully")
except ImportError as e:
    print(f"❌ Failed to import TrueShot Python module: {e}")
    print("Make sure to build the Python bindings first:")
    print("  cargo build -p trueshot-py")
    print("  # Or use maturin for development:")
    print("  pip install maturin")
    print("  cd trueshot-py && maturin develop")
    sys.exit(1)

def main():
    """Main example function"""
    print("🎯 TrueShot Python API Examples")
    print("================================")
    
    # Example 1: Basic processing with default configuration
    example_basic_processing()
    
    # Example 2: Custom configuration
    example_custom_configuration()
    
    # Example 3: Profile-based processing
    example_profile_processing()
    
    # Example 4: Image analysis
    example_image_analysis()
    
    # Example 5: Batch processing
    example_batch_processing()
    
    print("\n✅ All Python examples completed!")

def example_basic_processing():
    """Example 1: Basic processing with default settings"""
    print("\n📁 Example 1: Basic Processing")
    print("-------------------------------")
    
    try:
        # Create processor with default configuration
        processor = trueshot_py.PyTrueShot()
        
        input_dir = "realTest"
        output_dir = "output/python_basic"
        
        if Path(input_dir).exists():
            print(f"Processing {input_dir} with default settings...")
            start_time = time.time()
            
            processor.process_directory(input_dir, output_dir)
            
            elapsed = time.time() - start_time
            print(f"✅ Basic processing completed in {elapsed:.2f}s")
        else:
            print(f"⚠️  Input directory {input_dir} not found, skipping")
            
    except Exception as e:
        print(f"⚠️  Basic processing failed: {e}")

def example_custom_configuration():
    """Example 2: Processing with custom configuration"""
    print("\n⚙️  Example 2: Custom Configuration")
    print("-----------------------------------")
    
    try:
        # Create custom configuration
        config_dict = {
            "pre_cropping": True,
            "background_removal": False,
            "early_burst_collapse": True,
            "fusion_mode": "qif",
            "output_format": "tiff32",
            "tone_mapping": "reinhard",
            "parallel_workers": 4
        }
        
        print("Custom configuration:")
        for key, value in config_dict.items():
            print(f"  - {key}: {value}")
        
        # Create processor with custom configuration
        processor = trueshot_py.PyTrueShot.from_config(config_dict)
        
        input_dir = "realTest"
        output_dir = "output/python_custom"
        
        if Path(input_dir).exists():
            print(f"Processing {input_dir} with custom settings...")
            start_time = time.time()
            
            processor.process_directory(input_dir, output_dir)
            
            elapsed = time.time() - start_time
            print(f"✅ Custom processing completed in {elapsed:.2f}s")
        else:
            print(f"⚠️  Input directory {input_dir} not found, skipping")
            
    except Exception as e:
        print(f"⚠️  Custom processing failed: {e}")

def example_profile_processing():
    """Example 3: Processing using a profile file"""
    print("\n📋 Example 3: Profile-based Processing")
    print("--------------------------------------")
    
    try:
        profile_path = "profiles/default.json"
        
        if Path(profile_path).exists():
            print(f"Loading profile from: {profile_path}")
            
            # Create processor from profile
            processor = trueshot_py.PyTrueShot.from_profile(profile_path)
            
            input_dir = "realTest"
            output_dir = "output/python_profile"
            
            if Path(input_dir).exists():
                print(f"Processing {input_dir} with profile settings...")
                start_time = time.time()
                
                processor.process_directory(input_dir, output_dir)
                
                elapsed = time.time() - start_time
                print(f"✅ Profile processing completed in {elapsed:.2f}s")
            else:
                print(f"⚠️  Input directory {input_dir} not found, skipping")
        else:
            print(f"⚠️  Profile file {profile_path} not found, skipping")
            
    except Exception as e:
        print(f"⚠️  Profile processing failed: {e}")

def example_image_analysis():
    """Example 4: Analyze images without processing"""
    print("\n🔍 Example 4: Image Analysis")
    print("-----------------------------")
    
    try:
        input_dir = "realTest"
        
        if Path(input_dir).exists():
            print(f"Analyzing images in {input_dir}...")
            
            # Analyze images
            result = trueshot_py.analyze_images(input_dir)
            print(f"Analysis result: {result}")
        else:
            print(f"⚠️  Input directory {input_dir} not found, skipping")
            
    except Exception as e:
        print(f"⚠️  Image analysis failed: {e}")

def example_batch_processing():
    """Example 5: Batch processing multiple image sets"""
    print("\n📦 Example 5: Batch Processing")
    print("-------------------------------")
    
    try:
        # Create processor
        processor = trueshot_py.PyTrueShot()
        
        # Define batch jobs
        batch_jobs = [
            {
                "input": "realTest",
                "output": "output/python_batch/set1",
                "name": "Test Set 1"
            },
            # Add more sets as needed
        ]
        
        for job in batch_jobs:
            input_dir = job["input"]
            output_dir = job["output"]
            name = job["name"]
            
            if Path(input_dir).exists():
                print(f"Processing {name}: {input_dir} -> {output_dir}")
                start_time = time.time()
                
                processor.process_directory(input_dir, output_dir)
                
                elapsed = time.time() - start_time
                print(f"  ✅ {name} completed in {elapsed:.2f}s")
            else:
                print(f"  ⚠️  {name} directory {input_dir} not found, skipping")
                
    except Exception as e:
        print(f"⚠️  Batch processing failed: {e}")

def example_specific_images():
    """Example: Process specific image files"""
    print("\n🖼️  Example: Specific Image Processing")
    print("--------------------------------------")
    
    try:
        # Create processor
        processor = trueshot_py.PyTrueShot()
        
        # Define specific images to process
        image_files = [
            "realTest/_Z9Z5338.NEF",
            "realTest/_Z9Z5339.NEF",
            "realTest/_Z9Z5340.NEF",
        ]
        
        # Check if files exist
        existing_files = [f for f in image_files if Path(f).exists()]
        
        if existing_files:
            output_path = "output/python_specific/result.tif"
            
            print(f"Processing {len(existing_files)} specific images...")
            start_time = time.time()
            
            processor.process_image_set(existing_files, output_path)
            
            elapsed = time.time() - start_time
            print(f"✅ Specific image processing completed in {elapsed:.2f}s")
            print(f"Output saved to: {output_path}")
        else:
            print("⚠️  No specified image files found, skipping")
            
    except Exception as e:
        print(f"⚠️  Specific image processing failed: {e}")

def create_custom_profile():
    """Helper: Create and save a custom profile"""
    print("\n📝 Creating Custom Profile")
    print("---------------------------")
    
    try:
        # Create custom configuration
        config = trueshot_py.PyProcessingConfig()
        
        # Save to file
        profile_path = "profiles/python_custom.json"
        
        # Create directory if needed
        Path(profile_path).parent.mkdir(parents=True, exist_ok=True)
        
        config.save(profile_path)
        print(f"✅ Custom profile saved to: {profile_path}")
        
        # Load and display
        loaded_config = trueshot_py.PyProcessingConfig.from_file(profile_path)
        config_json = loaded_config.to_dict()
        print(f"Profile contents: {config_json}")
        
    except Exception as e:
        print(f"⚠️  Profile creation failed: {e}")

def performance_comparison():
    """Helper: Compare performance of different settings"""
    print("\n⏱️  Performance Comparison")
    print("--------------------------")
    
    configurations = [
        {"name": "Default", "config": {}},
        {"name": "High Quality", "config": {
            "fusion_mode": "qif",
            "output_format": "tiff32",
            "tone_mapping": "reinhard"
        }},
        {"name": "Fast", "config": {
            "early_burst_collapse": True,
            "background_removal": False,
            "output_format": "jpeg"
        }}
    ]
    
    input_dir = "realTest"
    
    if not Path(input_dir).exists():
        print(f"⚠️  Input directory {input_dir} not found, skipping performance test")
        return
    
    results = []
    
    for config_info in configurations:
        name = config_info["name"]
        config_dict = config_info["config"]
        
        try:
            print(f"Testing {name} configuration...")
            
            if config_dict:
                processor = trueshot_py.PyTrueShot.from_config(config_dict)
            else:
                processor = trueshot_py.PyTrueShot()
            
            output_dir = f"output/python_perf_{name.lower().replace(' ', '_')}"
            
            start_time = time.time()
            processor.process_directory(input_dir, output_dir)
            elapsed = time.time() - start_time
            
            results.append((name, elapsed))
            print(f"  ✅ {name}: {elapsed:.2f}s")
            
        except Exception as e:
            print(f"  ⚠️  {name} failed: {e}")
    
    if results:
        print("\nPerformance Summary:")
        for name, elapsed in sorted(results, key=lambda x: x[1]):
            print(f"  {name}: {elapsed:.2f}s")

if __name__ == "__main__":
    main()
    
    # Uncomment to run additional examples
    # example_specific_images()
    # create_custom_profile()
    # performance_comparison()
