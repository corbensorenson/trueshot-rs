#!/bin/bash
# TrueShot Production Build Script
# Builds optimized release binaries and frontend

set -e

echo "🚀 TrueShot Production Build"
echo "==========================="

# Colors
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 1. Build Rust backend
echo -e "${BLUE}[1/4] Building Rust backend...${NC}"
cargo build --release --workspace
echo -e "${GREEN}✓ Backend built successfully${NC}"

# 2. Build frontend
echo -e "${BLUE}[2/4] Building frontend...${NC}"
cd trueshot-dashboard
npm ci --legacy-peer-deps
npm run build
cd ..
echo -e "${GREEN}✓ Frontend built successfully${NC}"

# 3. Run tests
echo -e "${BLUE}[3/4] Running tests...${NC}"
cargo test --workspace --release
echo -e "${GREEN}✓ Tests passed${NC}"

# 4. Create distribution
echo -e "${BLUE}[4/4] Creating distribution...${NC}"
mkdir -p dist
cp target/release/trueshot-server dist/
cp -r trueshot-dashboard/dist dist/static
cp -r trueshot-server/config dist/
echo -e "${GREEN}✓ Distribution ready in ./dist${NC}"

echo ""
echo -e "${GREEN}🎉 Production build complete!${NC}"
echo ""
echo "To run:"
echo "  cd dist && ./trueshot-server"
echo ""
echo "Environment variables:"
echo "  TRUESHOT_ENV=production"
echo "  HOST=0.0.0.0"
echo "  PORT=3000"
