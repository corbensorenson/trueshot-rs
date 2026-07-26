// Shader for Heatmap Computation
struct Point {
    min: vec3<f32>,
    padding: f32,
};

struct VoxelGrid {
    min_bound: vec3<f32>,
    max_bound: vec3<f32>,
    grid_size: vec3<u32>,
    voxel_size: f32,
}

@group(0) @binding(0) var<storage, read> points: array<Point>;
@group(0) @binding(1) var<storage, read_write> density: array<atomic<u32>>;
@group(0) @binding(2) var<uniform> grid_params: VoxelGrid;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let index = global_id.x;
    if (index >= arrayLength(&points)) {
        return;
    }

    let p = points[index].min;
    
    // Calculate Voxel Index
    let vx = u32((p.x - grid_params.min_bound.x) / grid_params.voxel_size);
    let vy = u32((p.y - grid_params.min_bound.y) / grid_params.voxel_size);
    let vz = u32((p.z - grid_params.min_bound.z) / grid_params.voxel_size);
    
    // Bounds Check
    if (vx >= grid_params.grid_size.x || vy >= grid_params.grid_size.y || vz >= grid_params.grid_size.z) {
        return;
    }

    let flat_index = vx + (vy * grid_params.grid_size.x) + (vz * grid_params.grid_size.x * grid_params.grid_size.y);
    
    // Atomic Add
    atomicAdd(&density[flat_index], 1u);
}
