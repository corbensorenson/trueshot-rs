
/// Metadata Sidecar (.xmp) Writer
/// Writes standard XMP data for photogrammetry (lighting, position)
pub fn write_xmp_sidecar(
    path: &std::path::Path,
    params: &SidecarParams
) -> std::io::Result<()> {
    let xmp_content = format!(
r#"<?xml version="1.0" encoding="UTF-8"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="TrueShot Core 6.0">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:trueshot="http://augment.tech/trueshot/1.0/"
    xmlns:exif="http://ns.adobe.com/exif/1.0/">
   <trueshot:StepperPosition>{}</trueshot:StepperPosition>
   <trueshot:LightingState>{}</trueshot:LightingState>
   <trueshot:CameraID>{}</trueshot:CameraID>
   <exif:DateTimeOriginal>{}</exif:DateTimeOriginal>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#,
        params.stepper_pos,
        params.lighting_state,
        params.camera_id,
        params.timestamp
    );

    std::fs::write(path.with_extension("xmp"), xmp_content)
}

pub struct SidecarParams {
    pub stepper_pos: f32, // Corrected field name
    pub lighting_state: String,
    pub camera_id: String,
    pub timestamp: String,
}
