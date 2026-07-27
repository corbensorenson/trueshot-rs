# TrueShot Feature Catalog

> **Last Updated:** January 16, 2026  
> **Version:** 2.0.0  
> **Purpose:** Complete feature inventory for development tracking and investor/customer pitches

---

## Status Legend

| Status | Meaning |
|--------|---------|
| 🟢 **Complete** | Production-ready, tested, documented |
| 🟡 **In Progress** | Actively being developed |
| 🔴 **Planned** | Designed but not implemented |
| ⚪ **Future** | On roadmap, not yet designed |

---

## 1. Core Scanning Features

### 1.1 Hybrid 3DGS + Photogrammetry Reconstruction

| Attribute | Value |
|-----------|-------|
| **Status** | 🟡 In Progress |
| **Priority** | P0 (Critical) |
| **Category** | Core |

**Description:**
TrueShot's flagship feature - combines the speed and visual quality of 3D Gaussian Splatting with the geometric accuracy of traditional photogrammetry. Users get the best of both worlds: real-time preview during scanning (3DGS) and accurate exportable meshes (photogrammetry).

**Why State-of-the-Art:**
- **Only solution** offering true hybrid output in a single pipeline
- Real-time 3DGS preview during capture (competitors require post-processing)
- Automatic mesh extraction via GS2Mesh (ECCV 2024 approach)
- Sub-pixel bundle adjustment for maximum accuracy

**Technical Implementation:**
- `trueshot-core/src/reconstruction/hybrid.rs` - HybridPipeline
- `trueshot-core/src/gaussian_splatting/` - 3DGS training
- `trueshot-core/src/gaussian_splatting/gs2mesh.rs` - Mesh extraction

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| Polycam | ❌ (3DGS only or mesh only) | Single unified pipeline |
| RealityCapture | ❌ (mesh only) | Real-time preview |
| Luma AI | ❌ (3DGS only) | Exportable accurate mesh |

---

### 1.2 Background Subtraction / Pixel Collapse

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P0 (Critical) |
| **Category** | Core |

**Description:**
Before placing an object, users capture the empty turntable. TrueShot automatically subtracts this background from all subsequent frames, isolating the object perfectly. The "Pixel Collapse" visualizer shows exactly what's being captured.

**Why State-of-the-Art:**
- Eliminates need for green screen or manual masking
- Works with any background, any lighting
- Real-time visualization during capture
- Handles reflections and shadows intelligently

**Technical Implementation:**
- `trueshot-core/src/matting.rs` - Background subtraction
- `trueshot-dashboard/src/components/PixelCollapseVisualizer.tsx`

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| Polycam | ❌ | Automatic, no setup required |
| RealityCapture | ❌ (manual masking) | Real-time, integrated |
| Meshroom | ❌ (requires manual masking) | Zero-click operation |

---

### 1.3 AI Object Analysis

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P1 (High) |
| **Category** | AI |

**Description:**
Before scanning, TrueShot analyzes the placed object to determine optimal capture parameters: object size/category, complexity level (simple/moderate/complex), surface type (matte/glossy/transparent/mixed), and whether underside capture is needed.

**Why State-of-the-Art:**
- Automatic quality optimization - users don't need to understand photogrammetry
- Predicts challenging areas (reflections, thin structures)
- Recommends scanning approach based on object properties
- Calculates optimal photo count and angles

**Technical Implementation:**
- `trueshot-dashboard/src/components/ScanWizard.tsx` - runAnalysis()
- Backend AI endpoint:
  - `POST /api/wizard/analyze`
  - Auth: admin/authorized session (cookie or bearer token)
  - Inputs: none (uses latest live preview + optional background capture)
  - Outputs: `ObjectAnalysis` (size category, complexity score, surface type, capture guidance)
  - Failure modes: `500` if preview capture fails; `401` on missing auth

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| Polycam | Partial (size detection) | Full complexity analysis |
| RealityCapture | ❌ | Automated optimization |
| Scaniverse | ❌ | Surface type detection |

---

### 1.4 Guided Capture Workflow

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P1 (High) |
| **Category** | Core |

**Description:**
Step-by-step scanning wizard guides users through optimal capture positions. Shows exactly where to position cameras, when to rotate turntable, and when to reorient object. Progress bar and visual indicators ensure complete coverage.

**Why State-of-the-Art:**
- Cannot miss critical angles - guided path ensures completeness
- Adapts to detected object complexity
- Shows real-time capture quality feedback
- Supports multi-camera orchestration

**Technical Implementation:**
- `trueshot-dashboard/src/components/ScanWizard.tsx` - Full wizard
- `ScanPlan` and `ScanStep` types

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| Polycam | Partial (object mode) | Multi-elevation guidance |
| RealityCapture | ❌ (manual only) | Intelligent path planning |
| Matterport | ✅ (rooms) | Object-specific optimization |

---

### 1.5 Multi-Camera Synchronization

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P1 (High) |
| **Category** | Hardware |

**Description:**
TrueShot supports simultaneous capture from multiple cameras (DSLR, webcam, Insta360). Cameras are synchronized for precise multi-view acquisition. Essential for 4DGS dynamic capture.

**Why State-of-the-Art:**
- Sub-millisecond sync for dynamic scenes (via hardware trigger)
- Automatic exposure matching across cameras
- Supports heterogeneous camera types
- Essential for 4D Gaussian Splatting (unique capability)

**Technical Implementation:**
- `trueshot-core/src/camera/` - Camera abstraction
- `trueshot-core/src/reconstruction/hybrid.rs` - MultiCameraRig

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| Polycam | ❌ (single camera) | Multi-camera architecture |
| RealityCapture | Partial (import only) | Real-time sync |
| All Others | ❌ | 4DGS-ready infrastructure |

---

### 1.6 SD Card High-Resolution Import

| Attribute | Value |
|-----------|-------|
| **Status** | 🟡 In Progress |
| **Priority** | P1 (High) |
| **Category** | Core |

**Description:**
For maximum quality, users can capture with DSLR cameras to SD card while webcam provides real-time pose tracking. After scanning, high-res images are imported and time-synced with pose data for final reconstruction.

**Why State-of-the-Art:**
- Maximum resolution without USB bandwidth limits
- Real-time preview + ultimate quality
- Automatic time-sync matching
- Metadata extraction (EXIF, focus, exposure)

**Technical Implementation:**
- `trueshot-core/src/reconstruction/hybrid.rs` - import_high_res_images()
- SD card detection and file watching

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| Polycam | ❌ (mobile only) | Pro DSLR integration |
| RealityCapture | ✅ (import only) | Real-time pose preview |
| Meshroom | ✅ (import only) | Automated sync |

---

## 2. 3D Gaussian Splatting Features

### 2.1 Native 3DGS Training

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P0 (Critical) |
| **Category** | Core |

**Description:**
Fully native Rust implementation of 3D Gaussian Splatting training. No external Python dependencies or CUDA libraries required. Trains photorealistic 3DGS models from captured images.

**Why State-of-the-Art:**
- Pure Rust - no Python/CUDA setup
- Based on original 3DGS paper (SIGGRAPH 2023)
- Automatic densification and pruning
- Progressive training with real-time preview

**Technical Implementation:**
- `trueshot-core/src/gaussian_splatting/gaussian.rs` - Gaussian3D
- `trueshot-core/src/gaussian_splatting/trainer.rs`
- `trueshot-core/src/gaussian_splatting/optimizer.rs` - Adam

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| Luma AI | ✅ (cloud) | Fully local, no upload |
| gsplat | ✅ (Python) | No dependencies |
| 3DGS Original | ✅ (CUDA) | Cross-platform, Rust |

---

### 2.2 Mip-Splatting Anti-Aliasing

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P1 (High) |
| **Category** | Core |

**Description:**
Implements Mip-Splatting (SIGGRAPH 2024) for alias-free rendering at any scale. Eliminates pixel-shimmer and artifacts when zooming in/out. Essential for high-quality WebXR viewing.

**Why State-of-the-Art:**
- 3D low-pass filter for scale adaptation
- 2D Mip filter for screen-space anti-aliasing
- Multi-scale Gaussian representation (LOD)
- Dramatically improved visual quality at all zoom levels

**Technical Implementation:**
- `trueshot-core/src/gaussian_splatting/mip.rs` - MipSplatting

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| Luma AI | ❌ | Superior visual quality |
| Polycam | ❌ | No aliasing artifacts |
| All Others | ❌ | Clean zooming |

---

### 2.3 Anisotropic Spherical Gaussians (Spec-Gaussian)

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P2 (Medium) |
| **Category** | Core |

**Description:**
Implements Spec-Gaussian (NeurIPS 2024) for accurate specular reflection rendering. Standard 3DGS struggles with shiny surfaces - ASG correctly represents view-dependent reflections.

**Why State-of-the-Art:**
- NeurIPS 2024 cutting-edge research
- Accurate specular highlights and reflections
- Works on glossy, metallic, and mixed surfaces
- Separated diffuse and specular components

**Technical Implementation:**
- `trueshot-core/src/gaussian_splatting/asg.rs` - AnisotropicSphericalGaussian

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| All Others | ❌ | Accurate shiny surfaces |

---

### 2.4 4D Gaussian Splatting (Dynamic Scenes)

| Attribute | Value |
|-----------|-------|
| **Status** | 🔴 Planned |
| **Priority** | P0 (Critical) |
| **Category** | Core |

**Description:**
Capture and render dynamic/moving scenes using 4D Gaussian Splatting. Multi-camera synchronized footage is processed into a temporal 3DGS representation that can be played back in real-time.

**Why State-of-the-Art:**
- CVPR/ICLR 2024 research (True 4D Gaussians)
- 30-82 FPS real-time playback
- Variable-length capture support
- 4D Spherindrical Harmonics for time-varying appearance
- **UNIQUE DIFFERENTIATOR** - No competitor offers this

**Technical Implementation:**
- `trueshot-core/src/gaussian_splatting/gaussian_4d.rs` (planned)
- `trueshot-core/src/gaussian_splatting/trainer_4d.rs` (planned)

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| Polycam | ❌ | **UNIQUE** |
| RealityCapture | ❌ | **UNIQUE** |
| Luma AI | ❌ | **UNIQUE** |
| All Others | ❌ | **MAJOR DIFFERENTIATOR** |

---

### 2.5 GPU Rasterizer (WGPU)

| Attribute | Value |
|-----------|-------|
| **Status** | 🟡 In Progress |
| **Priority** | P0 (Critical) |
| **Category** | Core |

**Description:**
Native WebGPU (WGPU) compute shader rasterizer for 3DGS. Real-time rendering of millions of Gaussians directly in browser and desktop applications.

**Why State-of-the-Art:**
- WebGPU for cross-platform GPU access
- Compute shader-based pipeline
- Tile-based sorting for efficiency
- Direct GPU training gradients (planned)

**Technical Implementation:**
- `trueshot-core/src/gaussian_splatting/rasterizer.rs` - WGPU compute

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| gsplat | ✅ (CUDA) | Cross-platform, web-ready |
| Browser | ✅ (WebGL hack) | Native compute shaders |

---

## 3. Computer Vision Features

### 3.1 FAST Corner Detection

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P1 (High) |
| **Category** | Core |

**Description:**
Native Rust implementation of FAST-9/12 corner detection. Parallelized with Rayon for 8-16x speedup on multi-core CPUs.

**Why State-of-the-Art:**
- No OpenCV dependency
- SIMD-ready implementation (planned)
- Parallelized row processing
- Non-maximum suppression included

**Technical Implementation:**
- `trueshot-vision/src/features/fast.rs`

---

### 3.2 BRIEF Binary Descriptors

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P1 (High) |
| **Category** | Core |

**Description:**
rBRIEF rotation-invariant binary descriptors. 256-bit compact descriptors with Hamming distance matching.

**Why State-of-the-Art:**
- Rotation compensation (rBRIEF)
- Extremely fast matching (POPCNT instruction)
- Memory efficient (32 bytes per descriptor)

**Technical Implementation:**
- `trueshot-vision/src/features/brief.rs`

---

### 3.3 MAGSAC++ Robust Estimation

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P0 (Critical) |
| **Category** | Core |

**Description:**
State-of-the-art robust estimation (CVPR 2020) for outlier rejection. No manual threshold tuning required - automatically determines inlier/outlier boundaries.

**Why State-of-the-Art:**
- Threshold-free model quality evaluation
- Sigma-consensus marginalizing over noise
- IRLS refinement for sub-pixel accuracy
- Superior to classic RANSAC

**Technical Implementation:**
- `trueshot-vision/src/geometry/magsac.rs`

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| COLMAP | ✅ (LORANSAC) | No threshold tuning |
| OpenCV | ❌ (classic RANSAC) | Better accuracy |
| Meshroom | ✅ (AC-RANSAC) | Latest research |

---

### 3.4 Bundle Adjustment

| Attribute | Value |
|-----------|-------|
| **Status** | 🟡 In Progress |
| **Priority** | P0 (Critical) |
| **Category** | Core |

**Description:**
Native bundle adjustment for sub-pixel accurate camera poses and 3D points. Levenberg-Marquardt optimization with Huber robust loss.

**Why State-of-the-Art:**
- Rayon-parallelized cost computation
- Robust Huber loss for outlier handling
- Radial distortion modeling
- Sparse solver optimization (planned)

**Technical Implementation:**
- `trueshot-vision/src/geometry/bundle_adjustment.rs`

---

## 4. Hardware Support

### 4.1 DSLR Camera Control (gPhoto2)

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P1 (High) |
| **Category** | Hardware |

**Description:**
Control professional DSLR cameras (Canon, Nikon, Sony) via gPhoto2. Adjust settings, trigger capture, download images.

**Technical Implementation:**
- `trueshot-core/src/camera/gphoto.rs`

---

### 4.2 Turntable Control

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P1 (High) |
| **Category** | Hardware |

**Description:**
Control motorized turntables for automated object rotation. Precise angle positioning for complete 360° coverage.

**Technical Implementation:**
- `trueshot-core/src/hardware/turntable.rs`

---

### 4.3 Insta360 Gimbal Control

| Attribute | Value |
|-----------|-------|
| **Status** | 🟡 In Progress |
| **Priority** | P2 (Medium) |
| **Category** | Hardware |

**Description:**
Control Insta360 camera gimbal for pan/tilt/zoom. Virtual joystick interface.

**Technical Implementation:**
- `trueshot-core/src/camera/insta360.rs`

---

## 5. WebXR Features

### 5.1 WebXR Model Viewer

| Attribute | Value |
|-----------|-------|
| **Status** | 🟡 In Progress |
| **Priority** | P1 (High) |
| **Category** | XR |

**Description:**
View scanned models in VR/AR headsets via WebXR. QR code links to immersive viewing experience.

**Why State-of-the-Art:**
- Browser-based, no app install
- Supports 3DGS splat rendering
- Hand tracking interaction
- Place models in real environment

**Technical Implementation:**
- `trueshot-dashboard/src/components/UnifiedViewer.tsx`

---

### 5.2 VR Object Scanning

| Attribute | Value |
|-----------|-------|
| **Status** | 🔴 Planned |
| **Priority** | P1 (High) |
| **Category** | XR |

**Description:**
Use VR headset (Quest 3, Vision Pro) to scan individual objects. Walk around object, guided capture path, real-time preview.

**Why State-of-the-Art:**
- **UNIQUE FEATURE** - no competitor offers this
- Hand tracking for gesture controls
- Depth sensor integration
- Instant preview in XR

---

### 5.3 VR Room Scanning

| Attribute | Value |
|-----------|-------|
| **Status** | 🔴 Planned |
| **Priority** | P1 (High) |
| **Category** | XR |

**Description:**
Full room walkthrough scanning with VR headset. Automatic floor plan generation, portal detection, multi-room support.

**Why State-of-the-Art:**
- **UNIQUE FEATURE** - beyond Meta's Hyperscape
- Our hybrid 3DGS+mesh pipeline
- Exportable floor plans
- Multi-room stitching

---

### 5.4 XR Scan Gallery

| Attribute | Value |
|-----------|-------|
| **Status** | 🔴 Planned |
| **Priority** | P2 (Medium) |
| **Category** | XR |

**Description:**
Browse and interact with previous scans in XR. Place scans in current environment, scale for comparison, merge scans.

---

## 6. Export & Sharing

### 6.1 PLY Export (Gaussian Splats)

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P1 (High) |
| **Category** | Export |

**Description:**
Export 3DGS models in PLY format compatible with standard viewers.

---

### 6.2 OBJ/GLTF Mesh Export

| Attribute | Value |
|-----------|-------|
| **Status** | 🟡 In Progress |
| **Priority** | P1 (High) |
| **Category** | Export |

**Description:**
Export textured meshes in industry-standard formats for DCC tools.

---

### 6.3 USDZ Export (Apple AR)

| Attribute | Value |
|-----------|-------|
| **Status** | 🔴 Planned |
| **Priority** | P2 (Medium) |
| **Category** | Export |

**Description:**
Apple USDZ format for iOS AR Quick Look.

---

### 6.4 QR Code Sharing

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P2 (Medium) |
| **Category** | Export |

**Description:**
Generate QR codes linking to WebXR viewer for easy sharing.

---

## 7. Licensing & Security

### 7.1 Device-Bound Licensing

| Attribute | Value |
|-----------|-------|
| **Status** | 🔴 Planned |
| **Priority** | P0 (Critical) |
| **Category** | Security |

**Description:**
Hardware-bound licensing with cryptographic verification. Ed25519 signed licenses, offline verification, device activation limits.

---

### 7.2 License Tiers (Hobby/Education/Pro)

| Attribute | Value |
|-----------|-------|
| **Status** | 🔴 Planned |
| **Priority** | P0 (Critical) |
| **Category** | Security |

**Description:**
Tiered licensing with device limits: Hobby (1), Education (3), Pro (10). Feature access controlled by license.

---

## 8. AI/ML Features

### 8.1 AI Single-Image 3D

| Attribute | Value |
|-----------|-------|
| **Status** | 🔴 Planned |
| **Priority** | P2 (Medium) |
| **Category** | AI |

**Description:**
Generate 3D model from single image using AI. Similar to Polycam's feature but with our hybrid pipeline.

---

### 8.2 Voice Control

| Attribute | Value |
|-----------|-------|
| **Status** | 🔴 Planned |
| **Priority** | P3 (Low) |
| **Category** | AI |

**Description:**
Voice commands for hands-free scanning. "Capture", "Rotate left", "Start scan", etc.

---

## 9. Pro Tier Features ⭐

> **Note:** These premium features are available with Pro/Enterprise licenses

### 9.1 Avatar Capture Mode ⭐ PRO

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P1 (High) |
| **Category** | Pro |
| **Tier** | 💎 PRO |

**Description:**
Complete avatar creation system for generating high-quality, rigged 3D avatars from multi-camera scans. Includes SMPL-X body fitting, clothing layer separation, blendshape generation, and voice profile cloning for virtual assistants.

**Why State-of-the-Art:**
- SMPL-X integration with 55-joint skeleton (HumanRig-compatible)
- SO-SMPL-style clothing separation for mix-and-match
- 10 base blendshapes for facial animation
- Voice cloning integration for TTS avatars
- Guided capture workflow (T-pose → Expressions → Voice)

**Technical Implementation:**
- `trueshot-core/src/avatar/mod.rs` - Full avatar pipeline (~600 LOC)
- `trueshot-dashboard/src/components/AvatarCapture.tsx` - UI (~400 LOC)

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| Ready Player Me | ✅ (simplified) | Full body + skeleton |
| Polycam | ❌ | Complete avatar pipeline |
| RealityCapture | ❌ | Integrated voice cloning |

---

### 9.2 Scene Reconstruction Mode ⭐ PRO

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P0 (Critical) |
| **Category** | Pro |
| **Tier** | 💎 PRO |

**Description:**
Reconstruct 4DGS scenes from crowd-sourced, heterogeneous video footage. Combine personal phone recordings, official broadcasts, online videos, and surveillance footage into a single coherent 4D reconstruction with confidence visualization.

**Why State-of-the-Art:**
- Audio fingerprinting + cross-correlation for automatic temporal sync
- Video quality assessment (resolution, stability, sharpness, exposure)
- 4D confidence field showing reconstruction certainty per voxel
- Visualization modes: Heatmap, Transparency, Wireframe
- **UNIQUE FEATURE** - No competitor offers crowd-sourced 4DGS

**Use Cases:**
- Concert/Event reconstruction from phone footage + official recording
- Incident investigation - walk through crime scene from multiple angles
- Sports replay from broadcast + fan footage
- Historical event preservation

**Technical Implementation:**
- `trueshot-core/src/scene_reconstruction/mod.rs` - Full pipeline (~900 LOC)
- `trueshot-dashboard/src/components/SceneReconstruction.tsx` - UI (~900 LOC)

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| Polycam | ❌ | **UNIQUE** |
| RealityCapture | ❌ | **UNIQUE** |
| Luma AI | ❌ | **UNIQUE** |
| All Others | ❌ | **MAJOR DIFFERENTIATOR** |

---

### 9.3 4D Gaussian Splatting ⭐ PRO

| Attribute | Value |
|-----------|-------|
| **Status** | 🔴 Planned |
| **Priority** | P0 (Critical) |
| **Category** | Pro |
| **Tier** | 💎 PRO |

**Description:**
Capture and render dynamic/moving scenes using 4D Gaussian Splatting. Multi-camera synchronized footage is processed into a temporal 3DGS representation with real-time playback.

**Why State-of-the-Art:**
- CVPR/ICLR 2024 research (True 4D Gaussians)
- 30-82 FPS real-time playback
- 4D Spherindrical Harmonics for time-varying appearance
- **UNIQUE DIFFERENTIATOR** - No competitor offers this

---

## 10. Device & Hardware Management

### 10.1 Device Manager Pro

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P1 (High) |
| **Category** | Hardware |

**Description:**
Enterprise-grade device management for large 4DGS setups with 20+ devices. Table view with search/filter, persistent nicknames, external storage integration, and bulk actions.

**Why State-of-the-Art:**
- Handles 20+ cameras, mics, lights, robot arms simultaneously
- Search by name, ID, manufacturer, nickname
- Persistent nicknames (localStorage + backend sync)
- External storage: NAS (SMB/NFS), S3, Google Cloud Storage, Azure
- Bulk enable/disable operations

**Technical Implementation:**
- `trueshot-dashboard/src/components/DeviceManagerPro.tsx` (~1100 LOC)
- `trueshot-device-manager/src/storage.rs` - Backend storage (~500 LOC)

---

### 10.2 External Storage Integration

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P1 (High) |
| **Category** | Hardware |

**Description:**
Connect external storage for scan data: NAS, Amazon S3, Google Cloud Storage, Azure Blob Storage. Auto-sync with configurable patterns.

**Technical Implementation:**
- `trueshot-device-manager/src/storage.rs` - StorageManager

**Supported Storage:**
| Type | Protocol | Status |
|------|----------|--------|
| Local | Filesystem | ✅ |
| NAS | SMB/NFS | ✅ |
| S3 | AWS/MinIO/R2 | ✅ |
| GCS | Google Cloud | ✅ |
| Azure | Blob Storage | ✅ |

---

## 11. Photo Management

### 11.1 Photo Editor Mode (Lightroom-Style Workflow) 🆕

| Attribute | Value |
|-----------|-------|
| **Status** | 🟡 In Progress |
| **Priority** | P0 (Critical) |
| **Category** | Core |
| **Tier** | Core |

**Description:**
Professional photo editing integrated directly into TrueShot. Toggle between Scan Mode and Photo Editor Mode on the main page. Non-destructive RAW processing with comprehensive adjustments, GPU-accelerated preview, and preset system.

**Product Goal:**
- Reduce dependence on external editors after color-managed RAW parity is validated
- GPU compute shaders for instant real-time preview
- Direct integration with 3DGS pipeline - edited photos feed into reconstruction
- XMP sidecar compatibility for interoperability
- Full preset system with import capability

**Key Features:**
- Photo grid with ratings (1-5 stars), color labels, flags
- Complete develop module: Exposure, Contrast, Highlights/Shadows, Whites/Blacks
- White Balance with temperature/tint and presets
- Tone Curve with RGB/R/G/B channels
- HSL panel (Hue/Saturation/Luminance per color)
- Sharpening and Noise Reduction
- Lens Corrections with profile database
- Filmstrip navigation in develop mode

**Technical Implementation:**
- `trueshot-dashboard/src/components/PhotoEditor.tsx` - Main component (~800 LOC)
- `trueshot-core/src/raw_processing/` - Backend adjustment engine
- GPU-accelerated via WGPU compute shaders

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| Polycam | ❌ | Integrated editing |
| RealityCapture | ❌ | No external tools needed |
| Luma AI | ❌ | Full professional editing |

---

### 11.2 Advanced DSLR Camera Control 🆕

| Attribute | Value |
|-----------|-------|
| **Status** | 🟡 In Progress |
| **Priority** | P1 (High) |
| **Category** | Hardware |
| **Tier** | Core |

**Description:**
Professional tethered capture with HDR bracket capture, focus stacking, and combined HDR + Focus Stack workflows. Full save location control. End-to-end hardware and direct competitor qualification remain in progress.

**Current Architecture:**
- Native NEF groups use one linear, CFA-safe HDR/focus fusion path
- Mertens/Debevec and 8-bit Laplacian utilities are preview-grade until routed through the validated linear core
- Combined HDR+FS retains dynamic range and focus depth without intermediate image files
- Integration with Scene Reconstruction for HDR 4DGS

**Key Features:**
| Feature | Description |
|---------|-------------|
| HDR Bracketing | 3/5/7/9 shots, 1-3 EV spacing, auto-merge |
| Focus Stacking | 5-50 slices, front-to-back/back-to-front/center-out |
| HDR + Focus | Combined workflow; direct output-quality qualification is in progress |
| Manual Focus | Fine-grained digital focus control |
| Save Location | Camera SD / Computer / Both |

**Technical Implementation:**
- `trueshot-dashboard/src/components/CameraControlPro.tsx` - UI component (~600 LOC)
- `trueshot-core/src/capture/hdr.rs` - HDR capture & merge
- `trueshot-core/src/capture/focus_stack.rs` - Focus stacking

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| NKTether | ✅ | Integrated in scanning pipeline |
| Capture One | ✅ | Integrated local-first workflow is the target; parity is not yet qualified |
| Helicon Remote | Partial | Combined HDR+FS capture exists; output quality comparison is pending |

---

## 12. Event & Crowd-Source Capture

### 12.1 Guest Portal (Crowd-Source Capture) ⭐ UNIQUE FEATURE 🆕

| Attribute | Value |
|-----------|-------|
| **Status** | 🟡 In Progress |
| **Priority** | P0 (Critical) |
| **Category** | Pro |
| **Tier** | 💎 PRO |

**Description:**
Enable crowd-sourced video capture from guests' phones at events. Simple QR code access to `/guest/{event-id}` - zero app install, pure browser-based. Synchronized recording across all devices with sub-100ms accuracy. Email collection for post-event 4DGS delivery.

**Why State-of-the-Art:**
- **UNIQUE FEATURE** - No competitor offers this
- Zero app install - works in any mobile browser
- NTP-style time synchronization for accurate 4DGS alignment
- Chunked, resumable uploads - works on spotty event WiFi
- Scalable to 100+ concurrent guests
- Perfect for weddings, concerts, graduations, sports events

**Use Cases:**
| Event | How It Works |
|-------|--------------|
| **Weddings** | QR on tables, guests record ceremony, receive 4DGS memory |
| **Concerts** | Venue displays QR, audience contributes angles for artist archive |
| **Graduations** | Parents record from different angles, combined 4DGS of the walk |
| **Sports** | Fans in stands capture multi-angle replays |
| **Corporate** | Conference capture from multiple perspectives |

**Key Features:**
- Guest Portal with camera preview, start/stop recording
- Save to device option for guests
- Email collection for 4DGS delivery
- Organizer Dashboard with live guest count, recording status
- Master start/stop all functionality
- QR code generation
- Chunked upload with progress tracking
- Time synchronization service

**Technical Implementation:**
- `trueshot-dashboard/src/components/GuestPortal.tsx` - Guest UI (~700 LOC)
- `trueshot-dashboard/src/components/EventDashboard.tsx` - Organizer UI (~500 LOC)
- `trueshot-server/src/guest/` - Backend event management
- WebSocket-based sync service

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| Polycam | ❌ | **Only TrueShot has this** |
| RealityCapture | ❌ | **Only TrueShot has this** |
| Luma AI | ❌ | **Only TrueShot has this** |
| Any Other | ❌ | **Complete market exclusivity** |

---

### 12.2 Slave Phone (Server-Controlled Capture) ⭐ UNIQUE FEATURE 🆕

| Attribute | Value |
|-----------|-------|
| **Status** | 🟢 Complete |
| **Priority** | P0 (Critical) |
| **Category** | Pro |
| **Tier** | 💎 PRO |

**Description:**
Mount any smartphone as a server-controlled camera for 3DGS scanning. Phones connect via browser to `/slave` and wait for capture commands. Supports simultaneous capture from unlimited phones for multi-view reconstruction.

**Why State-of-the-Art:**
- **UNIQUE FEATURE** - No competitor offers phone-as-controlled-camera
- Batch trigger: capture on ALL phones simultaneously
- Cross-platform: iPhone and Android via browser
- Wake lock keeps screen on for mounted cameras
- Battery/status monitoring from server
- Resolution/quality control per-phone
- Phone orientation for SfM pose hints

**Use Cases:**
| Scenario | How It Works |
|----------|--------------|
| **3DGS Turntable** | Mount 6-12 phones around turntable, batch capture at each rotation |
| **Room Scanning** | Place phones at corners, synchronized capture |
| **Multi-View Portrait** | Ring of phones around subject for instant 3DGS avatar |
| **Event Replay** | Fixed phones + guest phones for complete coverage |

**Key Features:**
- Auto-reconnect WebSocket for stable connection
- Countdown timer before capture
- Flash effect and shutter sound
- Quality slider (50-100%)
- Resolution selection (720p/1080p/4K)
- Device naming for identification
- Ready/Not Ready toggle for coordination

**Technical Implementation:**
- `trueshot-dashboard/src/components/SlavePhone.tsx` - Phone UI (~550 LOC)
- `trueshot-server/src/guest/slave.rs` - Backend controller (~450 LOC)
- WebSocket protocol for real-time control

**API Endpoints:**
| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/phones` | GET | List all connected phones |
| `/api/phones/{id}/capture` | POST | Trigger capture on one phone |
| `/api/phones/capture-all` | POST | Batch capture on all ready phones |
| `/api/phones/ws` | WS | WebSocket for phone connection |

**Competitive Analysis:**
| Competitor | Has Feature? | Our Advantage |
|------------|-------------|---------------|
| Polycam | ❌ | **Only TrueShot has this** |
| RealityCapture | ❌ | **Only TrueShot has this** |
| Luma AI | ❌ | **Only TrueShot has this** |
| Multi-cam rigs | Expensive hardware | Free - use existing phones |

---

## Summary Statistics

| Status | Count |
|--------|-------|
| 🟢 Complete | 17 |
| 🟡 In Progress | 10 |
| 🔴 Planned | 10 |
| ⚪ Future | 5+ |

### Features by Tier

| Tier | Count | Features |
|------|-------|----------|
| **Core** | 16 | Background subtraction, AI analysis, 3DGS training, exports, Photo Editor, Camera Control, etc. |
| **💎 PRO** | 7 | Avatar Capture, Scene Reconstruction, 4D GS, Guest Portal, VR Scanning (planned) |

### License Compliance

| Category | Status |
|----------|--------|
| Rust Dependencies | ✅ 100% permissive (MIT/Apache-2.0/BSD) |
| Frontend Dependencies | ✅ 100% permissive (MIT/ISC) |
| Optional Dependencies | ⚠️ OpenCV/gPhoto2 feature-gated |
| Commercial Distribution | ✅ APPROVED |

**Key Differentiators (Unique to TrueShot):**
1. Hybrid 3DGS + Photogrammetry in one pipeline
2. 4D Gaussian Splatting for dynamic scenes
3. VR/AR headset scanning
4. AI-driven automated quality optimization
5. Native Rust implementation (no Python/CUDA)
6. **Scene Reconstruction from crowd-sourced footage** (NEW)
7. **Integrated Avatar creation pipeline** (NEW)
8. **Guest Portal for event crowd-capture** ⭐ MARKET EXCLUSIVE
9. **Integrated photo editor workflow (color-managed Lightroom parity in progress)**
10. **Native RAW HDR + focus stacking (direct competitor qualification in progress)**
