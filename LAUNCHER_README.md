# TrueShot Cross-Platform Launchers

TrueShot provides multiple launcher options for different platforms and user preferences. All launchers automatically detect and install dependencies, build the application if needed, and launch the GUI with GPU acceleration enabled by default.

## 🚀 Quick Start

### macOS
- **Double-click**: `TrueShot.app` (Native macOS app bundle)
- **Terminal**: `./TrueShot.py` or `./TrueShot.sh`

### Windows
- **Double-click**: `TrueShot.bat` (Windows batch file)
- **PowerShell**: `.\TrueShot.ps1`
- **Command Prompt**: `TrueShot.bat`
- **Python**: `python TrueShot.py`

### Linux
- **Terminal**: `./TrueShot.sh` or `./TrueShot.py`
- **Desktop**: Create desktop shortcut to `TrueShot.sh`

## 📋 Launcher Details

### TrueShot.app (macOS Native)
- **Platform**: macOS only
- **Type**: Native app bundle
- **Features**: 
  - Integrates with macOS Dock and Launchpad
  - Shows native macOS notifications
  - Automatic Rust installation detection
  - Builds project automatically if needed

### TrueShot.py (Universal)
- **Platform**: Windows, macOS, Linux
- **Requirements**: Python 3.6+
- **Features**:
  - Cross-platform compatibility
  - Smart platform detection
  - Native notifications on all platforms
  - Automatic dependency management
  - Detailed error reporting

### TrueShot.sh (Linux/Unix)
- **Platform**: Linux, macOS, Unix-like systems
- **Type**: Bash shell script
- **Features**:
  - Native Linux desktop integration
  - Support for multiple notification systems
  - Package manager installation hints
  - Lightweight and fast

### TrueShot.ps1 (Windows PowerShell)
- **Platform**: Windows
- **Requirements**: PowerShell 5.0+
- **Features**:
  - Windows 10+ toast notifications
  - Advanced error handling
  - Automatic PATH detection
  - Modern Windows integration

### TrueShot.bat (Windows Batch)
- **Platform**: Windows
- **Type**: Traditional batch file
- **Features**:
  - Compatible with all Windows versions
  - No PowerShell dependency
  - Simple and reliable
  - Fallback option for older systems

## ⚙️ GPU Acceleration

**GPU acceleration is now enabled by default** for optimal performance. The system will automatically:

1. **Detect available GPU backends**: Metal (macOS), CUDA (NVIDIA), OpenCL, wgpu
2. **Benchmark performance**: Test compute capabilities and memory bandwidth
3. **Select optimal backend**: Choose the best GPU for your system
4. **Fallback gracefully**: Use CPU processing if GPU is unavailable

### Disabling GPU Acceleration

If you need to disable GPU acceleration:

1. **GUI**: Uncheck "GPU Acceleration" in settings
2. **CLI**: Use `--no-gpu` flag
3. **Config**: Set `gpu_acceleration: false` in profile JSON

## 🔧 Automatic Setup

All launchers automatically handle:

- **Rust Installation Detection**: Find Rust in common locations
- **Dependency Building**: Compile TrueShot if binaries don't exist
- **Error Reporting**: Show detailed build errors with logs
- **Path Management**: Add Rust to PATH automatically
- **Platform Optimization**: Use platform-specific features

## 📁 File Structure

```
trueshot-rs/
├── TrueShot.app/          # macOS app bundle
├── TrueShot.py            # Universal Python launcher
├── TrueShot.sh            # Linux/Unix shell script
├── TrueShot.ps1           # Windows PowerShell script
├── TrueShot.bat           # Windows batch file
├── target/
│   ├── release/
│   │   └── trueshot-gui   # Release binary (auto-built)
│   └── debug/
│       └── trueshot-gui   # Debug binary (fallback)
└── ...
```

## 🚨 Troubleshooting

### Rust Not Found
- **Windows**: Install from https://rustup.rs/ or use `winget install Rustlang.Rustup`
- **macOS**: Install from https://rustup.rs/ or use `brew install rust`
- **Linux**: Use package manager or install from https://rustup.rs/

### Build Failures
- Check build logs (saved to temp directory)
- Ensure internet connection for dependency downloads
- Try `cargo clean` and rebuild
- Check disk space (builds require ~2GB)

### GPU Issues
- GPU acceleration will fallback to CPU automatically
- Check GPU drivers are up to date
- For NVIDIA: Ensure CUDA toolkit is installed
- For AMD: Ensure OpenCL drivers are installed

### Permission Errors
- **Linux/macOS**: Ensure scripts are executable (`chmod +x TrueShot.sh`)
- **Windows**: Run as Administrator if needed

## 🎯 Performance Tips

1. **Use Release Builds**: Launchers prefer release binaries for better performance
2. **Enable GPU**: Keep GPU acceleration enabled for large images
3. **Memory Limits**: Adjust memory limits in settings for your system
4. **Parallel Workers**: Let TrueShot auto-detect CPU cores

## 📞 Support

If you encounter issues:

1. Check the build logs (path shown in error dialogs)
2. Ensure Rust is properly installed
3. Try the universal Python launcher (`TrueShot.py`)
4. Report issues with system details and error logs
