//! Local OCR, running in the process through ONNX Runtime.
//!
//! This is the engine that reads a page without an endpoint, a key or anything
//! sent anywhere, and the one that verifies everything else: it is local, free,
//! and fast enough to run on every page, which are the properties a verifier
//! has to have.
//!
//! It is *not* an engine that works with no setup. Its weights used to be
//! compiled into the binary and are now four files in the set `crate::models`
//! fetches, so on an installation that has not fetched them this refuses to
//! build, by name, and every command that reads a page says so. The trade is
//! written down in that module rather than here.
//!
//! The pipeline — text detection, optional angle classification, recognition —
//! comes from `rapidocr-core`. Writing DBNet post-processing, contour finding
//! and CTC decoding by hand would be a large amount of well-trodden work with
//! no decisions of our own in it.
//!
//! Lives here rather than in the core crate because it loads native libraries
//! and model files, and core performs no I/O.
//!
//! What it produces on its own is a page of paragraphs and nothing else, which
//! is all a text detector and a recogniser can produce. When the layout model
//! from [`crate::structure`] has been fetched, this runs it first and puts the
//! recognised words inside the regions it found, which is where headings and
//! tables come from. Without it every page reads exactly as it did before.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, TryLockError};

use docfusion_core::backend::{BackendError, Caps, OcrBackend, PageRequest};
use docfusion_core::ir::{Block, BlockKind, EngineId, Rect};
use rapidocr_core::RapidOcr;
use rapidocr_core::types::{OcrLine, OcrOutput};

use crate::models;
use crate::structure;
use crate::table;

const ENGINE_ID: &str = "rapidocr";

/// Raised from 1.0.0 when the page's reading order stopped being top-to-bottom.
///
/// The version is half the engine's identity and the identity is the cache key,
/// so a page read before that change is not the page this build would produce
/// and has to be read again. Raised for the whole engine rather than only for
/// the layout-aware one, which re-reads more than it has to and cannot serve a
/// page it should not.
const ENGINE_VERSION: &str = "1.1.0";

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
    /// The table structure reader, when its weights are here.
    ///
    /// Only ever consulted about a region the layout model called a table, so
    /// it is meaningless without that one and is only downloaded beside it.
    tables: Option<table::Reader>,
    /// The layout detector, when its weights are here.
    ///
    /// This is what turns a page of lines into a page of headings, tables and
    /// paragraphs. Absent by default and absent on most installations, because
    /// it is 124 MB that somebody has to ask for — see `crate::models`.
    structure: Option<structure::Detector>,
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

impl RapidOcrBackend {
    /// Reads pages using the models on this disk.
    ///
    /// The only way to build one. There used to be two — this, and a set of
    /// weights compiled into the binary — and the second is why "works with no
    /// setup at all" was true here and is not any more. See `crate::models` for
    /// what that bought and what it cost.
    ///
    /// Infallible, though it may be unusable: whether the four files are here is
    /// not known until a page needs reading, and a command that never reads one
    /// should not pay for the question. `build` is where it is asked.
    pub fn local() -> Self {
        let mut backend = Self::new();

        if structure::available() {
            backend.structure = Some(structure::Detector::new(structure::model_path()));
        }

        if table::available() {
            backend.tables = Some(table::Reader::new(table::model_path()));
        }

        backend
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

    fn new() -> Self {
        let count = pipeline_count();
        Self {
            runners: (0..count).map(|_| Mutex::new(None)).collect(),
            next: AtomicUsize::new(0),
            structure: None,
            tables: None,
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
        let root = models::default_root();

        // Asked here rather than in `local`, and only the four that a page of
        // text cannot be read without. A pipeline is built the first time a page
        // needs one, so a command that reads no page never asks and never pays
        // for the digest of 7 MB it is not going to use.
        let missing: Vec<&str> = models::absent(models::RECOGNITION_SET, &root)
            .iter()
            .map(|spec| spec.file)
            .collect();

        if !missing.is_empty() {
            return Err(BackendError::Unavailable(format!(
                "the models this tool reads a page with are not on disk: {}. \
                 `docfusion doctor --fetch` fetches them, and so does the window's settings.",
                missing.join(", ")
            )));
        }

        let model_set = rapidocr_core::model::model_set_by_name(models::RECOGNITION_DIR)
            .ok_or_else(|| {
                BackendError::Unavailable(format!(
                    "no model set named {}",
                    models::RECOGNITION_DIR
                ))
            })?;

        #[cfg_attr(not(feature = "directml"), allow(unused_mut))]
        let mut config = rapidocr_core::model::ModelCache::new(root.join(models::RECOGNITION_DIR))
            .config_for(model_set);

        // Offered rather than chosen, and falling back rather than failing: a
        // machine with no DirectML device is the ordinary case, and refusing to
        // read a page because the graphics card could not be borrowed would be
        // the worst possible answer to it.
        #[cfg(feature = "directml")]
        {
            config.inference.execution_provider =
                rapidocr_core::config::ExecutionProvider::DirectMl;

            if let Ok(ocr) = RapidOcr::new(config.clone()) {
                return Ok(ocr);
            }

            config.inference.execution_provider = rapidocr_core::config::ExecutionProvider::Cpu;
        }

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

/// Whether a page of text that is a picture can be read at all.
///
/// The same kind of answer as `structure::available`, and remembered the same
/// way and for the same reason: it costs the digest of 7 MB, it is asked before
/// every document and on every redraw of the window, and it changes only when
/// somebody has downloaded something.
///
/// Not to be confused with the check `build` makes. That one names the files
/// that are missing, which is a question about what to do next; this one is a
/// yes or a no.
pub fn available() -> bool {
    AVAILABLE.get(|| models::present_all(models::RECOGNITION_SET, &models::default_root()))
}

/// Throws the remembered answer away, so the next question looks at the disk.
///
/// What a download calls, and what makes the button that started it mean
/// something before the next launch.
pub fn forget() {
    AVAILABLE.forget();
}

static AVAILABLE: models::Cached = models::Cached::new();

impl OcrBackend for RapidOcrBackend {
    fn id(&self) -> EngineId {
        // Built from what is here, because the name is the cache key and what
        // read the page is what the page is worth reading again against. A
        // table reader cannot be here without the layout model, since it is
        // only ever asked about a region that one called a table.
        let mut name = String::from(ENGINE_ID);

        if self.structure.is_some() {
            name.push_str("+layout");
        }
        if self.tables.is_some() {
            name.push_str("+tables");
        }

        EngineId::from(name)
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
        let rgb = structure::to_rgb(request.image);

        // Found before the recogniser runs and not after it, so that the
        // pipeline is not held for the half second the detector takes. A page
        // with no structure model, or one the model finds nothing on, is read
        // exactly as it always was — this is an improvement on top of the
        // engine, never a way for a page not to be read.
        let regions = match &self.structure {
            Some(detector) => detector.detect(&rgb).unwrap_or_default(),
            None => Vec::new(),
        };

        let page = request.page;
        let (width, height) = (request.image.width, request.image.height);

        self.with_runner(|runner| {
            let output = runner.run_image(&rgb).map_err(|error| {
                BackendError::Failed(format!("the OCR pipeline failed on page {page}: {error}"))
            })?;

            Ok(if regions.is_empty() {
                lines_to_blocks(&output, width, height, &self.id())
            } else {
                let tables = self.tables.as_ref().map(|reader| structure::Tables {
                    page: &rgb,
                    reader,
                });

                structure::blocks_from(&output.lines, &regions, tables, width, height, &self.id())
            })
        })
    }
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

    /// What this downloads has to be what the pipeline is looking for.
    ///
    /// Pinned in two places — the catalogue in `crate::models` and
    /// `rapidocr-core` — because neither can ask the other at compile time: the
    /// crate does not know about this tree and the catalogue is a table of
    /// literals. Before this was a test it was a build script, and the property
    /// it protects is the same one: an upstream update must fail here rather
    /// than leave the downloader fetching weights the pipeline will not load.
    ///
    /// The paths matter as much as the digests. The crate looks for its four in
    /// one directory and finds them by filename, so a catalogue that put them
    /// somewhere else would download the right bytes to a place nothing reads.
    #[test]
    fn the_weights_we_fetch_are_the_ones_the_pipeline_pins() {
        let model_set = rapidocr_core::model::model_set_by_name(crate::models::RECOGNITION_DIR)
            .expect("the catalogue names a model set the crate has");

        let expected: Vec<(&str, &str)> = model_set
            .assets()
            .iter()
            .filter_map(|asset| asset.sha256.map(|digest| (asset.filename, digest)))
            .collect();

        assert_eq!(
            crate::models::RECOGNITION_SET.len(),
            expected.len(),
            "the catalogue and the pipeline disagree about how many files there are"
        );

        for spec in crate::models::RECOGNITION_SET {
            let filename = spec
                .file
                .rsplit('/')
                .next()
                .expect("a file name after the last separator");

            assert!(
                expected
                    .iter()
                    .any(|(name, digest)| *name == filename && *digest == spec.sha256),
                "{} is not the weight the pipeline pins, or not its digest",
                spec.file
            );
        }
    }

    /// Reads a page through the real models.
    ///
    /// Ignored by default because it loads several megabytes of weights and
    /// takes seconds, and because it now needs something a checkout does not
    /// have: the set downloaded. `docfusion doctor --fetch` first, or the run
    /// fails on a sentence naming the four files rather than on anything
    /// interesting.
    ///
    /// It renders a PDF whose text the test states rather than shipping a test
    /// image, so the expected answer is in the test rather than inside a binary
    /// nobody can read.
    #[test]
    #[ignore = "needs the model set downloaded, and takes seconds"]
    fn reads_text_from_a_rendered_page() {
        use docfusion_core::extract::PageSource;
        use docfusion_core::extract::fixtures;

        let backend = RapidOcrBackend::local();

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
