use anyhow::Result;
use std::fmt::Write;

pub enum CalibrationPattern {
    Checkerboard {
        rows: usize,
        cols: usize,
        square_size_mm: f32,
    },
    // ChArUco would require logic to draw markers, we will stick to Checkerboard for V1
}

impl CalibrationPattern {
    pub fn generate_svg(&self) -> Result<String> {
        match self {
            CalibrationPattern::Checkerboard {
                rows,
                cols,
                square_size_mm,
            } => {
                let width_mm = *cols as f32 * square_size_mm;
                let height_mm = *rows as f32 * square_size_mm;

                // Add margins (1 square size)
                let margin = *square_size_mm;
                let total_width = width_mm + 2.0 * margin;
                let total_height = height_mm + 2.0 * margin;

                let mut svg = String::new();
                writeln!(
                    svg,
                    r#"<svg width="{}mm" height="{}mm" viewBox="0 0 {} {}" xmlns="http://www.w3.org/2000/svg">"#,
                    total_width, total_height, total_width, total_height
                )?;

                // White background
                writeln!(
                    svg,
                    r#"<rect x="0" y="0" width="{}" height="{}" fill="white" />"#,
                    total_width, total_height
                )?;

                // Draw squares
                for row in 0..*rows {
                    for col in 0..*cols {
                        if (row + col) % 2 == 1 {
                            let x = margin + col as f32 * square_size_mm;
                            let y = margin + row as f32 * square_size_mm;
                            writeln!(
                                svg,
                                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="black" />"#,
                                x, y, square_size_mm, square_size_mm
                            )?;
                        }
                    }
                }

                // Add text info
                writeln!(
                    svg,
                    r#"<text x="{}" y="{}" font-family="Arial" font-size="5" fill="black">Checkerboard {}x{}, {}mm</text>"#,
                    margin,
                    total_height - margin / 2.0,
                    rows,
                    cols,
                    square_size_mm
                )?;

                writeln!(svg, "</svg>")?;
                Ok(svg)
            }
        }
    }
}
