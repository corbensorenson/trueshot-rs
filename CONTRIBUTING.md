# Contributing to TrueShot

Welcome to the TrueShot project! We're building the future of hybrid photogrammetry.

## Development Environment Setup

### Prerequisites
- **Rust**: Latest stable (`rustup update`)
- **Node.js**: v18+ (for Dashboard)
- **System Deps**: `libudev`, `pkg-config`, `cmake` (Linux); Xcode CLI Tools (macOS).
- **COLMAP**: Must be in your PATH for reconstruction features.

### Quick Start

1.  **Clone & Build**:
    ```bash
    git clone https://github.com/augment-tech/trueshot-rs
    cd trueshot-rs
    cargo build
    ```

2.  **Run Development Server**:
    This will start the API server and the frontend dev server.
    ```bash
    # Terminal 1
    cd trueshot-server
    cargo run

    # Terminal 2
    cd trueshot-dashboard
    npm install
    npm run dev
    ```

3.  **Mock Hardware**:
    The system defaults to Mock Mode if no physical camera/turntable is detected.
    - **Camera**: Generates synthetic noise patterns.
    - **Turntable**: Simulates rotation with 2s delay.

### Coding Standards

- **Formatting**: Run `cargo fmt` before committing.
- **Linting**: We enforce `cargo clippy -- -D warnings`.
- **Tests**: Add unit tests for new logic in `trueshot-core`. Run `cargo test` to verify.
- **Licensing**: Ensure the Apache 2.0 / MIT header is present in new files.

### Project Structure

- `trueshot-core`: Business logic (Vision, Math, Hardware Traits).
- `trueshot-server`: Axum API & App State.
- `trueshot-dashboard`: React/Vite Frontend.
- `trueshot-camera`: `nokhwa` & `gphoto2` wrappers.
- `trueshot-turntable`: BLE & Serial drivers.

### Submitting PRs

1.  Fork the repo.
2.  Create a feature branch (`feat/my-feature`).
3.  Add tests.
4.  Ensure CI passes.
5.  Open a Pull Request.

Thank you for contributing!
