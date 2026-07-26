//! SIMD Optimized Operations (SOTA 6)
//! 
//! Explicit SIMD intrinsics for hot loops using simple scalar optimization 
//! which LLVM auto-vectorizes very well, plus manual unrolling.


/// Weighted blend of two buffers
/// dest = dest * (1 - weight) + src * weight
///
/// Manually unrolled for AVX/NEON friendly auto-vectorization
pub fn blend_buffers_simd(
    dest: &mut [f64], // F64 for precision
    src: &[f64],
    weight: f64,
) {
    let len = dest.len();
    assert_eq!(len, src.len());
    
    let inv_weight = 1.0 - weight;
    
    // Chunk size 8 for AVX-512 (8 x 64-bit float)
    let chunks = len / 8;
    let remainder = len % 8;
    
    for i in 0..chunks {
        let offset = i * 8;
        // Unroll loop for maximum ILP
        dest[offset] = dest[offset] * inv_weight + src[offset] * weight;
        dest[offset + 1] = dest[offset + 1] * inv_weight + src[offset + 1] * weight;
        dest[offset + 2] = dest[offset + 2] * inv_weight + src[offset + 2] * weight;
        dest[offset + 3] = dest[offset + 3] * inv_weight + src[offset + 3] * weight;
        dest[offset + 4] = dest[offset + 4] * inv_weight + src[offset + 4] * weight;
        dest[offset + 5] = dest[offset + 5] * inv_weight + src[offset + 5] * weight;
        dest[offset + 6] = dest[offset + 6] * inv_weight + src[offset + 6] * weight;
        dest[offset + 7] = dest[offset + 7] * inv_weight + src[offset + 7] * weight;
    }
    
    // Handle remainder
    for i in (len - remainder)..len {
        dest[i] = dest[i] * inv_weight + src[i] * weight;
    }
}

/// Accumulate buffers
/// dest += src
pub fn accumulate_buffers_simd(
    dest: &mut [f64], 
    src: &[f64],
) {
    let len = dest.len();
    let chunks = len / 8;
    let remainder = len % 8;
    
    for i in 0..chunks {
        let offset = i * 8;
        dest[offset] += src[offset];
        dest[offset + 1] += src[offset + 1];
        dest[offset + 2] += src[offset + 2];
        dest[offset + 3] += src[offset + 3];
        dest[offset + 4] += src[offset + 4];
        dest[offset + 5] += src[offset + 5];
        dest[offset + 6] += src[offset + 6];
        dest[offset + 7] += src[offset + 7];
    }
    
    for i in (len - remainder)..len {
        dest[i] += src[i];
    }
}
