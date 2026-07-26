use image::{DynamicImage, GenericImage, GenericImageView, Rgba};

/// Merges Cross-Polarized and Parallel-Polarized images
pub fn separate_specular_diffuse(
    parallel: &DynamicImage,
    cross: &DynamicImage,
) -> (DynamicImage, DynamicImage) {
    let (w, h) = parallel.dimensions();
    let mut diffuse = DynamicImage::new_rgba8(w, h);
    let mut specular = DynamicImage::new_rgba8(w, h);

    // Diffuse = Cross Polarized (consists of subsurface scattering which randomizes polarization)
    // Specular = Parallel - Cross (Parallel contains both, Cross contains only diffuse)
    // *Simplified physics, assumes perfect extinction*

    for y in 0..h {
        for x in 0..w {
            let p_par = parallel.get_pixel(x, y);
            let p_cross = cross.get_pixel(x, y);

            // Diffuse IS Cross
            diffuse.put_pixel(x, y, p_cross);

            // Specular = Par - Cross
            let r = p_par[0].saturating_sub(p_cross[0]);
            let g = p_par[1].saturating_sub(p_cross[1]);
            let b = p_par[2].saturating_sub(p_cross[2]);

            specular.put_pixel(x, y, Rgba([r, g, b, 255]));
        }
    }

    (diffuse, specular)
}
