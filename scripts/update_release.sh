#!/bin/bash
set -euo pipefail

ARCHIVE_PATH="${1:-}"
SIG_PATH="${2:-}"
DEST_DIR="${3:-/opt/trueshot}"

if [ -z "$ARCHIVE_PATH" ] || [ -z "$SIG_PATH" ]; then
  echo "Usage: $0 <archive_path> <signature_path> [dest_dir]"
  exit 1
fi

if [ ! -f "$ARCHIVE_PATH" ]; then
  echo "Archive not found: $ARCHIVE_PATH"
  exit 1
fi

if [ ! -f "$SIG_PATH" ]; then
  echo "Signature not found: $SIG_PATH"
  exit 1
fi

if [ -z "${TRUESHOT_UPDATE_PUBKEY:-}" ]; then
  echo "TRUESHOT_UPDATE_PUBKEY is required (path to public key)."
  exit 1
fi

if [ ! -f "$TRUESHOT_UPDATE_PUBKEY" ]; then
  echo "Public key not found: $TRUESHOT_UPDATE_PUBKEY"
  exit 1
fi

echo "Verifying signature..."
openssl dgst -sha256 -verify "$TRUESHOT_UPDATE_PUBKEY" -signature "$SIG_PATH" "$ARCHIVE_PATH"

timestamp=$(date +"%Y%m%d_%H%M%S")
release_dir="$DEST_DIR/releases/$timestamp"
current_link="$DEST_DIR/current"

mkdir -p "$release_dir"
tar -xzf "$ARCHIVE_PATH" -C "$release_dir"

if [ -L "$current_link" ] || [ -e "$current_link" ]; then
  rm -rf "$current_link"
fi
ln -s "$release_dir" "$current_link"

if command -v systemctl >/dev/null 2>&1; then
  echo "Restarting systemd service..."
  systemctl restart trueshot.service || true
elif command -v launchctl >/dev/null 2>&1; then
  echo "Restarting launchd service..."
  launchctl kickstart -k gui/$(id -u)/com.augment.trueshot || true
fi

echo "Update applied. Current release: $release_dir"
