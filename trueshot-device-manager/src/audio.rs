//! Audio Device Manager
//!
//! Handles audio input device enumeration, configuration, and capture
//! for multi-microphone spatial audio recording.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::{Deserialize, Serialize};

/// Audio input device information
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioDevice {
    /// Unique device identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Manufacturer name if available
    pub manufacturer: Option<String>,
    /// Number of input channels
    pub channels: u8,
    /// Supported sample rates
    pub supported_sample_rates: Vec<u32>,
    /// Current sample rate
    pub sample_rate: u32,
    /// Is this device currently active
    pub is_active: bool,
    /// Is this the system default device
    pub is_default: bool,
    /// Device type
    pub device_type: AudioDeviceType,
    /// Estimated input latency in ms (from buffer size)
    pub latency_ms: Option<f32>,
    /// Default buffer size in frames if reported
    pub buffer_size_frames: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum AudioDeviceType {
    /// Built-in microphone
    BuiltIn,
    /// USB microphone/interface
    Usb,
    /// Audio interface (multi-channel)
    Interface,
    /// Wireless/Bluetooth
    Bluetooth,
    /// Virtual/software device
    Virtual,
    /// Unknown type
    Unknown,
}

/// Audio capture stream
struct AudioBuffer {
    samples: Mutex<Vec<Vec<f32>>>,
    is_recording: Mutex<bool>,
    sample_frames: Mutex<usize>,
    start_time: Mutex<Option<Instant>>,
}

impl AudioBuffer {
    fn new(channels: u8) -> Self {
        Self {
            samples: Mutex::new(vec![Vec::new(); channels as usize]),
            is_recording: Mutex::new(false),
            sample_frames: Mutex::new(0),
            start_time: Mutex::new(None),
        }
    }

    fn start(&self) {
        *self.is_recording.lock().unwrap() = true;
        let mut start = self.start_time.lock().unwrap();
        if start.is_none() {
            *start = Some(Instant::now());
        }
    }

    fn stop(&self) {
        *self.is_recording.lock().unwrap() = false;
    }

    fn push_samples(&self, channel: usize, samples: &[f32]) {
        if !*self.is_recording.lock().unwrap() {
            return;
        }
        let mut all_samples = self.samples.lock().unwrap();
        if channel < all_samples.len() {
            all_samples[channel].extend_from_slice(samples);
            if channel == 0 {
                let mut frames = self.sample_frames.lock().unwrap();
                *frames += samples.len();
            }
        }
    }

    fn push_interleaved(&self, interleaved: &[f32], channels: usize) {
        if !*self.is_recording.lock().unwrap() || channels == 0 {
            return;
        }
        let mut all_samples = self.samples.lock().unwrap();
        let frame_count = interleaved.len() / channels;
        for frame_idx in 0..frame_count {
            let base = frame_idx * channels;
            for ch in 0..channels {
                if let Some(value) = interleaved.get(base + ch) {
                    all_samples[ch].push(*value);
                }
            }
        }
        let mut frames = self.sample_frames.lock().unwrap();
        *frames += frame_count;
    }

    fn clear(&self) {
        let mut samples = self.samples.lock().unwrap();
        for channel in samples.iter_mut() {
            channel.clear();
        }
        *self.sample_frames.lock().unwrap() = 0;
    }

    fn duration_seconds(&self, sample_rate: u32) -> f32 {
        let frames = *self.sample_frames.lock().unwrap();
        if frames == 0 {
            return 0.0;
        }
        frames as f32 / sample_rate as f32
    }

    fn drift_samples(&self, sample_rate: u32) -> Option<i64> {
        let start = self.start_time.lock().unwrap().clone()?;
        let elapsed = start.elapsed().as_secs_f64();
        let expected = (elapsed * sample_rate as f64).round() as i64;
        let actual = *self.sample_frames.lock().unwrap() as i64;
        Some(actual - expected)
    }
}

pub struct AudioCaptureStream {
    /// Device ID
    pub device_id: String,
    /// Sample rate
    pub sample_rate: u32,
    /// Channels
    pub channels: u8,
    buffer: Arc<AudioBuffer>,
    stream: Option<cpal::Stream>,
}

impl AudioCaptureStream {
    pub fn new(device_id: &str, sample_rate: u32, channels: u8) -> Self {
        Self {
            device_id: device_id.to_string(),
            sample_rate,
            channels,
            buffer: Arc::new(AudioBuffer::new(channels)),
            stream: None,
        }
    }

    pub fn set_stream(&mut self, stream: cpal::Stream) {
        self.stream = Some(stream);
    }
    
    /// Start recording
    pub fn start(&self) {
        self.buffer.start();
        if let Some(stream) = self.stream.as_ref() {
            if let Err(err) = stream.play() {
                tracing::warn!("Failed to start audio stream {}: {}", self.device_id, err);
            }
        }
    }
    
    /// Stop recording
    pub fn stop(&self) {
        self.buffer.stop();
    }
    
    /// Add samples (called by audio callback)
    pub fn push_samples(&self, channel: usize, samples: &[f32]) {
        self.buffer.push_samples(channel, samples);
    }

    /// Add interleaved samples for all channels
    pub fn push_interleaved(&self, interleaved: &[f32]) {
        self.buffer.push_interleaved(interleaved, self.channels as usize);
    }
    
    /// Get recorded samples
    pub fn get_samples(&self) -> Vec<Vec<f32>> {
        self.buffer.samples.lock().unwrap().clone()
    }
    
    /// Clear recorded samples
    pub fn clear(&self) {
        self.buffer.clear();
    }
    
    /// Get recording duration
    pub fn duration_seconds(&self) -> f32 {
        self.buffer.duration_seconds(self.sample_rate)
    }

    /// Drift in samples relative to wall clock
    pub fn drift_samples(&self) -> Option<i64> {
        self.buffer.drift_samples(self.sample_rate)
    }

    pub fn drift_ms(&self) -> Option<f64> {
        self.drift_samples()
            .map(|samples| (samples as f64 / self.sample_rate as f64) * 1000.0)
    }
}

impl std::fmt::Debug for AudioCaptureStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioCaptureStream")
            .field("device_id", &self.device_id)
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .finish()
    }
}

/// Multi-device audio manager for synchronized capture
pub struct AudioManager {
    /// Available devices
    devices: Vec<AudioDevice>,
    /// Device map for cpal devices
    device_map: HashMap<String, cpal::Device>,
    /// Active capture streams
    streams: HashMap<String, AudioCaptureStream>,
    /// Master sample rate for synchronized capture
    master_sample_rate: u32,
    /// Reference device for synchronization
    reference_device: Option<String>,
}

impl AudioManager {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            device_map: HashMap::new(),
            streams: HashMap::new(),
            master_sample_rate: 48000,
            reference_device: None,
        }
    }
    
    /// Enumerate available audio input devices
    pub fn enumerate_devices(&mut self) -> Vec<AudioDevice> {
        let host = cpal::default_host();
        let default_device_name = host
            .default_input_device()
            .and_then(|d| d.name().ok());

        self.devices.clear();
        self.device_map.clear();

        if let Ok(devices) = host.input_devices() {
            for (idx, device) in devices.enumerate() {
                let name = device.name().unwrap_or_else(|_| "Unknown Input".to_string());
                let id = format!("{}::{}", name, idx);
                let default_config = device.default_input_config().ok();
                let (channels, sample_rate, buffer_size, latency_ms) = if let Some(cfg) = default_config.as_ref() {
                    let channels = cfg.channels() as u8;
                    let sample_rate = cfg.sample_rate().0;
                    let (buffer_size, latency_ms) = match cfg.buffer_size() {
                        cpal::SupportedBufferSize::Range { min: _, max } => {
                            let frames = *max;
                            let latency_ms = (frames as f32 / sample_rate as f32) * 1000.0;
                            (Some(frames), Some(latency_ms))
                        }
                        cpal::SupportedBufferSize::Unknown => (None, None),
                    };
                    (channels, sample_rate, buffer_size, latency_ms)
                } else {
                    (1, self.master_sample_rate, None, None)
                };

                let mut supported_sample_rates = Vec::new();
                if let Ok(configs) = device.supported_input_configs() {
                    for cfg in configs {
                        let min = cfg.min_sample_rate().0;
                        let max = cfg.max_sample_rate().0;
                        if min == max {
                            supported_sample_rates.push(min);
                        } else {
                            supported_sample_rates.push(min);
                            supported_sample_rates.push(max);
                        }
                    }
                }
                supported_sample_rates.sort_unstable();
                supported_sample_rates.dedup();

                let is_default = default_device_name
                    .as_ref()
                    .map(|n| *n == name)
                    .unwrap_or(false);

                let device_type = classify_device(&name);

                self.device_map.insert(id.clone(), device.clone());
                self.devices.push(AudioDevice {
                    id,
                    name,
                    manufacturer: None,
                    channels,
                    supported_sample_rates,
                    sample_rate,
                    is_active: false,
                    is_default,
                    device_type,
                    latency_ms,
                    buffer_size_frames: buffer_size,
                });
            }
        }

        if self.devices.is_empty() {
            tracing::warn!("No audio input devices detected");
        }

        self.devices.clone()
    }
    
    /// Get device by ID
    pub fn get_device(&self, id: &str) -> Option<&AudioDevice> {
        self.devices.iter().find(|d| d.id == id)
    }
    
    /// Set master sample rate for all captures
    pub fn set_master_sample_rate(&mut self, sample_rate: u32) {
        self.master_sample_rate = sample_rate;
    }
    
    /// Set reference device for synchronization
    pub fn set_reference_device(&mut self, device_id: &str) {
        self.reference_device = Some(device_id.to_string());
    }
    
    /// Create a capture stream for a device
    pub fn create_stream(&mut self, device_id: &str) -> Result<&AudioCaptureStream, AudioError> {
        let device_meta = self.devices.iter()
            .find(|d| d.id == device_id)
            .ok_or(AudioError::DeviceNotFound(device_id.to_string()))?;
        let device = self.device_map
            .get(device_id)
            .ok_or(AudioError::DeviceNotFound(device_id.to_string()))?;

        let (config, sample_format) = select_input_config(device, self.master_sample_rate)
            .map_err(|err| AudioError::StreamError(err.to_string()))?;

        let mut stream = AudioCaptureStream::new(
            device_id,
            config.sample_rate.0,
            device_meta.channels,
        );
        let shared_buffer = stream.buffer.clone();
        let channel_count = device_meta.channels as usize;
        let err_fn = |err| tracing::error!("Audio stream error: {}", err);

        let built = match sample_format {
            cpal::SampleFormat::F32 => {
                let buffer = shared_buffer.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[f32], _| buffer.push_interleaved(data, channel_count),
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let buffer = shared_buffer.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[i16], _| {
                        let mut samples = Vec::with_capacity(data.len());
                        for sample in data {
                            samples.push(*sample as f32 / i16::MAX as f32);
                        }
                        buffer.push_interleaved(&samples, channel_count);
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let buffer = shared_buffer.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[u16], _| {
                        let mut samples = Vec::with_capacity(data.len());
                        for sample in data {
                            samples.push((*sample as f32 / u16::MAX as f32) * 2.0 - 1.0);
                        }
                        buffer.push_interleaved(&samples, channel_count);
                    },
                    err_fn,
                    None,
                )
            }
            _ => {
                return Err(AudioError::StreamError("Unsupported sample format".to_string()));
            }
        }
        .map_err(|err| AudioError::StreamError(err.to_string()))?;

        stream.set_stream(built);
        self.streams.insert(device_id.to_string(), stream);
        Ok(self.streams.get(device_id).unwrap())
    }
    
    /// Start synchronized capture on all streams
    pub fn start_all(&self) {
        for stream in self.streams.values() {
            stream.start();
        }
    }
    
    /// Stop all streams
    pub fn stop_all(&self) {
        for stream in self.streams.values() {
            stream.stop();
        }
    }
    
    /// Get all captured audio
    pub fn get_all_recordings(&self) -> HashMap<String, Vec<Vec<f32>>> {
        self.streams.iter()
            .map(|(id, stream)| (id.clone(), stream.get_samples()))
            .collect()
    }
    
    /// Calculate synchronization offsets between devices
    pub fn calculate_sync_offsets(&self) -> HashMap<String, i32> {
        let mut offsets = HashMap::new();
        
        let reference_id = match &self.reference_device {
            Some(id) => id.clone(),
            None => return offsets,
        };
        
        let reference_samples = match self.streams.get(&reference_id) {
            Some(stream) => stream.get_samples(),
            None => return offsets,
        };
        
        if reference_samples.is_empty() || reference_samples[0].is_empty() {
            return offsets;
        }
        
        // Cross-correlate each device with reference to find offset
        for (device_id, stream) in &self.streams {
            if device_id == &reference_id {
                offsets.insert(device_id.clone(), 0);
                continue;
            }
            
            let device_samples = stream.get_samples();
            if device_samples.is_empty() || device_samples[0].is_empty() {
                continue;
            }
            
            // Find offset using cross-correlation
            let offset = self.cross_correlate_find_offset(
                &reference_samples[0],
                &device_samples[0],
            );
            
            offsets.insert(device_id.clone(), offset);
        }
        
        offsets
    }

    /// Summarize drift between devices (samples and ms)
    pub fn drift_report(&self) -> HashMap<String, (Option<i64>, Option<f64>)> {
        let mut report = HashMap::new();
        for (device_id, stream) in &self.streams {
            report.insert(device_id.clone(), (stream.drift_samples(), stream.drift_ms()));
        }
        report
    }
    
    fn cross_correlate_find_offset(&self, ref_signal: &[f32], test_signal: &[f32]) -> i32 {
        let max_lag = (0.1 * self.master_sample_rate as f32) as i32;  // Max 100ms offset
        let n = ref_signal.len().min(test_signal.len()).min(48000);  // Use first 1 second
        
        let mut best_lag = 0i32;
        let mut best_corr = f32::NEG_INFINITY;
        
        for lag in -max_lag..=max_lag {
            let mut corr = 0.0f32;
            let mut count = 0;
            
            for i in 0..n {
                let j = i as i32 + lag;
                if j >= 0 && (j as usize) < n {
                    corr += ref_signal[i] * test_signal[j as usize];
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
        
        best_lag
    }
}

impl Default for AudioManager {
    fn default() -> Self {
        Self::new()
    }
}

fn classify_device(name: &str) -> AudioDeviceType {
    let lower = name.to_lowercase();
    if lower.contains("usb") {
        AudioDeviceType::Usb
    } else if lower.contains("bluetooth") || lower.contains("airpods") {
        AudioDeviceType::Bluetooth
    } else if lower.contains("virtual") {
        AudioDeviceType::Virtual
    } else if lower.contains("interface") {
        AudioDeviceType::Interface
    } else if lower.contains("built") || lower.contains("internal") {
        AudioDeviceType::BuiltIn
    } else {
        AudioDeviceType::Unknown
    }
}

fn select_input_config(
    device: &cpal::Device,
    target_rate: u32,
) -> Result<(cpal::StreamConfig, cpal::SampleFormat), anyhow::Error> {
    let default_config = device.default_input_config()?;
    let sample_format = default_config.sample_format();
    let mut config: cpal::StreamConfig = default_config.config();
    if let Ok(configs) = device.supported_input_configs() {
        for cfg in configs {
            if cfg.sample_format() != sample_format {
                continue;
            }
            let min = cfg.min_sample_rate().0;
            let max = cfg.max_sample_rate().0;
            if target_rate >= min && target_rate <= max {
                config.sample_rate = cpal::SampleRate(target_rate);
                break;
            }
        }
    }
    Ok((config, sample_format))
}

/// Audio-related errors
#[derive(Clone, Debug)]
pub enum AudioError {
    DeviceNotFound(String),
    InvalidSampleRate(u32),
    StreamError(String),
    CaptureError(String),
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::DeviceNotFound(id) => write!(f, "Audio device not found: {}", id),
            AudioError::InvalidSampleRate(sr) => write!(f, "Invalid sample rate: {}", sr),
            AudioError::StreamError(msg) => write!(f, "Stream error: {}", msg),
            AudioError::CaptureError(msg) => write!(f, "Capture error: {}", msg),
        }
    }
}

impl std::error::Error for AudioError {}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_audio_manager() {
        let mut manager = AudioManager::new();
        let devices = manager.enumerate_devices();
        assert!(!devices.is_empty() || manager.device_map.is_empty());
    }
    
    #[test]
    fn test_capture_stream() {
        let stream = AudioCaptureStream::new("test", 48000, 2);
        stream.start();
        stream.push_interleaved(&[0.1, 0.4, 0.2, 0.5, 0.3, 0.6]);
        stream.stop();
        
        let samples = stream.get_samples();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].len(), 3);
    }
}
