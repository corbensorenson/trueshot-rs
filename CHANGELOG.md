# Changelog

All notable changes to TrueShot will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [6.8.0] - 2026-01-15

### Added
- **Smart Scan Wizard** - AI-powered object detection and scan planning
- **OpenAPI Documentation** - Available at `/api/docs`
- **Environment Configuration** - `.env` support with `VITE_API_BASE`
- **Docker Support** - Multi-stage Dockerfile and docker-compose
- **GLTF/GLB Export** - Full binary export with proper spec compliance
- **Error Boundaries** - React error handling for crash prevention
- **Loading States** - Skeleton loaders and spinners
- **TrueShot Branded Favicon**

### Changed
- Updated server version to 6.8.0
- Whitepaper updated to reflect current architecture
- README updated with correct build commands
- All hardcoded URLs replaced with environment variables

### Fixed
- Camera intrinsics handling for multi-camera setups
- Duplicate struct definitions in scan API
- WebSocket connection URL configuration

## [5.4.0] - 2025-12-30

### Added
- Initial ScanWizard implementation
- COLMAP pipeline integration
- Object tracking with CSRT
- Coverage heatmap visualization

### Changed
- Migrated from GUI to web dashboard
- Separated frontend (React) from backend (Actix)

## [5.0.0] - 2025-12-15

### Added
- Hybrid photogrammetry pipeline
- 3D Gaussian Splatting support
- Turntable control integration
- Hardware abstraction layer

### Changed
- Complete architecture redesign
- "Capture to Reality" philosophy
