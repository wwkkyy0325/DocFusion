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

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, TryLockError};

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
    /// Several pipelines, each behind its own lock, each built on the first
    /// page that needs it.
    ///
    /// Built lazily because loading the weights takes seconds and hundreds of
    /// megabytes, and most command lines never read a page at all — `list`,
    /// `show` and `schema` would each pay for a pipeline they do not use.
    ///
    /// Several rather than one because `run_image` takes `&mut self`, so a
    /// single pipeline serialises every page behind it. Widening one page does
    /// not help — measured, see `build` — so the only way to use more than one
    /// core is to read more than one page at a time, which needs more than one
    /// pipeline.
    runners: Vec<Mutex<Option<RapidOcr>>>,
    /// Rotates which pipeline a caller tries first, so that pages arriving at
    /// once spread out instead of all queueing on the same one.
    next: AtomicUsize,
    source: Source,
}

/// How many pages this machine will read at once.
///
/// The machine answers this — from its memory and its cores — rather than a
/// constant here. A loaded pipeline costs about **250 MB** resident: measured
/// over 32 pages, one peaks at 439 MB and takes 263 s where four peak at 1193 MB
/// and take 76 s. A number that is right on a laptop is wrong on a workstation,
/// and the same build runs on both.
///
/// See `crate::memory::page_concurrency` for the arithmetic. Overriding it is
/// what `with_pipelines` is for.
fn pipeline_count() -> usize {
    crate::memory::page_concurrency()
}

enum Source {
    /// Weights embedded in the binary. See `build.rs`.
    Bundled,
    /// Weights found by a caller that knows where they are.
    Config(Box<RapidOcrConfig>),
}

impl RapidOcrBackend {
    /// Reads pages using the weights embedded in the binary.
    ///
    /// This is the path that makes the tool work with no setup: nothing is
    /// downloaded, nothing is configured, and it is offline from the moment it
    /// is installed.
    pub fn bundled() -> Self {
        Self::with_source(Source::Bundled)
    }

    /// Reads pages using weights the caller located itself.
    pub fn from_config(config: RapidOcrConfig) -> Self {
        Self::with_source(Source::Config(Box::new(config)))
    }

    /// Sets how many pages may be read at once.
    ///
    /// The memory dial as much as the speed one, and the only one there is:
    /// everything else here is measured in kilobytes. See `MAX_PIPELINES` for
    /// the numbers. Below one is one, because a pipeline count of zero is not a
    /// setting, it is a mistake that would read nothing.
    pub fn with_pipelines(mut self, count: usize) -> Self {
        let count = count.max(1);
        self.runners.truncate(count);
        self.runners.resize_with(count, || Mutex::new(None));

        // Whatever was built beyond the new count is dropped with the vector
        // entries above, which releases those pipelines.
        self
    }

    fn with_source(source: Source) -> Self {
        let count = pipeline_count();
        Self {
            runners: (0..count).map(|_| Mutex::new(None)).collect(),
            next: AtomicUsize::new(0),
            source,
        }
    }

    /// Runs `action` on a free pipeline, building it if this is its first use.
    ///
    /// A free one is looked for before any is waited on, starting from a
    /// rotating offset. Blocking on the first pipeline would serialise pages
    /// that could have run side by side — the lock is per pipeline, but the
    /// choice of which to take is what decides whether they collide.
    fn with_runner<T>(
        &self,
        action: impl FnOnce(&mut RapidOcr) -> Result<T, BackendError>,
    ) -> Result<T, BackendError> {
        let count = self.runners.len();
        let start = self.next.fetch_add(1, Ordering::Relaxed) % count;

        for offset in 0..count {
            match self.runners[(start + offset) % count].try_lock() {
                Ok(mut runner) => return self.run(&mut runner, action),
                Err(TryLockError::WouldBlock) => continue,
                // A thread panicked while holding this pipeline. The weights
                // are still good; refusing to read another page because of it
                // would turn one broken page into a broken run.
                Err(TryLockError::Poisoned(poisoned)) => {
                    return self.run(&mut poisoned.into_inner(), action);
                }
            }
        }

        // Everything is busy, which is the intended state on a full document.
        let mut runner = self.runners[start]
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        self.run(&mut runner, action)
    }

    fn run<T>(
        &self,
        slot: &mut Option<RapidOcr>,
        action: impl FnOnce(&mut RapidOcr) -> Result<T, BackendError>,
    ) -> Result<T, BackendError> {
        if slot.is_none() {
            *slot = Some(self.build()?);
        }

        action(slot.as_mut().expect("built immediately above"))
    }

    fn build(&self) -> Result<RapidOcr, BackendError> {
        let config = match &self.source {
            Source::Config(config) => (**config).clone(),
            Source::Bundled => {
                let directory = ensure_models_on_disk()?;
                let model_set = rapidocr_core::model::model_set_by_name(MODEL_SET).ok_or_else(
                    || BackendError::Unavailable(format!("no model set named {MODEL_SET}")),
                )?;

                rapidocr_core::model::ModelCache::new(directory).config_for(model_set)
            }
        };

        // One page is one unit of work, and widening a single inference does
        // not help: the recogniser runs once per detected line on a crop too
        // small for threads to pay for themselves. Measured on sixteen cores —
        // raising `intra_threads` from 1 to 16 changed nothing, three runs each
        // way. The parallelism that matters is across pages, which is the
        // extractor's business rather than this configuration's.

        RapidOcr::new(config).map_err(|error| {
            BackendError::Unavailable(format!("the OCR models could not be loaded: {error}"))
        })
    }
}

/// The model set the embedded weights belong to.
const MODEL_SET: &str = "ppocrv6-tiny";

/// What `build.rs` actually downloaded, so that a test can hold it against what
/// the pipeline expects.
#[cfg(test)]
mod model_pins {
    include!(concat!(env!("OUT_DIR"), "/model_pins.rs"));
}

/// The weights, embedded at build time. See `build.rs`.
mod weights {
    pub const DET: &[u8] = include_bytes!(concat!(
        env!("OUT_DIR"),
        "/ocr-models/PP-OCRv6_det_tiny.onnx"
    ));
    pub const REC: &[u8] = include_bytes!(concat!(
        env!("OUT_DIR"),
        "/ocr-models/PP-OCRv6_rec_tiny.onnx"
    ));
    pub const CLS: &[u8] = include_bytes!(concat!(
        env!("OUT_DIR"),
        "/ocr-models/ch_ppocr_mobile_v2.0_cls_mobile.onnx"
    ));
    pub const DICT: &[u8] = include_bytes!(concat!(
        env!("OUT_DIR"),
        "/ocr-models/ppocrv6_tiny_dict.txt"
    ));

    pub const ALL: &[(&str, &[u8])] = &[
        ("PP-OCRv6_det_tiny.onnx", DET),
        ("PP-OCRv6_rec_tiny.onnx", REC),
        ("ch_ppocr_mobile_v2.0_cls_mobile.onnx", CLS),
        ("ppocrv6_tiny_dict.txt", DICT),
    ];
}

/// Writes the embedded weights out, once, and returns where they went.
///
/// ONNX Runtime loads from a path, so bytes in the binary have to become files
/// somewhere. Each is skipped when a file of the right size is already there,
/// which makes this cheap on every run after the first.
fn ensure_models_on_disk() -> Result<PathBuf, BackendError> {
    let directory = model_cache_dir().join(MODEL_SET);

    std::fs::create_dir_all(&directory).map_err(|error| {
        BackendError::Unavailable(format!(
            "could not create {}: {error}",
            directory.display()
        ))
    })?;

    for (filename, bytes) in weights::ALL {
        let path = directory.join(filename);

        if std::fs::metadata(&path).is_ok_and(|meta| meta.len() == bytes.len() as u64) {
            continue;
        }

        // Written through a temporary file so that a run interrupted half way
        // does not leave a short file that looks complete to the size check.
        let temporary = directory.join(format!("{filename}.part"));
        std::fs::write(&temporary, bytes).map_err(|error| {
            BackendError::Unavailable(format!("could not write {}: {error}", temporary.display()))
        })?;
        std::fs::rename(&temporary, &path).map_err(|error| {
            BackendError::Unavailable(format!("could not place {}: {error}", path.display()))
        })?;
    }

    Ok(directory)
}

/// Where extracted weights live.
///
/// Under the tool's own per-user directory unless something says otherwise.
/// The override exists because the weights are the one thing here large enough
/// that somebody may want them on a different disk.
fn model_cache_dir() -> PathBuf {
    match std::env::var("DOCFUSION_MODEL_CACHE") {
        Ok(explicit) => PathBuf::from(explicit),
        Err(_) => crate::dirs::user_dir(),
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
        let page = request.page;

        self.with_runner(|runner| {
            let output = runner.run_image(&rgb).map_err(|error| {
                BackendError::Failed(format!("the OCR pipeline failed on page {page}: {error}"))
            })?;

            Ok(lines_to_blocks(
                &output,
                request.image.width,
                request.image.height,
                &self.id(),
            ))
        })
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

            // And its confidence falls to the least confident line in it. A
            // paragraph that is only as sure as its first line would let an
            // uncertain line hide behind a confident one, which is exactly what
            // a policy reading confidence to decide whether to check the page
            // must not be told.
            let score = line.score.clamp(0.0, 1.0);
            previous.confidence = Some(previous.confidence.unwrap_or(score).min(score));

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

    /// The embedded weights have to be the ones the pipeline is looking for.
    ///
    /// They are pinned in two places — here and in `rapidocr-core` — because a
    /// build script cannot depend on the crate it is embedding for. Without
    /// this test an upstream update would leave the crate expecting weights
    /// that are no longer the ones embedded, and the symptom would be a model
    /// that reads nothing rather than a build that fails.
    #[test]
    fn the_embedded_weights_are_the_ones_the_pipeline_pins() {
        let model_set = rapidocr_core::model::model_set_by_name(MODEL_SET).expect("model set");
        let expected: Vec<(&str, &str)> = model_set
            .assets()
            .iter()
            .filter_map(|asset| asset.sha256.map(|digest| (asset.filename, digest)))
            .collect();

        assert_eq!(
            super::model_pins::PINS.len(),
            expected.len(),
            "a weight the pipeline needs is not embedded"
        );

        for &(filename, digest) in super::model_pins::PINS {
            assert!(
                expected
                    .iter()
                    .any(|(name, hash)| *name == filename && *hash == digest),
                "{filename} is not the weight the pipeline pins"
            );
        }
    }

    /// Reads a page through the real models.
    ///
    /// Ignored by default because it loads several megabytes of weights and
    /// takes seconds. It needs no setup beyond that: the weights come from the
    /// binary, which is the point of embedding them.
    ///
    /// It renders a PDF whose text the test states rather than shipping a test
    /// image, so the expected answer is in the test rather than inside a binary
    /// nobody can read.
    #[test]
    #[ignore = "loads the embedded weights and takes seconds"]
    fn reads_text_from_a_rendered_page() {
        use docfusion_core::extract::PageSource;
        use docfusion_core::extract::fixtures;

        let backend = RapidOcrBackend::bundled();

        // Large type, so that what is under test is the pipeline rather than
        // the resolution a page happens to be rendered at.
        let pdf = fixtures::pdf(&[(48.0, "Hello world")]);
        let pages = docfusion_core::extract::pdf::RenderedPages::open(&pdf, 200.0)
            .expect("open");
        let page = pages.page(0).expect("render");

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
