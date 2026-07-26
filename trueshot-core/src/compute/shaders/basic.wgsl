// Simple compute shader for pixel-wise multiplication (Gain) as a proof of concept
// Eventually will hold FFT and Wavelet logic.

@group(0) @binding(0) var<storage, read> input_buf: array<f32>;
@group(0) @binding(1) var<storage, read_write> output_buf: array<f32>;
@group(0) @binding(2) var<uniform> gain: f32;

@compute @workgroup_size(64)
fn apply_gain(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let index = global_id.x;
    if (index >= arrayLength(&input_buf)) {
        return;
    }
    output_buf[index] = input_buf[index] * gain;
}

// Placeholder for future FFT
// @compute ... fn fft_horizontal ...
