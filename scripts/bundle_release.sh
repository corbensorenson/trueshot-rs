#!/bin/bash
set -e

VERSION=${1:-"v6.0-dev"}

echo "Building TrueShot $VERSION Release..."

# 1. Build Frontend
echo "Building Dashboard..."
cd trueshot-dashboard
npm install --legacy-peer-deps
npm run build
cd ..

# 2. Build Backend
echo "Building Server (Release)..."
cd trueshot-server
cargo build --release
cd ..

# 3. Create Bundle Dir
DIST_DIR="dist/trueshot-$VERSION"
mkdir -p "$DIST_DIR/bin"
mkdir -p "$DIST_DIR/static"

# 4. Copy Artifacts
cp target/release/trueshot-server "$DIST_DIR/bin/"
cp -r trueshot-dashboard/dist/* "$DIST_DIR/static/"
cp config.toml "$DIST_DIR/" 2>/dev/null || :

# Create Runner Script
cat << 'EOF' > "$DIST_DIR/run.sh"
#!/bin/bash
cd "$(dirname "$0")"

# Start Server in background
./bin/trueshot-server &
SERVER_PID=$!

echo "TrueShot Server started (PID: $SERVER_PID)"
echo "Waiting for health check..."

sleep 2
# Open Browser
if [[ "$OSTYPE" == "darwin"* ]]; then
    open "http://localhost:3000"
elif command -v xdg-open &> /dev/null; then
    xdg-open "http://localhost:3000"
fi

echo "Press Ctrl+C to stop."
wait $SERVER_PID
EOF
chmod +x "$DIST_DIR/run.sh"

# 5. Zip
cd dist
tar -czf "trueshot-$VERSION-mac.tar.gz" "trueshot-$VERSION"
echo "Bundle created at dist/trueshot-$VERSION-mac.tar.gz"

# Optional signing
if [ -n "${TRUESHOT_SIGNING_KEY:-}" ]; then
  echo "Signing release..."
  ../scripts/sign_release.sh "trueshot-$VERSION-mac.tar.gz"
fi
