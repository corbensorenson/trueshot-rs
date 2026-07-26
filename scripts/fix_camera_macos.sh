#!/bin/bash

# TrueShot Camera Fix for macOS
# This script resolves the PTPCamera daemon conflict that prevents gphoto2 from accessing cameras

echo "🔧 TrueShot Camera Fix for macOS"
echo "================================"

# Check if ptpcamerad is running
PTPCAMERA_PID=$(pgrep ptpcamerad)

if [ -n "$PTPCAMERA_PID" ]; then
    echo "📷 Found PTPCamera daemon running (PID: $PTPCAMERA_PID)"
    echo "   This daemon prevents gphoto2 from accessing your camera."
    echo ""
    echo "🛠️  Attempting to stop PTPCamera daemon..."
    
    # Try to kill the daemon
    if sudo kill $PTPCAMERA_PID; then
        echo "✅ Successfully stopped PTPCamera daemon"
        echo ""
        echo "🎯 Your camera should now be accessible to TrueShot!"
        echo "   You can now run: cargo run --bin test_hardware --features tethering"
        echo "   Or launch the TrueShot GUI normally."
        echo ""
        echo "⚠️  Note: The daemon may restart automatically when you reconnect the camera."
        echo "   If camera access fails again, run this script again."
    else
        echo "❌ Failed to stop PTPCamera daemon"
        echo "   You may need to run TrueShot with sudo instead:"
        echo "   sudo cargo run --bin test_hardware --features tethering"
        echo "   sudo ./target/release/trueshot-gui"
    fi
else
    echo "✅ No PTPCamera daemon found running"
    echo "   Your camera should be accessible to TrueShot."
    echo ""
    echo "🔍 If you're still having camera connection issues:"
    echo "   1. Make sure your camera is connected via USB"
    echo "   2. Make sure your camera is in the correct mode for tethering"
    echo "   3. Try disconnecting and reconnecting the camera"
    echo "   4. Run: gphoto2 --auto-detect"
fi

echo ""
echo "🚀 Ready to test camera connection!"
