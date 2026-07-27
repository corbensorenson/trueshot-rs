# Multi-stage Dockerfile for TrueShot
# Provides both development and production builds

# ============================================================================
# Stage 1: Rust builder
# ============================================================================
FROM rust:1.90-bookworm AS rust-builder

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y \
    libudev-dev \
    libdbus-1-dev \
    libgphoto2-dev \
    libasound2-dev \
    libssl-dev \
    pkg-config \
    cmake \
    libvulkan-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy Cargo files first for better caching
COPY Cargo.toml Cargo.lock ./
COPY trueshot-core/Cargo.toml trueshot-core/
COPY trueshot-server/Cargo.toml trueshot-server/
COPY trueshot-cli/Cargo.toml trueshot-cli/
COPY trueshot-device-manager/Cargo.toml trueshot-device-manager/
COPY trueshot-calibration/Cargo.toml trueshot-calibration/

# Create dummy source files for dependency caching
RUN mkdir -p trueshot-core/src trueshot-server/src trueshot-cli/src \
    trueshot-device-manager/src trueshot-calibration/src && \
    echo "fn main() {}" > trueshot-server/src/main.rs && \
    echo "fn main() {}" > trueshot-cli/src/main.rs && \
    echo "pub fn lib() {}" > trueshot-core/src/lib.rs && \
    echo "pub fn lib() {}" > trueshot-device-manager/src/lib.rs && \
    echo "pub fn lib() {}" > trueshot-calibration/src/lib.rs

# Build dependencies only (cached)
RUN cargo build --release -p trueshot-server || true

# Copy actual source code
COPY . .

# Build for release
RUN cargo build --release -p trueshot-server

# ============================================================================
# Stage 2: Node.js frontend builder
# ============================================================================
FROM node:24-bookworm AS frontend-builder

WORKDIR /app/trueshot-dashboard

COPY trueshot-dashboard/package*.json ./
RUN npm ci --legacy-peer-deps

COPY trueshot-dashboard/ ./
RUN npm run build

# ============================================================================
# Stage 3: Production runtime
# ============================================================================
FROM debian:bookworm-slim AS runtime

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libasound2 \
    libdbus-1-3 \
    libgphoto2-6 \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy built artifacts
COPY --from=rust-builder /app/target/release/trueshot-server /app/
COPY --from=frontend-builder /app/trueshot-dashboard/dist /app/static/

# Copy configuration
COPY trueshot-server/config.toml /app/config.toml

# Create directories
RUN mkdir -p /app/logs /app/data /app/captures && \
    useradd -r -u 10001 -g root trueshot && \
    chown -R trueshot:root /app

# Environment
ENV RUST_LOG=info
ENV HOST=0.0.0.0
ENV PORT=3000

EXPOSE 3000

USER trueshot

CMD ["/app/trueshot-server"]

# ============================================================================
# Development target
# ============================================================================
FROM rust-builder AS development

RUN cargo install cargo-watch

WORKDIR /app
EXPOSE 3000

CMD ["cargo", "watch", "-x", "run -p trueshot-server"]
