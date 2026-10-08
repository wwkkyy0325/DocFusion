//! Documents built in memory for tests.
//!
//! Shared rather than repeated per test module: two copies of a fixture drift,
//! and the copy that drifts is always the one a test depends on.

use std::io::Cursor;

use image::{DynamicImage, ImageFormat};

/// Builds a minimal but valid single-page PDF, 612×792 points.
///
/// Hand-written rather than produced by a crate, so the repository carries no
/// binary fixtures and a test states exactly what its page contains.
pub fn pdf(entries: &[(f32, &str)]) -> Vec<u8> {
    let mut content = String::new();
    let mut y = 720.0f32;
    for (size, text) in entries {
        content.push_str(&format!(
            "BT /F1 {size} Tf 72 {y:.2} Td ({}) Tj ET\n",
            escape(text)
        ));
        y -= size * 1.8;
    }

    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
            .to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{}endstream",
            content.len(),
            content
        ),
    ];

    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
    }

    let xref_at = pdf.len();
    pdf.push_str(&format!("xref\n0 {}\n", objects.len() + 1));
    pdf.push_str("0000000000 65535 f \n");
    for offset in &offsets {
        pdf.push_str(&format!("{offset:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        objects.len() + 1
    ));

    pdf.into_bytes()
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

/// Builds an encoded PNG of the given size.
pub fn png(width: u32, height: u32) -> Vec<u8> {
    let buffer = image::RgbaImage::from_pixel(width, height, image::Rgba([255, 255, 255, 255]));

    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(buffer)
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encoding a PNG cannot fail");
    bytes.into_inner()
}
