#!/bin/bash
set -euo pipefail

INSTALL_ROOT="${1:-/opt/trueshot}"
CURRENT_BIN="$INSTALL_ROOT/current/bin/trueshot-server"

if [ ! -x "$CURRENT_BIN" ]; then
  echo "Server binary not found at $CURRENT_BIN"
  exit 1
fi

if [[ "$OSTYPE" == "darwin"* ]]; then
  plist_path="$HOME/Library/LaunchAgents/com.augment.trueshot.plist"
  cat > "$plist_path" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.augment.trueshot</string>
  <key>ProgramArguments</key>
  <array>
    <string>$CURRENT_BIN</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>$INSTALL_ROOT/logs/trueshot.out</string>
  <key>StandardErrorPath</key>
  <string>$INSTALL_ROOT/logs/trueshot.err</string>
</dict>
</plist>
EOF
  mkdir -p "$INSTALL_ROOT/logs"
  launchctl unload "$plist_path" >/dev/null 2>&1 || true
  launchctl load "$plist_path"
  echo "LaunchAgent installed at $plist_path"
  exit 0
fi

if command -v systemctl >/dev/null 2>&1; then
  unit_path="/etc/systemd/system/trueshot.service"
  sudo mkdir -p "$INSTALL_ROOT/logs"
  sudo tee "$unit_path" >/dev/null <<EOF
[Unit]
Description=TrueShot Server
After=network.target

[Service]
Type=simple
ExecStart=$CURRENT_BIN
Restart=always
WorkingDirectory=$INSTALL_ROOT/current
StandardOutput=append:$INSTALL_ROOT/logs/trueshot.out
StandardError=append:$INSTALL_ROOT/logs/trueshot.err

[Install]
WantedBy=multi-user.target
EOF
  sudo systemctl daemon-reload
  sudo systemctl enable trueshot.service
  sudo systemctl restart trueshot.service
  echo "systemd service installed at $unit_path"
  exit 0
fi

echo "No supported service manager found."
exit 1
