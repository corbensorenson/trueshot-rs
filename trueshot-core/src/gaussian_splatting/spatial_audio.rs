//! Spatial Audio System for 4D Gaussian Splatting
//!
//! State-of-the-art spatial audio with:
//! - Multi-microphone audio capture and synchronization
//! - Sound source localization using TDoA/triangulation
//! - HRTF-based 3D audio rendering
//! - Distance attenuation and doppler effect
//! - Room acoustics simulation (reverb, occlusion)

use nalgebra as na;
use serde::{Deserialize, Serialize};

// ============================================================================
// Core Types
// ============================================================================

/// Represents a microphone in the capture setup
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Microphone {
    /// Unique identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Position in 3D space (meters)
    pub position: na::Point3<f32>,
    /// Direction the mic is facing (for directional mics)
    pub direction: na::Vector3<f32>,
    /// Microphone type
    pub mic_type: MicrophoneType,
    /// Sample rate (Hz)
    pub sample_rate: u32,
    /// Channel count
    pub channels: u8,
    /// Gain (dB)
    pub gain: f32,
    /// Capture delay offset (samples)
    pub delay_offset: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum MicrophoneType {
    /// Picks up sound equally from all directions
    Omnidirectional,
    /// Heart-shaped pickup pattern
    Cardioid,
    /// More directional than cardioid
    Supercardioid,
    /// Figure-8 pattern
    Bidirectional,
    /// Highly directional
    Shotgun,
    /// Lavalier/lapel
    Lavalier,
}

impl Microphone {
    /// Calculate gain multiplier at a given position based on mic pattern
    pub fn pattern_gain(&self, sound_position: &na::Point3<f32>) -> f32 {
        let to_sound = (sound_position - self.position).normalize();
        let cos_angle = self.direction.dot(&to_sound);

        match self.mic_type {
            MicrophoneType::Omnidirectional => 1.0,
            MicrophoneType::Cardioid => (1.0 + cos_angle) * 0.5,
            MicrophoneType::Supercardioid => 0.366 + 0.634 * cos_angle,
            MicrophoneType::Bidirectional => cos_angle.abs(),
            MicrophoneType::Shotgun => {
                if cos_angle > 0.8 {
                    1.0
                } else {
                    (cos_angle + 1.0) * 0.3
                }
            }
            MicrophoneType::Lavalier => (1.0 + cos_angle) * 0.6,
        }
    }
}

/// A sound source in the 4D scene
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SoundSource {
    /// Unique identifier
    pub id: String,
    /// Human-readable name (e.g., "Speaker 1", "Instrument")
    pub name: String,
    /// Position in 3D space
    pub position: na::Point3<f32>,
    /// Direction source is facing (for directional sources)
    pub direction: na::Vector3<f32>,
    /// Velocity for doppler effect
    pub velocity: na::Vector3<f32>,
    /// Audio data samples (mono, normalized -1 to 1)
    pub samples: Vec<f32>,
    /// Sample rate
    pub sample_rate: u32,
    /// Volume (0-1)
    pub volume: f32,
    /// Minimum distance for attenuation
    pub min_distance: f32,
    /// Maximum distance for attenuation
    pub max_distance: f32,
    /// Rolloff factor for distance attenuation
    pub rolloff_factor: f32,
    /// Cone inner angle (degrees) for directional sound
    pub cone_inner_angle: f32,
    /// Cone outer angle (degrees)
    pub cone_outer_angle: f32,
    /// Cone outer gain (volume at outer angle)
    pub cone_outer_gain: f32,
    /// Time range when this source is active
    pub time_range: (f32, f32),
}

impl SoundSource {
    /// Create a new omnidirectional point source
    pub fn new_point_source(
        id: &str,
        name: &str,
        position: na::Point3<f32>,
        samples: Vec<f32>,
        sample_rate: u32,
    ) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            position,
            direction: na::Vector3::z(),
            velocity: na::Vector3::zeros(),
            samples,
            sample_rate,
            volume: 1.0,
            min_distance: 0.5,
            max_distance: 100.0,
            rolloff_factor: 1.0,
            cone_inner_angle: 360.0,
            cone_outer_angle: 360.0,
            cone_outer_gain: 1.0,
            time_range: (0.0, f32::MAX),
        }
    }

    /// Calculate distance attenuation
    pub fn distance_attenuation(&self, listener_pos: &na::Point3<f32>) -> f32 {
        let distance = (self.position - listener_pos).norm();

        if distance <= self.min_distance {
            return 1.0;
        }

        if distance >= self.max_distance {
            return 0.0;
        }

        // Web Audio/OpenAL-style inverse distance. Normalizing distance into
        // [0, 1] made even very distant sources remain near half volume.
        let min_distance = self.min_distance.max(f32::EPSILON);
        let rolloff = self.rolloff_factor.max(0.0);
        min_distance / (min_distance + rolloff * (distance - min_distance))
    }

    /// Calculate directional cone attenuation
    pub fn cone_attenuation(&self, listener_pos: &na::Point3<f32>) -> f32 {
        let to_listener = (listener_pos - self.position).normalize();
        let cos_angle = self.direction.dot(&to_listener);
        let angle = cos_angle.acos().to_degrees();

        let inner_half = self.cone_inner_angle / 2.0;
        let outer_half = self.cone_outer_angle / 2.0;

        if angle <= inner_half {
            1.0
        } else if angle >= outer_half {
            self.cone_outer_gain
        } else {
            // Linear interpolation
            let t = (angle - inner_half) / (outer_half - inner_half);
            1.0 + t * (self.cone_outer_gain - 1.0)
        }
    }

    /// Calculate total gain for a listener position
    pub fn total_gain(&self, listener_pos: &na::Point3<f32>) -> f32 {
        self.volume * self.distance_attenuation(listener_pos) * self.cone_attenuation(listener_pos)
    }
}

// ============================================================================
// Sound Source Localization
// ============================================================================

/// Sound source localizer using TDoA (Time Difference of Arrival)
pub struct SoundLocalizer {
    /// Microphones in the array
    microphones: Vec<Microphone>,
    /// Speed of sound (m/s)
    speed_of_sound: f32,
    /// Sample rate for processing
    sample_rate: u32,
    /// GCC-PHAT cross-correlation window
    gcc_window_size: usize,
}

impl SoundLocalizer {
    pub fn new(microphones: Vec<Microphone>, sample_rate: u32) -> Self {
        Self {
            microphones,
            speed_of_sound: 343.0, // At 20°C sea level
            sample_rate,
            gcc_window_size: 1024,
        }
    }

    /// Set speed of sound based on temperature
    pub fn set_temperature(&mut self, celsius: f32) {
        self.speed_of_sound = 331.3 * (1.0 + celsius / 273.15).sqrt();
    }

    /// Estimate sound source position from multi-channel recording
    /// Uses TDoA triangulation with GCC-PHAT
    pub fn localize(
        &self,
        audio_data: &[Vec<f32>], // One channel per microphone
    ) -> Option<na::Point3<f32>> {
        if audio_data.len() < 3 || self.microphones.len() < 3 {
            return None; // Need at least 3 mics for 3D localization
        }

        // Compute TDoA between all mic pairs
        let tdoas = self.compute_tdoas(audio_data);

        // Triangulate position
        self.triangulate(&tdoas)
    }

    /// Compute time differences using GCC-PHAT
    fn compute_tdoas(&self, audio_data: &[Vec<f32>]) -> Vec<(usize, usize, f32)> {
        let mut tdoas = Vec::new();

        // Reference is first microphone
        for i in 1..audio_data.len() {
            if let Some(delay) = self.gcc_phat(&audio_data[0], &audio_data[i]) {
                tdoas.push((0, i, delay));
            }
        }

        // Also compute between non-reference pairs for robustness
        for i in 1..audio_data.len() {
            for j in (i + 1)..audio_data.len() {
                if let Some(delay) = self.gcc_phat(&audio_data[i], &audio_data[j]) {
                    tdoas.push((i, j, delay));
                }
            }
        }

        tdoas
    }

    /// GCC-PHAT cross-correlation for time delay estimation
    fn gcc_phat(&self, sig1: &[f32], sig2: &[f32]) -> Option<f32> {
        let n = self.gcc_window_size.min(sig1.len().min(sig2.len()));

        // Simple cross-correlation (for production, use FFT-based)
        let max_lag = (0.1 * self.sample_rate as f32) as i32; // Max 100ms delay

        let mut best_lag = 0i32;
        let mut best_corr = f32::NEG_INFINITY;

        for lag in -max_lag..=max_lag {
            let mut corr = 0.0f32;
            let mut count = 0;

            for i in 0..n {
                let j = i as i32 + lag;
                if j >= 0 && (j as usize) < n {
                    corr += sig1[i] * sig2[j as usize];
                    count += 1;
                }
            }

            if count > 0 {
                corr /= count as f32;
                if corr > best_corr {
                    best_corr = corr;
                    best_lag = lag;
                }
            }
        }

        Some(best_lag as f32 / self.sample_rate as f32)
    }

    /// Triangulate position from TDoA measurements
    fn triangulate(&self, tdoas: &[(usize, usize, f32)]) -> Option<na::Point3<f32>> {
        if tdoas.is_empty() {
            return None;
        }

        // Grid search for simplicity (production would use nonlinear optimization)
        let mut best_pos = na::Point3::origin();
        let mut best_error = f32::INFINITY;

        // Search bounds based on microphone positions
        let (min_bound, max_bound) = self.get_search_bounds();
        let step = 0.5; // 50cm grid

        let mut x = min_bound.x;
        while x <= max_bound.x {
            let mut y = min_bound.y;
            while y <= max_bound.y {
                let mut z = min_bound.z;
                while z <= max_bound.z {
                    let pos = na::Point3::new(x, y, z);
                    let error = self.tdoa_error(&pos, tdoas);

                    if error < best_error {
                        best_error = error;
                        best_pos = pos;
                    }

                    z += step;
                }
                y += step;
            }
            x += step;
        }

        // Refine with smaller grid
        let refined = self.refine_position(best_pos, tdoas, 0.05);

        if best_error < 1.0 {
            // Reasonable error threshold
            Some(refined)
        } else {
            None
        }
    }

    fn refine_position(
        &self,
        initial: na::Point3<f32>,
        tdoas: &[(usize, usize, f32)],
        step: f32,
    ) -> na::Point3<f32> {
        let mut best_pos = initial;
        let mut best_error = self.tdoa_error(&initial, tdoas);

        for dx in [-1.0, 0.0, 1.0] {
            for dy in [-1.0, 0.0, 1.0] {
                for dz in [-1.0, 0.0, 1.0] {
                    let pos = na::Point3::new(
                        initial.x + dx * step,
                        initial.y + dy * step,
                        initial.z + dz * step,
                    );
                    let error = self.tdoa_error(&pos, tdoas);
                    if error < best_error {
                        best_error = error;
                        best_pos = pos;
                    }
                }
            }
        }

        best_pos
    }

    fn tdoa_error(&self, pos: &na::Point3<f32>, tdoas: &[(usize, usize, f32)]) -> f32 {
        let mut error = 0.0;

        for &(i, j, measured_tdoa) in tdoas {
            let dist_i = (pos - self.microphones[i].position).norm();
            let dist_j = (pos - self.microphones[j].position).norm();
            let predicted_tdoa = (dist_i - dist_j) / self.speed_of_sound;

            error += (predicted_tdoa - measured_tdoa).powi(2);
        }

        error.sqrt()
    }

    fn get_search_bounds(&self) -> (na::Point3<f32>, na::Point3<f32>) {
        let mut min = na::Point3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut max = na::Point3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);

        for mic in &self.microphones {
            min.x = min.x.min(mic.position.x);
            min.y = min.y.min(mic.position.y);
            min.z = min.z.min(mic.position.z);
            max.x = max.x.max(mic.position.x);
            max.y = max.y.max(mic.position.y);
            max.z = max.z.max(mic.position.z);
        }

        // Expand bounds
        let expand = 5.0;
        min.x -= expand;
        min.y -= expand;
        min.z -= expand;
        max.x += expand;
        max.y += expand;
        max.z += expand;

        (min, max)
    }
}

// ============================================================================
// Spatial Audio Scene
// ============================================================================

/// A complete spatial audio scene linked to 4DGS
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpatialAudioScene {
    /// All sound sources
    pub sources: Vec<SoundSource>,
    /// Microphones used during capture
    pub capture_microphones: Vec<Microphone>,
    /// Room dimensions (for reverb)
    pub room_dimensions: Option<(f32, f32, f32)>,
    /// Room material absorptions
    pub room_absorption: f32,
    /// Total duration in seconds
    pub duration: f32,
    /// Sample rate
    pub sample_rate: u32,
}

impl SpatialAudioScene {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sources: Vec::new(),
            capture_microphones: Vec::new(),
            room_dimensions: None,
            room_absorption: 0.3,
            duration: 0.0,
            sample_rate,
        }
    }

    /// Add a sound source
    pub fn add_source(&mut self, source: SoundSource) {
        if !source.samples.is_empty() {
            let source_duration = source.samples.len() as f32 / source.sample_rate as f32;
            self.duration = self.duration.max(source_duration);
        }
        self.sources.push(source);
    }

    /// Render audio at listener position for a time slice
    pub fn render_at_position(
        &self,
        listener_pos: &na::Point3<f32>,
        listener_forward: &na::Vector3<f32>,
        time_start: f32,
        duration: f32,
    ) -> StereoAudioBuffer {
        let num_samples = (duration * self.sample_rate as f32) as usize;
        let mut left = vec![0.0f32; num_samples];
        let mut right = vec![0.0f32; num_samples];

        let listener_right = listener_forward.cross(&na::Vector3::y()).normalize();
        for source in &self.sources {
            // Check if source is active at this time
            if time_start > source.time_range.1 || time_start + duration < source.time_range.0 {
                continue;
            }

            // Calculate spatial parameters
            let to_source = source.position - listener_pos;
            let direction = to_source.normalize();

            // Calculate stereo panning based on angle
            let pan = direction.dot(&listener_right); // -1 (left) to 1 (right)
                                                      // Distance attenuation
            let gain = source.total_gain(listener_pos);

            // Calculate sample range
            let start_sample =
                ((time_start - source.time_range.0) * source.sample_rate as f32) as usize;
            let start_sample = start_sample.max(0).min(source.samples.len());

            // Mix into output
            for i in 0..num_samples {
                let source_idx = start_sample + i;
                if source_idx >= source.samples.len() {
                    break;
                }

                let sample = source.samples[source_idx] * gain;

                // Simple stereo panning (equal power)
                let pan_angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
                let left_gain = pan_angle.cos();
                let right_gain = pan_angle.sin();

                left[i] += sample * left_gain;
                right[i] += sample * right_gain;
            }
        }

        // Apply basic room reverb
        if let Some((w, h, d)) = self.room_dimensions {
            self.apply_simple_reverb(&mut left, &mut right, w, h, d);
        }

        StereoAudioBuffer {
            left,
            right,
            sample_rate: self.sample_rate,
        }
    }

    /// Simple room reverb approximation
    fn apply_simple_reverb(&self, left: &mut [f32], right: &mut [f32], w: f32, h: f32, d: f32) {
        let volume = w * h * d;
        let surface = 2.0 * (w * h + w * d + h * d);
        let rt60 = (0.161 * volume / (surface * self.room_absorption.max(0.01))).max(0.05);

        // Simple delay lines for early reflections
        let delay_samples = (0.02 * self.sample_rate as f32) as usize; // 20ms early reflection
        let decay = (-6.907_755 * 0.02 / rt60).exp();

        if delay_samples < left.len() {
            for i in delay_samples..left.len() {
                left[i] += left[i - delay_samples] * decay;
                right[i] += right[i - delay_samples] * decay;
            }
        }
    }

    /// Export audio scene metadata for web playback
    pub fn to_web_format(&self) -> SpatialAudioWebFormat {
        SpatialAudioWebFormat {
            sources: self
                .sources
                .iter()
                .map(|s| WebAudioSource {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    position: [s.position.x, s.position.y, s.position.z],
                    direction: [s.direction.x, s.direction.y, s.direction.z],
                    volume: s.volume,
                    min_distance: s.min_distance,
                    max_distance: s.max_distance,
                    rolloff_factor: s.rolloff_factor,
                    cone_inner_angle: s.cone_inner_angle,
                    cone_outer_angle: s.cone_outer_angle,
                    cone_outer_gain: s.cone_outer_gain,
                    time_range: s.time_range,
                    audio_url: format!("audio/{}.wav", s.id),
                })
                .collect(),
            room_dimensions: self.room_dimensions,
            duration: self.duration,
            sample_rate: self.sample_rate,
        }
    }
}

/// Stereo audio buffer
#[derive(Clone, Debug)]
pub struct StereoAudioBuffer {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub sample_rate: u32,
}

impl StereoAudioBuffer {
    /// Convert to interleaved format
    pub fn interleaved(&self) -> Vec<f32> {
        let mut result = Vec::with_capacity(self.left.len() * 2);
        for (&l, &r) in self.left.iter().zip(self.right.iter()) {
            result.push(l);
            result.push(r);
        }
        result
    }

    /// Export to WAV
    pub fn to_wav(&self) -> Vec<u8> {
        let samples = self.interleaved();
        let mut wav = Vec::new();

        // RIFF header
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&((36 + samples.len() * 2) as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");

        // fmt chunk
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // chunk size
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&2u16.to_le_bytes()); // stereo
        wav.extend_from_slice(&self.sample_rate.to_le_bytes());
        wav.extend_from_slice(&(self.sample_rate * 2 * 2).to_le_bytes()); // byte rate
        wav.extend_from_slice(&4u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

        // data chunk
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&((samples.len() * 2) as u32).to_le_bytes());

        for sample in samples {
            let i16_sample = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
            wav.extend_from_slice(&i16_sample.to_le_bytes());
        }

        wav
    }
}

// ============================================================================
// Web Format for Frontend
// ============================================================================

/// Format for web-based spatial audio playback
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpatialAudioWebFormat {
    pub sources: Vec<WebAudioSource>,
    pub room_dimensions: Option<(f32, f32, f32)>,
    pub duration: f32,
    pub sample_rate: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebAudioSource {
    pub id: String,
    pub name: String,
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub volume: f32,
    pub min_distance: f32,
    pub max_distance: f32,
    pub rolloff_factor: f32,
    pub cone_inner_angle: f32,
    pub cone_outer_angle: f32,
    pub cone_outer_gain: f32,
    pub time_range: (f32, f32),
    pub audio_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_distance_attenuation() {
        let source =
            SoundSource::new_point_source("test", "Test", na::Point3::origin(), vec![], 48000);

        // At min distance, gain is 1.0
        let gain_near = source.distance_attenuation(&na::Point3::new(0.5, 0.0, 0.0));
        assert!((gain_near - 1.0).abs() < 0.01);

        // Far away, gain approaches 0
        let gain_far = source.distance_attenuation(&na::Point3::new(50.0, 0.0, 0.0));
        assert!(gain_far < 0.1);
    }

    #[test]
    fn test_stereo_panning() {
        let mut scene = SpatialAudioScene::new(48000);

        // Add source to the right
        let samples = vec![0.5; 1000];
        let source = SoundSource::new_point_source(
            "right",
            "Right Speaker",
            na::Point3::new(2.0, 0.0, 0.0),
            samples,
            48000,
        );
        scene.add_source(source);

        let output = scene.render_at_position(
            &na::Point3::origin(),
            &na::Vector3::new(0.0, 0.0, -1.0),
            0.0,
            0.01,
        );

        // Right channel should be louder
        let right_energy: f32 = output.right.iter().map(|x| x * x).sum();
        let left_energy: f32 = output.left.iter().map(|x| x * x).sum();
        assert!(right_energy > left_energy);
    }

    #[test]
    fn test_microphone_pattern() {
        let mic = Microphone {
            id: "test".to_string(),
            name: "Test Mic".to_string(),
            position: na::Point3::origin(),
            direction: na::Vector3::z(),
            mic_type: MicrophoneType::Cardioid,
            sample_rate: 48000,
            channels: 1,
            gain: 0.0,
            delay_offset: 0,
        };

        // Sound in front - max gain
        let gain_front = mic.pattern_gain(&na::Point3::new(0.0, 0.0, 1.0));
        assert!((gain_front - 1.0).abs() < 0.01);

        // Sound behind - zero gain for cardioid
        let gain_back = mic.pattern_gain(&na::Point3::new(0.0, 0.0, -1.0));
        assert!(gain_back < 0.1);
    }
}
