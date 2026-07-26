/// SIMD Vision logic (Feature Gated)
/// Provides accelerated image functions if architecture supports it.
/// Current implementation uses unsafe iterators for auto-vectorization friendly loops.

pub fn difference_keying_simd(base: &[u8], current: &[u8], threshold: u8) -> Vec<u8> {
    if base.len() != current.len() {
        return vec![0; base.len() / 3];
    }

    let len = base.len() / 3;
    let mut mask = Vec::with_capacity(len);
    
    // We use a safe iterator approach that the compiler is very good at vectorizing
    // rather than raw pointers, to ensure memory safety without sacrificing much speed.
    // The previous unsafe block was a bit aggressive.
    
    for (c1, c2) in base.chunks_exact(3).zip(current.chunks_exact(3)) {
        // Simple L1 norm
        let d = (c1[0] as i16 - c2[0] as i16).abs() +
                (c1[1] as i16 - c2[1] as i16).abs() +
                (c1[2] as i16 - c2[2] as i16).abs();
        
        mask.push(if d > threshold as i16 { 255 } else { 0 });
    }
    mask
}
