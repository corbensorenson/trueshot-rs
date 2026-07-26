// Gaussian Blur Compute Shader
// Separable Gaussian blur for efficient O(n) blurring
// Uses two passes: horizontal then vertical

struct BlurParams {
    width: u32,
    height: u32,
    sigma: f32,
    kernel_radius: u32,
}

@group(0) @binding(0) var<storage, read> input_image: array<f32>;
@group(0) @binding(1) var<storage, read_write> output_image: array<f32>;
@group(0) @binding(2) var<uniform> params: BlurParams;

// Precomputed Gaussian kernel (max 32 radius)
// Weights are computed at runtime based on sigma
var<private> kernel_weights: array<f32, 33>;

fn compute_kernel() {
    let sigma2 = params.sigma * params.sigma * 2.0;
    var sum: f32 = 0.0;
    
    for (var i: u32 = 0u; i <= params.kernel_radius; i = i + 1u) {
        let weight = exp(-f32(i * i) / sigma2);
        kernel_weights[i] = weight;
        if (i == 0u) {
            sum = sum + weight;
        } else {
            sum = sum + weight * 2.0; // Symmetric
        }
    }
    
    // Normalize
    for (var i: u32 = 0u; i <= params.kernel_radius; i = i + 1u) {
        kernel_weights[i] = kernel_weights[i] / sum;
    }
}

fn get_pixel_clamped(x: i32, y: i32) -> f32 {
    let cx = clamp(x, 0, i32(params.width) - 1);
    let cy = clamp(y, 0, i32(params.height) - 1);
    return input_image[u32(cy) * params.width + u32(cx)];
}

// Horizontal blur pass
@compute @workgroup_size(256, 1)
fn blur_horizontal(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);
    
    if (global_id.x >= params.width || global_id.y >= params.height) {
        return;
    }
    
    compute_kernel();
    
    var sum: f32 = 0.0;
    let radius = i32(params.kernel_radius);
    
    // Center pixel
    sum = get_pixel_clamped(x, y) * kernel_weights[0];
    
    // Symmetric neighbors
    for (var i: i32 = 1; i <= radius; i = i + 1) {
        let weight = kernel_weights[u32(i)];
        sum = sum + get_pixel_clamped(x - i, y) * weight;
        sum = sum + get_pixel_clamped(x + i, y) * weight;
    }
    
    output_image[global_id.y * params.width + global_id.x] = sum;
}

// Vertical blur pass
@compute @workgroup_size(1, 256)
fn blur_vertical(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);
    
    if (global_id.x >= params.width || global_id.y >= params.height) {
        return;
    }
    
    compute_kernel();
    
    var sum: f32 = 0.0;
    let radius = i32(params.kernel_radius);
    
    // Center pixel
    sum = get_pixel_clamped(x, y) * kernel_weights[0];
    
    // Symmetric neighbors
    for (var i: i32 = 1; i <= radius; i = i + 1) {
        let weight = kernel_weights[u32(i)];
        sum = sum + get_pixel_clamped(x, y - i) * weight;
        sum = sum + get_pixel_clamped(x, y + i) * weight;
    }
    
    output_image[global_id.y * params.width + global_id.x] = sum;
}

// RGB Version
struct RgbBlurParams {
    width: u32,
    height: u32,
    sigma: f32,
    kernel_radius: u32,
}

@group(0) @binding(3) var<storage, read> input_rgb: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> output_rgb: array<vec4<f32>>;

fn get_pixel_rgb_clamped(x: i32, y: i32) -> vec3<f32> {
    let cx = clamp(x, 0, i32(params.width) - 1);
    let cy = clamp(y, 0, i32(params.height) - 1);
    return input_rgb[u32(cy) * params.width + u32(cx)].xyz;
}

@compute @workgroup_size(256, 1)
fn blur_horizontal_rgb(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);
    
    if (global_id.x >= params.width || global_id.y >= params.height) {
        return;
    }
    
    compute_kernel();
    
    var sum: vec3<f32> = vec3<f32>(0.0);
    let radius = i32(params.kernel_radius);
    
    sum = get_pixel_rgb_clamped(x, y) * kernel_weights[0];
    
    for (var i: i32 = 1; i <= radius; i = i + 1) {
        let weight = kernel_weights[u32(i)];
        sum = sum + get_pixel_rgb_clamped(x - i, y) * weight;
        sum = sum + get_pixel_rgb_clamped(x + i, y) * weight;
    }
    
    output_rgb[global_id.y * params.width + global_id.x] = vec4<f32>(sum, 1.0);
}

@compute @workgroup_size(1, 256)
fn blur_vertical_rgb(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);
    
    if (global_id.x >= params.width || global_id.y >= params.height) {
        return;
    }
    
    compute_kernel();
    
    var sum: vec3<f32> = vec3<f32>(0.0);
    let radius = i32(params.kernel_radius);
    
    sum = get_pixel_rgb_clamped(x, y) * kernel_weights[0];
    
    for (var i: i32 = 1; i <= radius; i = i + 1) {
        let weight = kernel_weights[u32(i)];
        sum = sum + get_pixel_rgb_clamped(x, y - i) * weight;
        sum = sum + get_pixel_rgb_clamped(x, y + i) * weight;
    }
    
    output_rgb[global_id.y * params.width + global_id.x] = vec4<f32>(sum, 1.0);
}

// Combined single-pass blur (less efficient but simpler)
@compute @workgroup_size(16, 16)
fn blur_combined(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);
    
    if (global_id.x >= params.width || global_id.y >= params.height) {
        return;
    }
    
    compute_kernel();
    
    var sum: f32 = 0.0;
    var weight_sum: f32 = 0.0;
    let radius = i32(params.kernel_radius);
    
    for (var dy: i32 = -radius; dy <= radius; dy = dy + 1) {
        for (var dx: i32 = -radius; dx <= radius; dx = dx + 1) {
            let dist = sqrt(f32(dx * dx + dy * dy));
            if (dist <= f32(radius)) {
                let weight = exp(-(dist * dist) / (2.0 * params.sigma * params.sigma));
                sum = sum + get_pixel_clamped(x + dx, y + dy) * weight;
                weight_sum = weight_sum + weight;
            }
        }
    }
    
    output_image[global_id.y * params.width + global_id.x] = sum / weight_sum;
}
