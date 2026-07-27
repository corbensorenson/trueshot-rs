//! Centralized Input Validation Module
//!
//! Provides consistent validation for all TrueShot operations.
//! All validation functions return `Result<(), TrueShotError>` for
//! consistent error handling.

use crate::error::TrueShotError;
use std::path::Path;

/// Validation result type
pub type ValidationResult = Result<(), TrueShotError>;

// ============================================================================
// Path Validation
// ============================================================================

/// Validate that a path exists
pub fn validate_path_exists(path: &Path) -> ValidationResult {
    if !path.exists() {
        return Err(TrueShotError::InvalidState(format!(
            "Path does not exist: {}",
            path.display()
        )));
    }
    Ok(())
}

/// Validate that a path exists and is a file
pub fn validate_file_exists(path: &Path) -> ValidationResult {
    validate_path_exists(path)?;
    if !path.is_file() {
        return Err(TrueShotError::InvalidState(format!(
            "Path is not a file: {}",
            path.display()
        )));
    }
    Ok(())
}

/// Validate that a path exists and is a directory
pub fn validate_directory_exists(path: &Path) -> ValidationResult {
    validate_path_exists(path)?;
    if !path.is_dir() {
        return Err(TrueShotError::InvalidState(format!(
            "Path is not a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

/// Validate that a path is writable (parent exists)
pub fn validate_path_writable(path: &Path) -> ValidationResult {
    let parent = path.parent().unwrap_or(Path::new("."));
    if !parent.exists() {
        return Err(TrueShotError::InvalidState(format!(
            "Parent directory does not exist: {}",
            parent.display()
        )));
    }
    Ok(())
}

/// Validate that a file has a valid image extension
pub fn validate_image_extension(path: &Path) -> ValidationResult {
    let valid_extensions = [
        "jpg", "jpeg", "png", "tiff", "tif", "raw", "dng", "cr2", "cr3", "nef", "arw", "orf",
        "rw2", "pef", "srw",
    ];

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    match ext {
        Some(e) if valid_extensions.contains(&e.as_str()) => Ok(()),
        Some(e) => Err(TrueShotError::InvalidState(format!(
            "Invalid image extension '{}'. Supported: {:?}",
            e, valid_extensions
        ))),
        None => Err(TrueShotError::InvalidState(
            "File has no extension".to_string(),
        )),
    }
}

// ============================================================================
// Numeric Validation
// ============================================================================

/// Validate image dimensions
pub fn validate_image_dimensions(width: u32, height: u32) -> ValidationResult {
    const MIN_DIM: u32 = 16;
    const MAX_DIM: u32 = 65536;

    if !(MIN_DIM..=MAX_DIM).contains(&width) {
        return Err(TrueShotError::InvalidState(format!(
            "Image width {} out of range [{}, {}]",
            width, MIN_DIM, MAX_DIM
        )));
    }

    if !(MIN_DIM..=MAX_DIM).contains(&height) {
        return Err(TrueShotError::InvalidState(format!(
            "Image height {} out of range [{}, {}]",
            height, MIN_DIM, MAX_DIM
        )));
    }

    // Check for excessive memory requirement
    let pixels = width as u64 * height as u64;
    const MAX_PIXELS: u64 = 500_000_000; // 500 megapixels
    if pixels > MAX_PIXELS {
        return Err(TrueShotError::InvalidState(format!(
            "Image too large: {} megapixels (max: {} megapixels)",
            pixels / 1_000_000,
            MAX_PIXELS / 1_000_000
        )));
    }

    Ok(())
}

/// Validate Gaussian count for 3DGS
pub fn validate_gaussian_count(count: usize) -> ValidationResult {
    const MAX_GAUSSIANS: usize = 50_000_000; // 50 million

    if count == 0 {
        return Err(TrueShotError::InvalidState(
            "Gaussian count cannot be zero".to_string(),
        ));
    }

    if count > MAX_GAUSSIANS {
        return Err(TrueShotError::InvalidState(format!(
            "Too many Gaussians: {} (max: {})",
            count, MAX_GAUSSIANS
        )));
    }

    Ok(())
}

/// Validate mesh vertex count
pub fn validate_mesh_vertex_count(count: usize) -> ValidationResult {
    const MAX_VERTICES: usize = 100_000_000; // 100 million

    if count == 0 {
        return Err(TrueShotError::InvalidState(
            "Mesh must have at least one vertex".to_string(),
        ));
    }

    if count > MAX_VERTICES {
        return Err(TrueShotError::InvalidState(format!(
            "Too many vertices: {} (max: {})",
            count, MAX_VERTICES
        )));
    }

    Ok(())
}

/// Validate memory availability
pub fn validate_memory_available(needed_gb: f64) -> ValidationResult {
    let available_gb =
        crate::resource_manager::available_memory_bytes() as f64 / (1024.0 * 1024.0 * 1024.0);

    // Require 20% safety margin
    let required_with_margin = needed_gb * 1.2;

    if available_gb < required_with_margin {
        return Err(TrueShotError::InvalidState(format!(
            "Insufficient memory: need {:.1} GB but only {:.1} GB available",
            required_with_margin, available_gb
        )));
    }

    Ok(())
}

// ============================================================================
// Range Validation
// ============================================================================

/// Validate a value is within a range
pub fn validate_range<T: PartialOrd + std::fmt::Display>(
    value: T,
    min: T,
    max: T,
    name: &str,
) -> ValidationResult {
    if value < min || value > max {
        return Err(TrueShotError::InvalidState(format!(
            "{} value {} out of range [{}, {}]",
            name, value, min, max
        )));
    }
    Ok(())
}

/// Validate a normalized value is in [0.0, 1.0]
pub fn validate_normalized(value: f64, name: &str) -> ValidationResult {
    validate_range(value, 0.0, 1.0, name)
}

/// Validate a positive value
pub fn validate_positive<T: PartialOrd + Default + std::fmt::Display>(
    value: T,
    name: &str,
) -> ValidationResult {
    if value <= T::default() {
        return Err(TrueShotError::InvalidState(format!(
            "{} must be positive, got {}",
            name, value
        )));
    }
    Ok(())
}

// ============================================================================
// Collection Validation
// ============================================================================

/// Validate a collection is not empty
pub fn validate_not_empty<T>(collection: &[T], name: &str) -> ValidationResult {
    if collection.is_empty() {
        return Err(TrueShotError::InvalidState(format!(
            "{} cannot be empty",
            name
        )));
    }
    Ok(())
}

/// Validate minimum collection size
pub fn validate_min_count<T>(collection: &[T], min: usize, name: &str) -> ValidationResult {
    if collection.len() < min {
        return Err(TrueShotError::InvalidState(format!(
            "{} requires at least {} items, got {}",
            name,
            min,
            collection.len()
        )));
    }
    Ok(())
}

// ============================================================================
// Composite Validators
// ============================================================================

/// Validate input for photogrammetry pipeline
pub fn validate_photogrammetry_input(image_dir: &Path, min_images: usize) -> ValidationResult {
    validate_directory_exists(image_dir)?;

    // Count valid images
    let image_count = std::fs::read_dir(image_dir)
        .map_err(|e| TrueShotError::Io(format!("Failed to read directory: {}", e)))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    let lower = e.to_lowercase();
                    ["jpg", "jpeg", "png", "tiff", "tif"].contains(&lower.as_str())
                })
                .unwrap_or(false)
        })
        .count();

    if image_count < min_images {
        return Err(TrueShotError::InvalidState(format!(
            "Not enough images: found {}, need at least {}",
            image_count, min_images
        )));
    }

    Ok(())
}

/// Validate input for Gaussian splatting
pub fn validate_gaussian_splatting_input(
    image_count: usize,
    target_gaussians: usize,
) -> ValidationResult {
    validate_min_count(&vec![(); image_count], 4, "images")?;
    validate_gaussian_count(target_gaussians)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_validate_path_exists() {
        let dir = tempdir().unwrap();
        assert!(validate_path_exists(dir.path()).is_ok());
        assert!(validate_path_exists(Path::new("/nonexistent/path")).is_err());
    }

    #[test]
    fn test_validate_image_dimensions() {
        assert!(validate_image_dimensions(1920, 1080).is_ok());
        assert!(validate_image_dimensions(8256, 5504).is_ok());
        assert!(validate_image_dimensions(0, 100).is_err());
        assert!(validate_image_dimensions(100000, 100000).is_err());
    }

    #[test]
    fn test_validate_gaussian_count() {
        assert!(validate_gaussian_count(1000).is_ok());
        assert!(validate_gaussian_count(1_000_000).is_ok());
        assert!(validate_gaussian_count(0).is_err());
        assert!(validate_gaussian_count(100_000_000).is_err());
    }

    #[test]
    fn test_validate_range() {
        assert!(validate_range(5, 0, 10, "test").is_ok());
        assert!(validate_range(15, 0, 10, "test").is_err());
    }

    #[test]
    fn test_validate_not_empty() {
        assert!(validate_not_empty(&[1, 2, 3], "test").is_ok());
        let empty: Vec<i32> = vec![];
        assert!(validate_not_empty(&empty, "test").is_err());
    }
}
