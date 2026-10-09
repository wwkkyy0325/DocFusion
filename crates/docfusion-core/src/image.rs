//! Pages as pixels.
//!
//! A backend that cannot read a document's text reads its image instead, so
//! this is the handover point between the two. Everything here is pure: bytes
//! in, bytes out, with no file access.

use image::{DynamicImage, ImageFormat, ImageReader};
use std::io::Cursor;

/// A page rendered to pixels.
///
/// One representation — RGBA8 — rather than passing encoded files around.
/// Backends disagree about what they accept, and normalising on the way in
/// means each one is written against a single shape instead of each caller
/// working out what that particular backend wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageImage {
    pub width: u32,
    pub height: u32,
    /// RGBA8, row-major: `width * height * 4` bytes.
    pub pixels: Vec<u8>,
}

/// A picture of a number, big and plain, on a page-sized canvas.
///
/// Drawn here rather than shipped as a file: a fixture is a thing that goes
/// missing, or turns out to be the wrong one, or drifts away from what it was a
/// picture of. This is a handful of rectangles and a number nobody can mistake
/// for anything else.
///
/// It exists to ask an endpoint whether it can *see*. A model can be reachable,
/// listed, fast and blind — answering every request with fluent text about
/// nothing — and no amount of asking it which models it serves will say so.
/// Asking it to read a number is the smallest question that can.
pub fn number_page(number: u32) -> PageImage {
    /// Five wide, seven tall, one bit per pixel, most significant bit left.
    const GLYPHS: [[u8; 7]; 10] = [
        [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E], // 0
        [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E], // 1
        [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F], // 2
        [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E], // 3
        [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02], // 4
        [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E], // 5
        [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E], // 6
        [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08], // 7
        [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E], // 8
        [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C], // 9
    ];

    /// How many pixels each bit of a glyph becomes. Large enough that the
    /// number is unmistakable at whatever size a vision encoder reduces it to,
    /// and small enough that the picture is not worth thinking about.
    const SCALE: u32 = 24;
    const GLYPH_WIDTH: u32 = 5;
    const GLYPH_HEIGHT: u32 = 7;
    const SPACING: u32 = 1;

    let digits: Vec<u8> = number
        .to_string()
        .bytes()
        .filter_map(|byte| byte.checked_sub(b'0'))
        .filter(|digit| *digit < 10)
        .collect();

    let columns = digits.len() as u32 * (GLYPH_WIDTH + SPACING);
    let margin = SCALE * 3;

    let width = columns.saturating_sub(SPACING) * SCALE + margin * 2;
    let height = GLYPH_HEIGHT * SCALE + margin * 2;

    // White paper, black ink: the way round every page this tool will ever be
    // shown is.
    let mut pixels = vec![255u8; (width * height * 4) as usize];

    for (position, digit) in digits.iter().enumerate() {
        let glyph = GLYPHS[*digit as usize];
        let left = margin + position as u32 * (GLYPH_WIDTH + SPACING) * SCALE;

        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..GLYPH_WIDTH {
                // Bit 4 is the leftmost of the five.
                if bits & (1 << (GLYPH_WIDTH - 1 - column)) == 0 {
                    continue;
                }

                let x = left + column * SCALE;
                let y = margin + row as u32 * SCALE;
                fill(&mut pixels, width, x, y, SCALE);
            }
        }
    }

    PageImage {
        width,
        height,
        pixels,
    }
}

fn fill(pixels: &mut [u8], width: u32, x: u32, y: u32, size: u32) {
    for dy in 0..size {
        for dx in 0..size {
            let offset = (((y + dy) * width) + x + dx) as usize * 4;
            pixels[offset..offset + 4].copy_from_slice(&[0, 0, 0, 255]);
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("the image could not be decoded: {0}")]
    Decode(#[from] image::ImageError),

    /// The reader and the encoder report plain I/O errors, which the `image`
    /// crate does not fold into its own error type.
    #[error("the image could not be read or written: {0}")]
    Io(String),

    #[error("the image is empty")]
    Empty,

    #[error("the image is {width}×{height}, which exceeds the {limit} pixel limit")]
    TooLarge {
        width: u32,
        height: u32,
        limit: u32,
    },
}

/// The largest number of pixels a single page may have.
///
/// A decompression bomb is a small file that expands to an enormous image, and
/// the limit is checked against the declared dimensions *before* the pixels are
/// allocated — checking afterwards would mean the memory was already spent.
const MAX_PIXELS: u32 = 80_000_000;

impl PageImage {
    /// Decodes an encoded image, whatever format it is in.
    pub fn decode(bytes: &[u8]) -> Result<Self, ImageError> {
        Self::decode_within(bytes, MAX_PIXELS)
    }

    /// Decodes an encoded image, refusing anything above `max_pixels`.
    ///
    /// The limit is a parameter so that it can be exercised with a small real
    /// image rather than a forged header, which would test the forgery as much
    /// as the check.
    pub fn decode_within(bytes: &[u8], max_pixels: u32) -> Result<Self, ImageError> {
        if bytes.is_empty() {
            return Err(ImageError::Empty);
        }

        let reader = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|error| ImageError::Io(error.to_string()))?;
        let (width, height) = reader
            .into_dimensions()
            .map_err(|error| ImageError::Io(error.to_string()))?;

        // Checked from the header, before decoding: a decompression bomb is
        // small on disk and enormous in memory, so checking afterwards would
        // mean the memory was already spent.
        if width.saturating_mul(height) > max_pixels {
            return Err(ImageError::TooLarge {
                width,
                height,
                limit: max_pixels,
            });
        }

        Self::from_dynamic(image::load_from_memory(bytes)?)
    }

    pub fn from_dynamic(image: DynamicImage) -> Result<Self, ImageError> {
        let rgba = image.to_rgba8();
        Ok(Self {
            width: rgba.width(),
            height: rgba.height(),
            pixels: rgba.into_raw(),
        })
    }

    /// Encodes as PNG, which is what a vision backend is handed.
    ///
    /// PNG rather than JPEG because the input is usually text on a plain
    /// background, where JPEG's artefacts land exactly on the strokes that
    /// decide whether a character is recognised.
    pub fn to_png(&self) -> Result<Vec<u8>, ImageError> {
        let buffer = image::RgbaImage::from_raw(self.width, self.height, self.pixels.clone())
            .ok_or(ImageError::Empty)?;

        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(buffer)
            .write_to(&mut png, ImageFormat::Png)
            .map_err(|error| ImageError::Io(error.to_string()))?;
        Ok(png.into_inner())
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_of(width: u32, height: u32, colour: [u8; 4]) -> Vec<u8> {
        let buffer = image::RgbaImage::from_pixel(width, height, image::Rgba(colour));
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(buffer)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn decodes_a_png() {
        let image = PageImage::decode(&png_of(4, 3, [255, 0, 0, 255])).unwrap();

        assert_eq!((image.width, image.height), (4, 3));
        assert_eq!(image.pixels.len(), 4 * 3 * 4);
    }

    #[test]
    fn decodes_a_jpeg() {
        let buffer = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 128, 0, 255]));
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(buffer)
            .write_to(&mut bytes, ImageFormat::Jpeg)
            .unwrap();

        let image = PageImage::decode(&bytes.into_inner()).unwrap();

        assert_eq!((image.width, image.height), (8, 8));
    }

    /// The format is guessed from the bytes, so an image named `.png` that is
    /// really a JPEG is still read — the same rule the rest of the pipeline
    /// follows.
    #[test]
    fn the_format_comes_from_the_bytes_not_a_name() {
        let buffer = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(buffer)
            .write_to(&mut bytes, ImageFormat::Jpeg)
            .unwrap();

        assert!(PageImage::decode(&bytes.into_inner()).is_ok());
    }

    #[test]
    fn survives_a_round_trip_through_png() {
        let original = PageImage::decode(&png_of(3, 2, [10, 20, 30, 255])).unwrap();

        let decoded = PageImage::decode(&original.to_png().unwrap()).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn an_empty_input_is_an_error() {
        assert!(matches!(PageImage::decode(&[]), Err(ImageError::Empty)));
    }

    #[test]
    fn data_that_is_not_an_image_is_an_error() {
        assert!(PageImage::decode(b"this is not an image at all").is_err());
    }

    /// A decompression bomb is small on disk and enormous in memory, so the
    /// declared dimensions are checked before anything is allocated.
    #[test]
    fn an_image_above_the_limit_is_refused() {
        let bytes = png_of(64, 64, [0, 0, 0, 255]);

        assert!(PageImage::decode(&bytes).is_ok(), "the default limit allows it");

        let refusal = PageImage::decode_within(&bytes, 1000);
        assert!(
            matches!(refusal, Err(ImageError::TooLarge { width: 64, height: 64, limit: 1000 })),
            "expected a size refusal, got {refusal:?}"
        );
    }

    #[test]
    fn a_zero_sized_image_reports_itself_as_empty() {
        let image = PageImage {
            width: 0,
            height: 0,
            pixels: Vec::new(),
        };
        assert!(image.is_empty());
    }

    /// The picture has to be a picture: ink on paper, and a number's worth of
    /// it rather than a smudge or a blank page.
    #[test]
    fn the_test_page_is_a_number_drawn_in_ink_on_paper() {
        let page = number_page(4_321);

        assert!(
            page.width > 400 && page.height > 200,
            "{}x{}",
            page.width,
            page.height
        );

        let pixel = |index: usize| page.pixels[index * 4];
        let dark = (0..page.pixels.len() / 4)
            .filter(|index| pixel(*index) == 0)
            .count();
        let light = (0..page.pixels.len() / 4)
            .filter(|index| pixel(*index) == 255)
            .count();

        assert!(dark > 10_000, "only {dark} pixels of ink");
        assert!(light > dark * 4, "the page is mostly ink");
    }

    /// Two different numbers have to look different. The caller shows this to a
    /// model and looks for the number in the answer, so a page that drew the
    /// same thing whatever it was asked for would pass against a model that
    /// reads nothing at all.
    #[test]
    fn different_numbers_are_drawn_differently() {
        assert_ne!(number_page(1_234).pixels, number_page(4_321).pixels);
        assert_ne!(number_page(4_321).pixels, number_page(1_234).pixels);
    }
}
