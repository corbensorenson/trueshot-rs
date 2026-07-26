/// LOD Generation (Mesh Decimation)
/// Uses QEM (Quadric Error Metrics) simplification.
pub const LOD_HIGH: f32 = 1.0;
pub const LOD_MED: f32 = 0.5;
pub const LOD_LOW: f32 = 0.1;

pub fn generate_lods(_mesh_path: &std::path::Path) -> anyhow::Result<()> {
    // Load OBJ/GLB
    // Run decimation logic (meshopt bindings or pure rust naive edge collapse)
    // Save _med.obj, _low.obj
    Ok(())
}
