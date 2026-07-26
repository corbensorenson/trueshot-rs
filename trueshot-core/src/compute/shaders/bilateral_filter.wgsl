// Bilateral Filter Compute Shader
// Edge-preserving smoothing filter for image processing
// Combines spatial and range (color) proximity for filtering

struct Params {
    width: u32,
    height: u32,
    spatial_sigma: f32,
    range_sigma: f32,
    kernel_radius: u32,
    _padding: u32,
}

@group(0) @binding(0) var<storage, read> input_image: array<f32>;
@group(0) @binding(1) var<storage, read_write> output_image: array<f32>;
@group(0) @binding(2) var<uniform> params: Params;

// Gaussian weight function
fn gaussian(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma));
}

// Get pixel value with boundary clamping
fn get_pixel(x: i32, y: i32) -> f32 {
    let cx = clamp(x, 0, i32(params.width) - 1);
    let cy = clamp(y, 0, i32(params.height) - 1);
    return input_image[u32(cy) * params.width + u32(cx)];
}

@compute @workgroup_size(16, 16)
fn bilateral_filter(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);
    
    if (global_id.x >= params.width || global_id.y >= params.height) {
        return;
    }
    
    let center_value = get_pixel(x, y);
    let radius = i32(params.kernel_radius);
    
    var weight_sum: f32 = 0.0;
    var value_sum: f32 = 0.0;
    
    // Apply bilateral filter kernel
    for (var dy: i32 = -radius; dy <= radius; dy = dy + 1) {
        for (var dx: i32 = -radius; dx <= radius; dx = dx + 1) {
            let neighbor_value = get_pixel(x + dx, y + dy);
            
            // Spatial weight (distance from center)
            let spatial_dist = sqrt(f32(dx * dx + dy * dy));
            let spatial_weight = gaussian(spatial_dist, params.spatial_sigma);
            
            // Range weight (intensity difference)
            let range_diff = abs(neighbor_value - center_value);
            let range_weight = gaussian(range_diff, params.range_sigma);
            
            // Combined weight
            let weight = spatial_weight * range_weight;
            
            weight_sum = weight_sum + weight;
            value_sum = value_sum + neighbor_value * weight;
        }
    }
    
    // Normalize and write output
    let idx = global_id.y * params.width + global_id.x;
    if (weight_sum > 0.0) {
        output_image[idx] = value_sum / weight_sum;
    } else {
        output_image[idx] = center_value;
    }
}

// Color version for RGB images
struct ColorParams {
    width: u32,
    height: u32,
    spatial_sigma: f32,
    range_sigma: f32,
    kernel_radius: u32,
    _padding: u32,
}

@group(0) @binding(3) var<storage, read> input_rgb: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> output_rgb: array<vec4<f32>>;

fn get_pixel_rgb(x: i32, y: i32) -> vec3<f32> {
    let cx = clamp(x, 0, i32(params.width) - 1);
    let cy = clamp(y, 0, i32(params.height) - 1);
    let pixel = input_rgb[u32(cy) * params.width + u32(cx)];
    return pixel.xyz;
}

fn color_distance(a: vec3<f32>, b: vec3<f32>) -> f32 {
    let diff = a - b;
    return sqrt(dot(diff, diff));
}

@compute @workgroup_size(16, 16)
fn bilateral_filter_rgb(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = i32(global_id.x);
    let y = i32(global_id.y);
    
    if (global_id.x >= params.width || global_id.y >= params.height) {
        return;
    }
    
    let center_color = get_pixel_rgb(x, y);
    let radius = i32(params.kernel_radius);
    
    var weight_sum: f32 = 0.0;
    var color_sum: vec3<f32> = vec3<f32>(0.0);
    
    for (var dy: i32 = -radius; dy <= radius; dy = dy + 1) {
        for (var dx: i32 = -radius; dx <= radius; dx = dx + 1) {
            let neighbor_color = get_pixel_rgb(x + dx, y + dy);
            
            // Spatial weight
            let spatial_dist = sqrt(f32(dx * dx + dy * dy));
            let spatial_weight = gaussian(spatial_dist, params.spatial_sigma);
            
            // Range weight (color distance)
            let range_diff = color_distance(neighbor_color, center_color);
            let range_weight = gaussian(range_diff, params.range_sigma);
            
            let weight = spatial_weight * range_weight;
            
            weight_sum = weight_sum + weight;
            color_sum = color_sum + neighbor_color * weight;
        }
    }
    
    let idx = global_id.y * params.width + global_id.x;
    if (weight_sum > 0.0) {
        output_rgb[idx] = vec4<f32>(color_sum / weight_sum, 1.0);
    } else {
        output_rgb[idx] = vec4<f32>(center_color, 1.0);
    }
}
