//! Text-layer PDF extraction, backed by PDFium.
//!
//! PDFium is Chromium's PDF engine, which is the reason this path exists: it
//! handles CJK and CID fonts properly, and it reports per-character bounds,
//! font names and font sizes.
//!
//! Only PDFs that already carry a text layer are read here. Anything scanned
//! produces no text, and the engine declines so the router can fall through to
//! an OCR or vision backend — which is the whole point of `Fit::Maybe`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use std::sync::Arc;

use pdfium_bundled::bind_bundled;
use pdfium_bundled::pdfium_render::prelude::{PdfPageText, PdfRenderConfig, Pdfium};

use crate::image::PageImage;
use crate::ir::{
    Block, BlockKind, DocMeta, Document, EngineId, EngineStamp, Page, PageProvenance, Rect, Size,
};

use super::{ExtractError, Extractor, Fit, Input, InputProbe, PageSource};
use crate::mime::Container;

const ENGINE_ID: &str = "pdfium";
const ENGINE_VERSION: &str = "1.0.0";

/// A font is treated as a heading when it is at least this much larger than
/// the document's body text.
const HEADING_RATIO: f32 = 1.15;

/// Ratio thresholds for heading depth. Deliberately coarse: font size is a
/// signal, not a specification, and documents disagree about how much larger a
/// section title should be.
const LEVEL_TWO_RATIO: f32 = 1.4;
const LEVEL_ONE_RATIO: f32 = 1.8;

/// Two lines belong to the same paragraph when the gap between them is under
/// this multiple of the body line height.
const PARAGRAPH_GAP: f32 = 0.8;

pub struct PdfTextLayer;

impl Default for PdfTextLayer {
    fn default() -> Self {
        Self
    }
}

impl PdfTextLayer {
    pub fn new() -> Self {
        Self
    }
}

/// The shared PDFium binding.
///
/// PDFium can be bound only **once per process**: a second `bind_bundled()` call
/// fails with `PdfiumLibraryBindingsAlreadyInitialized` rather than handing back
/// the library that is already loaded. A process-wide handle is therefore a
/// requirement, not an optimisation — two engine instances in one process would
/// break, and so would one instance used twice.
///
/// A failed load is remembered for the life of the process. That is deliberate:
/// the failure means the native library could not be loaded, which is fixed by
/// restarting rather than by retrying inside the same process.
static ENGINE: OnceLock<Result<Mutex<Pdfium>, String>> = OnceLock::new();

fn with_engine<T>(action: impl FnOnce(&Pdfium) -> Result<T, ExtractError>) -> Result<T, ExtractError> {
    let engine = ENGINE
        .get_or_init(|| {
            bind_bundled()
                .map(Mutex::new)
                .map_err(|error| format!("could not load the PDF engine: {error}"))
        })
        .as_ref()
        .map_err(|message| ExtractError::failed(ENGINE_ID, message.clone()))?;

    // PDFium gives no thread-safety guarantee, and pdfium-render only serialises
    // calls internally when its `thread_safe` feature is enabled. Guard here.
    let guard = engine
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    action(&guard)
}

impl Extractor for PdfTextLayer {
    fn id(&self) -> EngineId {
        EngineId::from(ENGINE_ID)
    }

    fn version(&self) -> &str {
        ENGINE_VERSION
    }

    /// The underlying PDFium build participates in the cache key, so upgrading
    /// it invalidates previous results without anyone having to remember to
    /// bump `ENGINE_VERSION`.
    fn stamp(&self) -> EngineStamp {
        EngineStamp::new(self.id(), self.version())
            .with_model(format!("pdfium-{}", pdfium_bundled::PDFIUM_VERSION))
    }

    /// `Maybe`, not `Yes`: sixteen magic bytes cannot reveal whether a PDF
    /// carries a text layer. Only opening it can.
    fn accepts(&self, probe: &InputProbe) -> Fit {
        match probe.container {
            Container::Pdf => Fit::Maybe,
            _ => Fit::No,
        }
    }

    fn extract(&self, input: &Input<'_>) -> Result<Document, ExtractError> {
        with_engine(|pdfium| {
            let pdf = pdfium
                .load_pdf_from_byte_slice(input.bytes, None)
                .map_err(|error| {
                    ExtractError::failed(ENGINE_ID, format!("could not open the PDF: {error}"))
                })?;

            let mut pages = Vec::new();
            for page in pdf.pages().iter() {
                let text = page.text().map_err(|error| {
                    ExtractError::failed(ENGINE_ID, format!("could not read page text: {error}"))
                })?;
                let height = page.height().value;
                pages.push(PageRuns {
                    width: page.width().value,
                    height,
                    lines: group_into_lines(collect_runs(&text, height)),
                });
            }

            if pages.iter().all(|page| page.is_empty()) {
                return Err(ExtractError::unsupported(
                    "the PDF has no text layer, so it needs an OCR or vision backend",
                ));
            }

            let body_size = dominant_font_size(&pages);
            let engine = EngineId::from(ENGINE_ID);

            let extracted = pages
                .iter()
                .enumerate()
                .map(|(index, page)| page.to_ir(index as u32, body_size, &engine))
                .collect();

            Ok(Document {
                source: input.source.clone(),
                pages: extracted,
                meta: DocMeta::default(),
                conflicts: Vec::new(),
            })
        })
    }
}

/// Renders every page of a PDF to pixels.
///
/// This is how a document with no text layer is read at all: its pages become
/// images and go to an OCR or vision backend. Rendering happens at a chosen
/// resolution rather than the PDF's own units, because recognition quality
/// depends on how many pixels each character covers.
/// The pages of a PDF, rendered when they are asked for.
///
/// The document is reopened for every page. That looks wasteful and is not: a
/// page costs seconds to recognise and milliseconds to open, while holding the
/// document open across calls would mean holding the PDFium lock across them
/// too, which would leave rendering serialised against everything else for the
/// length of a document rather than the length of a page.
pub struct RenderedPages<'a> {
    bytes: &'a [u8],
    dpi: f32,
    count: usize,
}

impl<'a> RenderedPages<'a> {
    /// Opens the document far enough to know how many pages it has.
    pub fn open(bytes: &'a [u8], dpi: f32) -> Result<Self, ExtractError> {
        // PDFium's page index is a signed 32-bit integer; nothing a document
        // can be is anywhere near that.
        let count = with_engine(|pdfium| Ok(open(pdfium, bytes)?.pages().len() as usize))?;

        Ok(Self { bytes, dpi, count })
    }
}

impl PageSource for RenderedPages<'_> {
    fn len(&self) -> usize {
        self.count
    }

    fn page(&self, index: usize) -> Result<Arc<PageImage>, ExtractError> {
        with_engine(|pdfium| {
            let pdf = open(pdfium, self.bytes)?;

            let reachable = i32::try_from(index).map_err(|_| {
                ExtractError::unsupported(format!("this document has no page {}", index + 1))
            })?;
            let page = pdf.pages().get(reachable).map_err(|error| {
                ExtractError::failed(
                    ENGINE_ID,
                    format!("could not reach page {}: {error}", index + 1),
                )
            })?;

            // PDF units are points: 72 to the inch. The pixel format is left at
            // PDFium's own BGRA; `as_rgba_bytes` normalises it, and there is no
            // RGBA format to ask for in the first place.
            let config = PdfRenderConfig::new().scale_page_by_factor(self.dpi / 72.0);
            let bitmap = page.render_with_config(&config).map_err(|error| {
                ExtractError::failed(ENGINE_ID, format!("could not render a page: {error}"))
            })?;

            Ok(Arc::new(PageImage {
                width: bitmap.width() as u32,
                height: bitmap.height() as u32,
                pixels: bitmap.as_rgba_bytes(),
            }))
        })
    }
}

/// Opens a document. Both references carry the same lifetime because PDFium
/// borrows the bytes for as long as the document lives.
fn open<'a>(
    pdfium: &'a Pdfium,
    bytes: &'a [u8],
) -> Result<pdfium_bundled::pdfium_render::prelude::PdfDocument<'a>, ExtractError> {
    pdfium
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(|error| ExtractError::failed(ENGINE_ID, format!("could not open the PDF: {error}")))
}

/// One text fragment as PDFium reports it.
#[derive(Debug, Clone)]
struct Run {
    text: String,
    left: f32,
    right: f32,
    /// Distance from the top of the page, so that smaller means higher up.
    top: f32,
    bottom: f32,
    font_size: f32,
}

impl Run {
    fn height(&self) -> f32 {
        self.bottom - self.top
    }

    fn centre(&self) -> f32 {
        (self.top + self.bottom) / 2.0
    }
}

#[derive(Debug, Clone)]
struct Line {
    runs: Vec<Run>,
}

impl Line {
    fn top(&self) -> f32 {
        self.runs
            .iter()
            .map(|run| run.top)
            .fold(f32::INFINITY, f32::min)
    }

    fn bottom(&self) -> f32 {
        self.runs
            .iter()
            .map(|run| run.bottom)
            .fold(f32::NEG_INFINITY, f32::max)
    }

    fn left(&self) -> f32 {
        self.runs
            .iter()
            .map(|run| run.left)
            .fold(f32::INFINITY, f32::min)
    }

    fn right(&self) -> f32 {
        self.runs
            .iter()
            .map(|run| run.right)
            .fold(f32::NEG_INFINITY, f32::max)
    }

    fn font_size(&self) -> f32 {
        self.runs
            .iter()
            .map(|run| run.font_size)
            .fold(0.0f32, f32::max)
    }

    fn text(&self) -> String {
        let mut text = String::new();
        for run in &self.runs {
            if !text.is_empty() && needs_space(&text, &run.text) {
                text.push(' ');
            }
            text.push_str(&run.text);
        }
        text
    }
}

struct PageRuns {
    width: f32,
    height: f32,
    lines: Vec<Line>,
}

impl PageRuns {
    fn is_empty(&self) -> bool {
        self.lines
            .iter()
            .all(|line| line.text().trim().is_empty())
    }

    fn to_ir(&self, index: u32, body_size: f32, engine: &EngineId) -> Page {
        Page {
            index,
            size: Some(Size {
                w: self.width,
                h: self.height,
            }),
            blocks: self.blocks(body_size, engine),
            provenance: PageProvenance {
                primary: engine.clone(),
                engines: vec![engine.clone()],
                verified: false,
            },
        }
    }

    fn blocks(&self, body_size: f32, engine: &EngineId) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut pending: Option<Block> = None;
        let mut previous_bottom: Option<f32> = None;

        for line in &self.lines {
            let text = line.text();
            if text.trim().is_empty() {
                continue;
            }

            let bbox = self.normalised_bbox(line);
            let size = line.font_size();
            let kind = classify(size, body_size);

            let starts_paragraph = previous_bottom.is_some_and(|bottom| {
                line.top() - bottom > body_size.max(size) * PARAGRAPH_GAP
            });

            let continues_paragraph = matches!(kind, BlockKind::Paragraph)
                && !starts_paragraph
                && pending
                    .as_ref()
                    .is_some_and(|block| matches!(block.kind, BlockKind::Paragraph));

            if continues_paragraph {
                if let Some(block) = pending.as_mut() {
                    join_line(&mut block.text, &text);
                }
            } else {
                if let Some(block) = pending.take() {
                    blocks.push(block);
                }
                pending = Some(Block {
                    kind,
                    text,
                    bbox,
                    confidence: None,
                    engine: engine.clone(),
                    children: Vec::new(),
                });
            }

            previous_bottom = Some(line.bottom());
        }

        if let Some(block) = pending.take() {
            blocks.push(block);
        }

        blocks
    }

    /// Runs in `PageRuns` are already in a top-left origin (see `collect_runs`),
    /// so this only has to scale.
    fn normalised_bbox(&self, line: &Line) -> Option<Rect> {
        if self.width <= 0.0 || self.height <= 0.0 {
            return None;
        }

        let left = (line.left() / self.width).clamp(0.0, 1.0);
        let top = (line.top() / self.height).clamp(0.0, 1.0);
        let right = (line.right() / self.width).clamp(0.0, 1.0);
        let bottom = (line.bottom() / self.height).clamp(0.0, 1.0);

        Some(Rect::new(
            left,
            top,
            (right - left).max(0.0),
            (bottom - top).max(0.0),
        ))
    }
}

/// Collects text fragments, converting PDF coordinates as it goes.
///
/// PDFium reports PDF coordinates: the origin is the bottom-left corner and y
/// increases upwards. The IR uses a top-left origin with y increasing
/// downwards, so the conversion happens here — once, at the boundary, where the
/// page height is known. Doing it later means every consumer has to remember
/// which convention it is holding.
fn collect_runs(text: &PdfPageText<'_>, page_height: f32) -> Vec<Run> {
    let mut runs = Vec::new();

    for segment in text.segments().iter() {
        let content = segment.text();
        if content.trim().is_empty() {
            continue;
        }

        let bounds = segment.bounds();
        // `scaled_font_size` is the size the text is actually drawn at, after
        // any transformation matrix, which is what heading detection needs.
        let font_size = segment
            .chars()
            .ok()
            .and_then(|chars| chars.iter().next().map(|char| char.scaled_font_size().value))
            .unwrap_or(0.0);

        let (left, right) = ordered(bounds.left().value, bounds.right().value);
        let (low, high) = ordered(bounds.bottom().value, bounds.top().value);

        runs.push(Run {
            text: content,
            left,
            right,
            top: page_height - high,
            bottom: page_height - low,
            font_size,
        });
    }

    runs
}

fn ordered(a: f32, b: f32) -> (f32, f32) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Groups runs into visual lines by vertical overlap.
///
/// Runs are matched against existing lines rather than against the previous
/// run, because runs from different lines can interleave once sorted.
fn group_into_lines(mut runs: Vec<Run>) -> Vec<Line> {
    runs.sort_by(|a, b| a.centre().total_cmp(&b.centre()).then(a.left.total_cmp(&b.left)));

    let mut lines: Vec<Line> = Vec::new();
    for run in runs {
        let tolerance = run.height().max(run.font_size) / 2.0;
        match lines
            .iter_mut()
            .find(|line| (line_centre(line) - run.centre()).abs() <= tolerance)
        {
            Some(line) => line.runs.push(run),
            None => lines.push(Line { runs: vec![run] }),
        }
    }

    for line in &mut lines {
        line.runs.sort_by(|a, b| a.left.total_cmp(&b.left));
    }
    lines.sort_by(|a, b| a.top().total_cmp(&b.top()));

    lines
}

fn line_centre(line: &Line) -> f32 {
    let top = line.top();
    let bottom = line.bottom();
    (top + bottom) / 2.0
}

/// The dominant font size across the document, weighted by character count, so
/// that a few large headings cannot shift the baseline.
fn dominant_font_size(pages: &[PageRuns]) -> f32 {
    let mut tally: HashMap<i32, (f32, usize)> = HashMap::new();

    for page in pages {
        for line in &page.lines {
            let size = line.font_size();
            if size <= 0.0 {
                continue;
            }
            // Font sizes are floats; bucket to a tenth of a point.
            let key = (size * 10.0).round() as i32;
            let entry = tally.entry(key).or_insert((size, 0));
            entry.1 += line.text().chars().count();
        }
    }

    tally
        .into_values()
        .max_by_key(|(_, count)| *count)
        .map(|(size, _)| size)
        // A one-line document has no meaningful baseline; treat its only size
        // as body text.
        .unwrap_or(0.0)
}

fn classify(size: f32, body_size: f32) -> BlockKind {
    if body_size <= 0.0 || size < body_size * HEADING_RATIO {
        return BlockKind::Paragraph;
    }

    let ratio = size / body_size;
    let level = if ratio >= LEVEL_ONE_RATIO {
        1
    } else if ratio >= LEVEL_TWO_RATIO {
        2
    } else {
        3
    };

    BlockKind::Heading { level }
}

/// Joins a wrapped line onto the end of a paragraph.
///
/// CJK text is written without spaces between words, so inserting one at every
/// line break would corrupt it.
fn join_line(paragraph: &mut String, line: &str) {
    if !paragraph.is_empty() && needs_space(paragraph, line) {
        paragraph.push(' ');
    }
    paragraph.push_str(line);
}

fn needs_space(left: &str, right: &str) -> bool {
    match (left.chars().next_back(), right.chars().next()) {
        (Some(a), Some(b)) => !is_cjk(a) && !is_cjk(b),
        _ => false,
    }
}

fn is_cjk(character: char) -> bool {
    matches!(character as u32,
        0x3000..=0x303F     // CJK punctuation
        | 0x3040..=0x30FF   // kana
        | 0x3400..=0x4DBF   // CJK extension A
        | 0x4E00..=0x9FFF   // CJK unified ideographs
        | 0xF900..=0xFAFF   // compatibility ideographs
        | 0xFF00..=0xFFEF   // full-width forms
        | 0x20000..=0x2FA1F // extensions B and beyond
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::SourceRef;
    use std::path::PathBuf;

    /// Builds a minimal but valid single-page PDF.
    /// PDF fixtures are shared with the other extractor tests, so that two
    /// copies of the same document cannot drift apart.
    use crate::extract::fixtures::pdf as fixture;

    fn extract(bytes: &[u8]) -> Result<Document, ExtractError> {
        let probe = InputProbe::new(bytes, Some("fixture.pdf".into()), bytes.len() as u64);
        let source = SourceRef {
            path: PathBuf::from("fixture.pdf"),
            content_hash: "0".repeat(64),
            mime: "application/pdf".into(),
            size: bytes.len() as u64,
            mtime: None,
        };
        PdfTextLayer::new().extract(&Input {
            probe: &probe,
            bytes,
            source: &source,
        })
    }

    #[test]
    fn accepts_pdfs_but_only_speculatively() {
        let engine = PdfTextLayer::new();
        let pdf = InputProbe::new(b"%PDF-1.7", None, 8);
        let text = InputProbe::new(b"hello", None, 5);

        assert_eq!(engine.accepts(&pdf), Fit::Maybe);
        assert_eq!(engine.accepts(&text), Fit::No);
    }

    #[test]
    fn extracts_text_from_a_text_layer_pdf() {
        let document = extract(&fixture(&[(12.0, "Hello world")])).expect("extract");
        let blocks: Vec<&str> = document.blocks().map(|b| b.text.as_str()).collect();
        assert_eq!(blocks, vec!["Hello world"]);
    }

    /// Reading order is the failure that corrupts downstream use silently, so
    /// it is asserted directly rather than inferred from the block count.
    #[test]
    fn blocks_come_back_in_reading_order() {
        let document =
            extract(&fixture(&[(24.0, "Title"), (12.0, "Body text")])).expect("extract");
        let blocks: Vec<&str> = document.blocks().map(|b| b.text.as_str()).collect();
        assert_eq!(blocks, vec!["Title", "Body text"]);
    }

    #[test]
    fn larger_text_becomes_a_heading() {
        let document = extract(&fixture(&[
            (24.0, "Title"),
            (12.0, "First body line"),
            (12.0, "Second body line"),
            (12.0, "Third body line"),
        ]))
        .expect("extract");

        let blocks: Vec<&Block> = document.blocks().collect();
        assert_eq!(blocks[0].kind, BlockKind::Heading { level: 1 });
        assert_eq!(blocks[0].text, "Title");
        assert!(
            blocks[1..]
                .iter()
                .all(|block| block.kind == BlockKind::Paragraph),
            "body text should not be promoted to headings"
        );
    }

    #[test]
    fn page_geometry_is_reported() {
        let document = extract(&fixture(&[(12.0, "Hello")])).expect("extract");
        let size = document.pages[0].size.expect("page size");
        assert!((size.w - 612.0).abs() < 1.0, "{size:?}");
        assert!((size.h - 792.0).abs() < 1.0, "{size:?}");
    }

    #[test]
    fn bounding_boxes_are_normalised_to_the_top_left_origin() {
        let document = extract(&fixture(&[(12.0, "Near the top")])).expect("extract");
        let bbox = document.blocks().next().unwrap().bbox.expect("bbox");

        assert!((0.0..=1.0).contains(&bbox.x), "{bbox:?}");
        assert!((0.0..=1.0).contains(&bbox.y), "{bbox:?}");
        assert!(bbox.x + bbox.w <= 1.0 + f32::EPSILON, "{bbox:?}");
        assert!(bbox.y + bbox.h <= 1.0 + f32::EPSILON, "{bbox:?}");
        // The fixture draws at y=720 of a 792-high page, so the box must sit
        // near the top. A bottom-left origin would put it near the bottom.
        assert!(bbox.y < 0.2, "expected a top-of-page box, got {bbox:?}");
    }

    #[test]
    fn the_engine_reports_provenance_and_declines_verification() {
        let document = extract(&fixture(&[(12.0, "Hello")])).expect("extract");
        let provenance = &document.pages[0].provenance;

        assert_eq!(provenance.primary.as_str(), "pdfium");
        assert!(!provenance.verified);
        assert!(document.conflicts.is_empty());
    }

    /// The PDFium build participates in the cache key, so that upgrading the
    /// engine invalidates previously extracted documents automatically.
    #[test]
    fn the_cache_stamp_includes_the_pdfium_build() {
        let stamp = PdfTextLayer::new().stamp();
        assert_eq!(stamp.id.as_str(), "pdfium");
        assert_eq!(
            stamp.model.as_deref(),
            Some(format!("pdfium-{}", pdfium_bundled::PDFIUM_VERSION).as_str())
        );
    }

    #[test]
    fn joining_cjk_lines_does_not_insert_spaces() {
        let mut paragraph = String::from("这是第一行");
        join_line(&mut paragraph, "这是第二行");
        assert_eq!(paragraph, "这是第一行这是第二行");
    }

    #[test]
    fn joining_latin_lines_inserts_a_space() {
        let mut paragraph = String::from("first line");
        join_line(&mut paragraph, "second line");
        assert_eq!(paragraph, "first line second line");
    }

    /// Every page of a fixture, rendered.
    ///
    /// The pipeline asks for one page at a time; these tests want them all, and
    /// saying so once beats repeating the loop.
    fn render_all(bytes: &[u8], dpi: f32) -> Result<Vec<Arc<PageImage>>, ExtractError> {
        let pages = RenderedPages::open(bytes, dpi)?;
        (0..pages.len()).map(|index| pages.page(index)).collect()
    }

    #[test]
    fn renders_a_page_to_pixels_at_the_requested_resolution() {
        let bytes = fixture(&[(12.0, "Hello")]);

        // The fixture page is 612×792 points, and points are 72 to the inch.
        let at_72_dpi = render_all(&bytes, 72.0).expect("render");
        assert_eq!(at_72_dpi.len(), 1);
        assert_eq!((at_72_dpi[0].width, at_72_dpi[0].height), (612, 792));

        let at_144_dpi = render_all(&bytes, 144.0).expect("render");
        assert_eq!((at_144_dpi[0].width, at_144_dpi[0].height), (1224, 1584));
    }

    #[test]
    fn rendered_pixels_are_rgba() {
        let images = render_all(&fixture(&[(12.0, "Hello")]), 72.0).expect("render");
        let image = &images[0];

        assert_eq!(
            image.pixels.len(),
            (image.width * image.height * 4) as usize,
            "the buffer must be four bytes per pixel"
        );
    }

    /// Rendering is how a scan becomes readable at all, so it must work on a
    /// PDF that has no text to extract.
    #[test]
    fn a_pdf_without_a_text_layer_still_renders() {
        let bytes = fixture(&[]);

        assert!(matches!(
            extract(&bytes),
            Err(ExtractError::Unsupported { .. })
        ));
        assert_eq!(render_all(&bytes, 72.0).expect("render").len(), 1);
    }

    #[test]
    fn a_missing_text_layer_declines_rather_than_failing() {
        // Valid PDF, no content stream text at all.
        let bytes = fixture(&[]);
        let error = extract(&bytes).expect_err("should decline");
        assert!(
            matches!(error, ExtractError::Unsupported { .. }),
            "expected a decline so routing continues, got {error}"
        );
    }
}
