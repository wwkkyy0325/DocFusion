//! Local OCR, running in the process through ONNX Runtime.
//!
//! This is the engine that works with no setup at all, and the one that
//! verifies everything else: it is local, free, and fast enough to run on every
//! page, which are the properties a verifier has to have.
//!
//! The pipeline — text detection, optional angle classification, recognition —
//! comes from `rapidocr-core`. Writing DBNet post-processing, contour finding
//! and CTC decoding by hand would be a large amount of well-trodden work with
//! no decisions of our own in it.
//!
//! Lives here rather than in the core crate because it loads native libraries
//! and model files, and core performs no I/O.

use std::sync::Mutex;

use docfusion_core::backend::{BackendError, Caps, OcrBackend, PageRequest};
use docfusion_core::image::PageImage;
use docfusion_core::ir::{Block, BlockKind, EngineId, Rect};
use rapidocr_core::RapidOcr;
use rapidocr_core::config::RapidOcrConfig;
use rapidocr_core::types::{OcrLine, OcrOutput};

const ENGINE_ID: &str = "rapidocr";
const ENGINE_VERSION: &str = "1.0.0";

/// Two lines belong to the same paragraph when the vertical gap between them is
/// under this fraction of their height.
///
/// The recogniser reports lines, not paragraphs, so without this a wrapped
/// sentence becomes one line per row — correct words, wrong shape.
const PARAGRAPH_GAP: f32 = 0.6;

pub struct RapidOcrBackend {
    /// `run_image` takes `&mut self` and a pipeline is expensive to build, so
    /// one is kept behind a lock rather than rebuilt per page.
    runner: Mutex<RapidOcr>,
}

impl RapidOcrBackend {
    /// Builds the pipeline from a configuration naming where its models are.
    pub fn from_config(config: RapidOcrConfig) -> Result<Self, BackendError> {
        let runner = RapidOcr::new(config).map_err(|error| {
            BackendError::Unavailable(format!("the OCR models could not be loaded: {error}"))
        })?;

        Ok(Self {
            runner: Mutex::new(runner),
        })
    }
}

impl OcrBackend for RapidOcrBackend {
    fn id(&self) -> EngineId {
        EngineId::from(ENGINE_ID)
    }

    fn version(&self) -> &str {
        ENGINE_VERSION
    }

    /// Both, unlike a general vision model.
    ///
    /// The detector reports where each line sits and the recogniser reports how
    /// sure it is, which is what makes this engine usable as a verifier rather
    /// than only as a reader.
    fn capabilities(&self) -> Caps {
        Caps {
            bounding_boxes: true,
            confidence: true,
        }
    }

    fn recognize(&self, request: &PageRequest<'_>) -> Result<Vec<Block>, BackendError> {
        let rgb = to_rgb(request.image);

        let mut runner = self
            .runner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let output = runner.run_image(&rgb).map_err(|error| {
            BackendError::Failed(format!(
                "the OCR pipeline failed on page {}: {error}",
                request.page
            ))
        })?;

        Ok(lines_to_blocks(
            &output,
            request.image.width,
            request.image.height,
            &self.id(),
        ))
    }
}

fn to_rgb(image: &PageImage) -> image::RgbImage {
    let mut rgb = image::RgbImage::new(image.width, image.height);
    for (pixel, channels) in rgb.pixels_mut().zip(image.pixels.chunks_exact(4)) {
        *pixel = image::Rgb([channels[0], channels[1], channels[2]]);
    }
    rgb
}

/// Turns recognized lines into blocks.
///
/// Pure, and deliberately separate from the model call. Ordering, grouping and
/// geometry are where the decisions are, and none of them should need fifteen
/// megabytes of weights on disk to be tested.
fn lines_to_blocks(output: &OcrOutput, width: u32, height: u32, engine: &EngineId) -> Vec<Block> {
    let mut lines: Vec<&OcrLine> = output
        .lines
        .iter()
        .filter(|line| !line.text.trim().is_empty())
        .collect();

    // Reading order: top to bottom, then left to right. The recogniser returns
    // them in detection order, which is not the order a person reads.
    lines.sort_by(|left, right| {
        let (left_top, left_left) = (top_of(left), left_of(left));
        let (right_top, right_left) = (top_of(right), left_of(right));
        left_top
            .total_cmp(&right_top)
            .then(left_left.total_cmp(&right_left))
    });

    let mut blocks: Vec<Block> = Vec::new();

    for line in lines {
        let bbox = normalized(line, width, height);

        let continues = blocks
            .last()
            .and_then(|previous| previous.bbox.zip(bbox))
            .is_some_and(|(previous, current)| continues_paragraph(previous, current));

        if continues
            && let Some(previous) = blocks.last_mut()
        {
            previous.text.push('\n');
            previous.text.push_str(line.text.trim());

            // The paragraph's box grows to cover what it now contains.
            if let (Some(mut covered), Some(current)) = (previous.bbox, bbox) {
                covered.union(&current);
                previous.bbox = Some(covered);
            }
            continue;
        }

        blocks.push(Block {
            kind: BlockKind::Paragraph,
            text: line.text.trim().to_owned(),
            bbox,
            confidence: Some(line.score.clamp(0.0, 1.0)),
            engine: engine.clone(),
            children: Vec::new(),
        });
    }

    blocks
}

/// Whether a line continues the paragraph that came before it.
///
/// Three conditions, and each one is doing work:
///
/// - **Just below it.** A line far above or far below starts something new.
/// - **Close to it.** The gap is a fraction of the line height, so a new
///   paragraph is separated by its own spacing.
/// - **Sharing horizontal space.** This is the one that is easy to leave out,
///   and the reason two lines sitting side by side in different columns were
///   being read as one wrapped sentence.
fn continues_paragraph(previous: Rect, current: Rect) -> bool {
    let gap = current.y - (previous.y + previous.h);

    gap >= -current.h * 0.5
        && gap <= current.h.max(previous.h) * PARAGRAPH_GAP
        && horizontal_overlap(previous, current) > 0.0
}

fn horizontal_overlap(left: Rect, right: Rect) -> f32 {
    let start = left.x.max(right.x);
    let end = (left.x + left.w).min(right.x + right.w);
    (end - start).max(0.0)
}

fn top_of(line: &OcrLine) -> f32 {
    line.bbox.points.iter().map(|point| point[1]).fold(f32::INFINITY, f32::min)
}

fn left_of(line: &OcrLine) -> f32 {
    line.bbox.points.iter().map(|point| point[0]).fold(f32::INFINITY, f32::min)
}

/// The box a quad occupies, in the IR's normalised coordinates.
///
/// A quad is four arbitrary corners, because detection returns rotated boxes.
/// The IR wants an axis-aligned rectangle, so the corners are reduced to their
/// extent — which is the right answer for near-horizontal text such as a page
/// of prose, and the honest one for the rest.
fn normalized(line: &OcrLine, width: u32, height: u32) -> Option<Rect> {
    if width == 0 || height == 0 {
        return None;
    }

    let xs = line.bbox.points.map(|point| point[0]);
    let ys = line.bbox.points.map(|point| point[1]);

    let left = xs.iter().copied().fold(f32::INFINITY, f32::min);
    let right = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let top = ys.iter().copied().fold(f32::INFINITY, f32::min);
    let bottom = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);

    Some(Rect::new(
        (left / width as f32).clamp(0.0, 1.0),
        (top / height as f32).clamp(0.0, 1.0),
        ((right - left) / width as f32).clamp(0.0, 1.0),
        ((bottom - top) / height as f32).clamp(0.0, 1.0),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rapidocr_core::types::Quad;

    fn line(text: &str, x0: f32, y0: f32, x1: f32, y1: f32, score: f32) -> OcrLine {
        OcrLine {
            bbox: Quad::from_xyxy(x0, y0, x1, y1),
            text: text.to_owned(),
            score,
        }
    }

    fn output(lines: Vec<OcrLine>) -> OcrOutput {
        OcrOutput { lines }
    }

    fn blocks(lines: Vec<OcrLine>) -> Vec<Block> {
        lines_to_blocks(
            &output(lines),
            100,
            200,
            &EngineId::from(ENGINE_ID),
        )
    }

    #[test]
    fn produces_one_block_per_line_when_they_are_far_apart() {
        let blocks = blocks(vec![
            line("first", 0.0, 0.0, 50.0, 10.0, 0.9),
            line("second", 0.0, 100.0, 50.0, 110.0, 0.9),
        ]);

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].text, "first");
        assert_eq!(blocks[1].text, "second");
    }

    /// The recogniser reports lines, not paragraphs, so without merging a
    /// wrapped sentence becomes one block per row: right words, wrong shape.
    #[test]
    fn lines_that_almost_touch_are_one_paragraph() {
        let blocks = blocks(vec![
            line("a sentence that", 0.0, 0.0, 80.0, 10.0, 0.9),
            line("wraps onto a second line", 0.0, 12.0, 90.0, 22.0, 0.9),
        ]);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text, "a sentence that\nwraps onto a second line");
    }

    #[test]
    fn a_paragraph_box_covers_every_line_in_it() {
        let blocks = blocks(vec![
            line("first", 10.0, 0.0, 50.0, 10.0, 0.9),
            line("second", 5.0, 12.0, 80.0, 22.0, 0.9),
        ]);

        let bbox = blocks[0].bbox.expect("bbox");

        assert!((bbox.x - 0.05).abs() < 1e-6, "{bbox:?}");
        assert!((bbox.y - 0.0).abs() < 1e-6, "{bbox:?}");
        assert!((bbox.w - 0.75).abs() < 1e-6, "{bbox:?}");
        assert!((bbox.h - 0.11).abs() < 1e-6, "{bbox:?}");
    }

    /// Detection returns them in whatever order it found them, which is not the
    /// order a person reads.
    #[test]
    fn blocks_come_back_in_reading_order() {
        let blocks = blocks(vec![
            line("bottom", 0.0, 100.0, 50.0, 110.0, 0.9),
            line("top", 0.0, 0.0, 50.0, 10.0, 0.9),
        ]);

        assert_eq!(blocks[0].text, "top");
        assert_eq!(blocks[1].text, "bottom");
    }

    /// Two lines on the same row are two columns, not one wrapped sentence.
    /// Vertical distance alone cannot tell the difference; horizontal overlap
    /// can.
    #[test]
    fn lines_side_by_side_stay_separate() {
        let blocks = blocks(vec![
            line("right", 60.0, 0.0, 90.0, 10.0, 0.9),
            line("left", 0.0, 0.0, 30.0, 10.0, 0.9),
        ]);

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].text, "left");
        assert_eq!(blocks[1].text, "right");
    }

    #[test]
    fn a_heading_above_a_paragraph_is_not_absorbed_into_it() {
        let blocks = blocks(vec![
            line("Heading", 0.0, 0.0, 40.0, 12.0, 0.9),
            line("Body text follows some way below", 0.0, 40.0, 90.0, 50.0, 0.9),
        ]);

        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn geometry_is_normalised_to_the_page() {
        let blocks = blocks(vec![line("half", 0.0, 0.0, 50.0, 100.0, 0.9)]);
        let bbox = blocks[0].bbox.expect("bbox");

        assert!((bbox.x - 0.0).abs() < 1e-6, "{bbox:?}");
        assert!((bbox.w - 0.5).abs() < 1e-6, "{bbox:?}");
        assert!((bbox.h - 0.5).abs() < 1e-6, "{bbox:?}");
    }

    #[test]
    fn confidence_is_carried_through() {
        let blocks = blocks(vec![line("text", 0.0, 0.0, 50.0, 10.0, 0.83)]);

        assert_eq!(blocks[0].confidence, Some(0.83));
    }

    #[test]
    fn blank_lines_are_dropped() {
        let blocks = blocks(vec![
            line("   ", 0.0, 0.0, 50.0, 10.0, 0.9),
            line("real", 0.0, 100.0, 50.0, 110.0, 0.9),
        ]);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text, "real");
    }

    #[test]
    fn a_page_with_no_text_produces_no_blocks() {
        assert!(blocks(Vec::new()).is_empty());
    }

    #[test]
    fn every_block_is_attributed_to_this_engine() {
        let blocks = blocks(vec![line("text", 0.0, 0.0, 50.0, 10.0, 0.9)]);

        assert_eq!(blocks[0].engine.as_str(), ENGINE_ID);
    }

    /// Reads a page through the real models.
    ///
    /// Ignored by default because it needs weights on disk, which a unit test
    /// has no business fetching. Run it with a model directory:
    ///
    /// ```text
    /// DOCFUSION_OCR_MODELS=<dir> cargo test -p docfusion-store -- --ignored
    /// ```
    ///
    /// It renders a PDF whose text is known rather than shipping a test image,
    /// so the expected answer is stated in the test instead of trusted to a
    /// binary that nobody can read.
    #[test]
    #[ignore = "needs OCR models; set DOCFUSION_OCR_MODELS to a directory holding them"]
    fn reads_text_from_a_rendered_page() {
        use docfusion_core::extract::fixtures;
        use rapidocr_core::model::{ModelCache, model_set_by_name};

        let directory = std::env::var("DOCFUSION_OCR_MODELS").expect("DOCFUSION_OCR_MODELS");
        let model_set = model_set_by_name("ppocrv6-tiny").expect("a registered model set");
        let backend = RapidOcrBackend::from_config(ModelCache::new(&directory).config_for(model_set))
            .expect("the models should load");

        // Large type, so that what is being tested is the pipeline rather than
        // the resolution a page happens to be rendered at.
        let pdf = fixtures::pdf(&[(48.0, "Hello world")]);
        let page = docfusion_core::extract::pdf::render_pages(&pdf, 200.0)
            .expect("render")
            .remove(0);

        let blocks = backend
            .recognize(&PageRequest { image: &page, page: 0 })
            .expect("recognize");

        let text: String = blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        assert!(
            text.to_lowercase().contains("hello"),
            "expected the page's own words back, got: {text:?}"
        );
        assert!(
            blocks.iter().all(|block| block.bbox.is_some()),
            "every block should carry geometry"
        );
    }
}
