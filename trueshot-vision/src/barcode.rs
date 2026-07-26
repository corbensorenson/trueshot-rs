use image::{DynamicImage, GrayImage};
use rxing::{BarcodeFormat, DecodeHintType, DecodeHintValue};
use std::collections::{HashMap, HashSet};

/// Scan Setup via Barcode
pub fn detect_project_barcode(img: &DynamicImage) -> Option<String> {
    let mut luma = img.to_luma8();
    for _ in 0..4 {
        if let Some(text) = decode_luma(&luma) {
            return Some(text);
        }
        luma = image::imageops::rotate90(&luma);
    }
    None
}

fn decode_luma(luma: &GrayImage) -> Option<String> {
    let (width, height) = luma.dimensions();
    let data = luma.as_raw().clone();

    let mut hints = HashMap::new();
    hints.insert(DecodeHintType::TRY_HARDER, DecodeHintValue::TryHarder(true));
    hints.insert(DecodeHintType::ALSO_INVERTED, DecodeHintValue::AlsoInverted(true));
    hints.insert(
        DecodeHintType::POSSIBLE_FORMATS,
        DecodeHintValue::PossibleFormats(HashSet::from([
            BarcodeFormat::QR_CODE,
            BarcodeFormat::MICRO_QR_CODE,
            BarcodeFormat::DATA_MATRIX,
            BarcodeFormat::AZTEC,
            BarcodeFormat::PDF_417,
            BarcodeFormat::CODE_128,
            BarcodeFormat::CODE_39,
        ])),
    );

    if let Ok(result) = rxing::helpers::detect_in_luma_with_hints(
        data.clone(),
        width,
        height,
        None,
        &mut hints,
    ) {
        let text = result.getText().trim().to_string();
        if !text.is_empty() {
            return Some(text);
        }
    }

    if let Ok(mut results) =
        rxing::helpers::detect_multiple_in_luma_with_hints(data, width, height, &mut hints)
    {
        results.sort_by_key(|r| r.getNumBits());
        if let Some(best) = results.last() {
            let text = best.getText().trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }

    None
}
