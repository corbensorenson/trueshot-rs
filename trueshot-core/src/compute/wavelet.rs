/// Wavelet Denoise (Haar, 1-level)
/// For RAW data (f32).
///
/// Performs a full forward transform, soft-thresholds detail bands (LH/HL/HH),
/// then reconstructs via inverse transform. Handles odd dimensions by leaving
/// the last row/column untouched.
pub fn denoise_wavelet_haar(data: &mut [f32], width: usize, height: usize, threshold: f32) {
    if width < 2 || height < 2 {
        return;
    }
    if data.len() < width * height {
        return;
    }

    let w2 = width / 2;
    let h2 = height / 2;
    let mut temp = vec![0.0f32; width * height];
    let mut coeffs = vec![0.0f32; width * height];

    // Preserve edges for odd dimensions.
    let preserve_edges = (width % 2 != 0) || (height % 2 != 0);
    let original = if preserve_edges {
        Some(data.to_vec())
    } else {
        None
    };

    // Forward transform: rows
    for y in 0..height {
        let row_start = y * width;
        for x in 0..w2 {
            let a = data[row_start + 2 * x];
            let b = data[row_start + 2 * x + 1];
            let avg = 0.5 * (a + b);
            let diff = 0.5 * (a - b);
            temp[row_start + x] = avg;
            temp[row_start + w2 + x] = diff;
        }
        if width % 2 != 0 {
            temp[row_start + width - 1] = data[row_start + width - 1];
        }
    }

    // Forward transform: columns
    for x in 0..width {
        for y in 0..h2 {
            let a = temp[(2 * y) * width + x];
            let b = temp[(2 * y + 1) * width + x];
            let avg = 0.5 * (a + b);
            let diff = 0.5 * (a - b);
            coeffs[y * width + x] = avg;
            coeffs[(h2 + y) * width + x] = diff;
        }
        if height % 2 != 0 {
            coeffs[(height - 1) * width + x] = temp[(height - 1) * width + x];
        }
    }

    // Threshold detail sub-bands (LH, HL, HH)
    for y in 0..height {
        for x in 0..width {
            if y < h2 && x < w2 {
                continue; // LL
            }
            let idx = y * width + x;
            let v = coeffs[idx];
            coeffs[idx] = if v.abs() < threshold {
                0.0
            } else {
                v - v.signum() * threshold
            };
        }
    }

    // Inverse transform: columns
    for x in 0..width {
        for y in 0..h2 {
            let avg = coeffs[y * width + x];
            let diff = coeffs[(h2 + y) * width + x];
            temp[(2 * y) * width + x] = avg + diff;
            temp[(2 * y + 1) * width + x] = avg - diff;
        }
        if height % 2 != 0 {
            temp[(height - 1) * width + x] = coeffs[(height - 1) * width + x];
        }
    }

    // Inverse transform: rows
    for y in 0..height {
        let row_start = y * width;
        for x in 0..w2 {
            let avg = temp[row_start + x];
            let diff = temp[row_start + w2 + x];
            data[row_start + 2 * x] = avg + diff;
            data[row_start + 2 * x + 1] = avg - diff;
        }
        if width % 2 != 0 {
            data[row_start + width - 1] = temp[row_start + width - 1];
        }
    }

    if let Some(orig) = original {
        if width % 2 != 0 {
            for y in 0..height {
                data[y * width + width - 1] = orig[y * width + width - 1];
            }
        }
        if height % 2 != 0 {
            let row_start = (height - 1) * width;
            data[row_start..row_start + width].copy_from_slice(&orig[row_start..row_start + width]);
        }
    }
}
