use anyhow::Result;
use image::DynamicImage;
use rxing::{BarcodeFormat, DecodeHintType, DecodeHintValue};
use std::collections::{HashMap, HashSet};

/// Fiducial Tracking (QR Codes / Markers)
/// Used to precisely determine turntable angle.
pub struct FiducialTracker;

#[derive(Debug)]
pub struct Fiducial {
    pub id: String,
    pub center: (f32, f32),
    pub angle: Option<f32>,
}

impl FiducialTracker {
    pub fn detect(img: &DynamicImage) -> Result<Vec<Fiducial>> {
        let gray = img.to_luma8();
        let (width, height) = gray.dimensions();
        let data = gray.as_raw().clone();

        let mut hints = HashMap::new();
        hints.insert(DecodeHintType::TRY_HARDER, DecodeHintValue::TryHarder(true));
        hints.insert(
            DecodeHintType::ALSO_INVERTED,
            DecodeHintValue::AlsoInverted(true),
        );
        hints.insert(
            DecodeHintType::POSSIBLE_FORMATS,
            DecodeHintValue::PossibleFormats(HashSet::from([
                BarcodeFormat::QR_CODE,
                BarcodeFormat::MICRO_QR_CODE,
                BarcodeFormat::DATA_MATRIX,
                BarcodeFormat::AZTEC,
                BarcodeFormat::PDF_417,
            ])),
        );

        let results =
            rxing::helpers::detect_multiple_in_luma_with_hints(data, width, height, &mut hints)
                .unwrap_or_default();

        let mut out = Vec::new();
        for result in results {
            let text = result.getText().trim().to_string();
            if text.is_empty() {
                continue;
            }
            let points = result.getPoints();
            let (mut cx, mut cy) = (0.0f32, 0.0f32);
            if !points.is_empty() {
                for p in points {
                    cx += p.x;
                    cy += p.y;
                }
                let denom = points.len() as f32;
                cx /= denom;
                cy /= denom;
            }

            let angle = if points.len() >= 2 {
                let dx = points[1].x - points[0].x;
                let dy = points[1].y - points[0].y;
                Some(dy.atan2(dx).to_degrees())
            } else {
                None
            };

            out.push(Fiducial {
                id: text,
                center: (cx, cy),
                angle,
            });
        }

        Ok(out)
    }
}
