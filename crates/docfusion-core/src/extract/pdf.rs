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

use super::{
    ExtractError, Extractor, Fit, FoundTable, Input, InputProbe, PageSource, TableReader, TextLine,
};
use crate::mime::Container;
use crate::text::needs_space;

const ENGINE_ID: &str = "pdfium";
const ENGINE_VERSION: &str = "1.0.0";

/// The name this engine files a page under when the table model is in play.
///
/// Part of the cache key: the same bytes read with the table model and without
/// it are different Markdown, and one must not be served for the other.
const TABLES_ENGINE_ID: &str = "pdfium+tables";

/// The resolution a page is rendered at when it is looked at rather than read.
///
/// The text is the file's own and does not care; only the boxes around it come
/// from a picture. What it buys is how sharply the rulings of a table come out,
/// which is what the detector finds them by — and it costs a page of RGBA8 per
/// page in flight, so it is the same number the recognition path renders at.
pub const RENDER_DPI: f32 = 200.0;

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

pub struct PdfTextLayer {
    /// What finds the tables, when something can. `None` is not a degraded
    /// mode: a PDF's tables were never found before this existed, and every
    /// page still reads without it.
    tables: Option<Box<dyn TableReader>>,
}

impl Default for PdfTextLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl PdfTextLayer {
    pub fn new() -> Self {
        Self { tables: None }
    }

    /// The same reader, with something that can find the tables on a page.
    pub fn with_tables(mut self, reader: Box<dyn TableReader>) -> Self {
        self.tables = Some(reader);
        self
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
    /// The composition goes here and not in the id.
    ///
    /// `id` is what `--engine` names and what a person reading a report sees as
    /// the engine; the stamp is the cache key. A page read with the table model
    /// on it is a different document from the same bytes read without, so the
    /// two must not be served for each other — and an engine that renamed
    /// itself as soon as a model was downloaded would stop answering to the name
    /// it is documented under. The recognition path makes the same split: the
    /// router sees `scan`, the page says `rapidocr+layout+tables`.
    fn stamp(&self) -> EngineStamp {
        let model = match &self.tables {
            Some(reader) => format!("pdfium-{}+{}", pdfium_bundled::PDFIUM_VERSION, reader.id()),
            None => format!("pdfium-{}", pdfium_bundled::PDFIUM_VERSION),
        };

        EngineStamp::new(self.id(), self.version()).with_model(model)
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
        let pages = read_text(input)?;

        if pages.iter().all(|page| page.is_empty()) {
            return Err(ExtractError::unsupported(
                "the PDF has no text layer, so it needs an OCR or vision backend",
            ));
        }

        // Looked for rather than read out, and deliberately **after** the lock
        // the reading above held has been let go: finding a table renders a
        // page, rendering takes the same lock, and a lock taken twice by one
        // thread is a thread that waits for itself.
        //
        // What a table model adds here is which cell each word is in, and the
        // boxes that decide it. The words are the file's own and are already
        // exact, so the page is rendered and shown to the model, and the model
        // is told the words rather than asked for them.
        let found = match &self.tables {
            Some(reader) => tables_on(reader.as_ref(), input.bytes, &pages),
            None => vec![Vec::new(); pages.len()],
        };

        let body_size = dominant_font_size(&pages);

        // The name a page carries and the name the extractor answers to are not
        // the same thing, which is the split the recognition path makes: the
        // router sees `scan` and the page says `rapidocr+layout+tables`. The
        // first is what `stamp_of` resolves a page's engine through, so a page
        // filed under a name no extractor answers to is a page whose stamp
        // cannot be found — which is a document that cannot be written at all.
        let named = if self.tables.is_some() {
            TABLES_ENGINE_ID
        } else {
            ENGINE_ID
        };

        let extracted = pages
            .iter()
            .enumerate()
            .map(|(index, page)| page.to_ir(index as u32, body_size, named, &found[index]))
            .collect();

        Ok(Document {
            source: input.source.clone(),
            pages: extracted,
            meta: DocMeta::default(),
            conflicts: Vec::new(),
        })
    }
}

/// Every page's text, as the file reports it.
///
/// One call into PDFium for the whole document, so that the lock is held for the
/// reading and released before anything that has to render a page.
fn read_text(input: &Input<'_>) -> Result<Vec<PageRuns>, ExtractError> {
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

        Ok(pages)
    })
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

/// How much of `bbox` is inside `region`, as an area. Zero when they do not
/// meet.
fn held_by(bbox: &Rect, region: &Rect) -> f32 {
    let across = (bbox.x + bbox.w).min(region.x + region.w) - bbox.x.max(region.x);
    let down = (bbox.y + bbox.h).min(region.y + region.h) - bbox.y.max(region.y);

    across.max(0.0) * down.max(0.0)
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

/// The tables on every page, found by the model and filled with the file's own
/// words.
///
/// A page the model cannot be shown, or one it fails on, comes back with no
/// tables rather than as a failure. Every page of a text-layer PDF read without
/// a table model until this existed, and a model that is missing, corrupt or
/// simply wrong about a picture must not become a way for a page not to be read.
fn tables_on(reader: &dyn TableReader, bytes: &[u8], pages: &[PageRuns]) -> Vec<Vec<FoundTable>> {
    let scale = RENDER_DPI / 72.0;

    let Ok(rendered) = RenderedPages::open(bytes, RENDER_DPI) else {
        return vec![Vec::new(); pages.len()];
    };

    pages
        .iter()
        .enumerate()
        .map(|(index, page)| {
            if page.is_empty() {
                return Vec::new();
            }

            let Ok(image) = rendered.page(index) else {
                return Vec::new();
            };

            reader
                .tables(&image, &page.text_lines(scale))
                .unwrap_or_default()
        })
        .collect()
}

struct PageRuns {
    width: f32,
    height: f32,
    lines: Vec<Line>,
}

/// A found table with its boxes put into the page's own terms.
///
/// The table came back in the pixels of a rendered page and everything else
/// here speaks in points and normalises them, so the conversion happens once, at
/// the edge, rather than at every comparison.
struct Placed {
    bbox: Rect,
    /// Row by row, each row padded to the widest so that the cells line up.
    cells: Vec<Vec<(String, Option<Rect>)>>,
}

impl Placed {
    fn columns(&self) -> usize {
        self.cells.iter().map(Vec::len).max().unwrap_or(0)
    }
}

impl PageRuns {
    fn is_empty(&self) -> bool {
        self.lines
            .iter()
            .all(|line| line.text().trim().is_empty())
    }

    /// The words as a table reader wants them: text, and boxes in the pixels of
    /// a page rendered `scale` times the size of this one.
    ///
    /// Per **run** and not per line, which is the whole of what makes this work.
    /// A run is a stretch of text the file draws together and it is the finest
    /// thing that can be put in a cell. A line cannot be: a PDF with a text
    /// layer reports a table row as one line running its whole width, and handed
    /// that as a single box the placement rule puts the entire row in the cell
    /// holding most of it — which is the first one, since a row of five cells
    /// gives every cell a fifth of it. Measured that way, the header of a
    /// five-column table came out as `Region Q1 Q2 Q3 Q4` in one cell and four
    /// empty ones.
    fn text_lines(&self, scale: f32) -> Vec<TextLine> {
        self.lines
            .iter()
            .flat_map(|line| line.runs.iter())
            .filter(|run| !run.text.trim().is_empty())
            .map(|run| TextLine {
                text: run.text.clone(),
                bbox: (
                    run.left * scale,
                    run.top * scale,
                    run.right * scale,
                    run.bottom * scale,
                ),
            })
            .collect()
    }

    /// The found tables, in the page's own normalized terms.
    fn placed(&self, found: &[FoundTable], scale: f32) -> Vec<Placed> {
        let (across, down) = (self.width.max(1.0), self.height.max(1.0));

        let normalized = |(x0, y0, x1, y1): (f32, f32, f32, f32)| {
            let (left, top) = ((x0 / scale / across).clamp(0.0, 1.0), (y0 / scale / down).clamp(0.0, 1.0));
            let (right, bottom) = ((x1 / scale / across).clamp(0.0, 1.0), (y1 / scale / down).clamp(0.0, 1.0));

            Rect::new(left, top, (right - left).max(0.0), (bottom - top).max(0.0))
        };

        found
            .iter()
            .map(|table| {
                let widest = table.columns();
                let cells = table
                    .rows
                    .iter()
                    .map(|row| {
                        (0..widest)
                            .map(|column| match row.get(column) {
                                Some(cell) => {
                                    (cell.text.clone(), cell.bbox.map(normalized))
                                }
                                None => (String::new(), None),
                            })
                            .collect()
                    })
                    .collect();

                Placed {
                    bbox: normalized(table.bbox),
                    cells,
                }
            })
            .collect()
    }

    /// Which table holds each line, if any.
    ///
    /// The one holding most of it, by the rule the cells are filled by: a line
    /// belongs to the table it is mostly inside, and a line that is mostly
    /// outside every table is not in a table at all.
    fn owner(&self, placed: &[Placed]) -> Vec<Option<usize>> {
        self.lines
            .iter()
            .map(|line| {
                if line.text().trim().is_empty() {
                    return None;
                }

                let bbox = self.normalised_bbox(line)?;

                let mut best = (0.0f32, None);
                for (index, table) in placed.iter().enumerate() {
                    let held = held_by(&bbox, &table.bbox);
                    if held > best.0 {
                        best = (held, Some(index));
                    }
                }

                best.1
            })
            .collect()
    }

    /// A table as a block, its rows and cells drawn from what was found.
    ///
    /// **Every row is kept, empty ones included.** A row with no words in it is
    /// still a row: the recognition path drops them and measured, that took the
    /// tables from seven of nine back to one, because the model puts a header
    /// in a row of its own. The same model answers here and the same lesson
    /// applies.
    fn table_block(&self, table: &Placed, engine: &EngineId) -> Block {
        let columns = table.columns();
        let mut body: Vec<String> = Vec::with_capacity(table.cells.len());
        let mut children: Vec<Block> = Vec::with_capacity(table.cells.len());

        for row in &table.cells {
            let mut texts: Vec<String> = Vec::with_capacity(columns);
            let mut cells: Vec<Block> = Vec::with_capacity(columns);

            for (text, bbox) in row {
                texts.push(text.clone());
                cells.push(Block {
                    kind: BlockKind::TableCell,
                    text: text.clone(),
                    bbox: *bbox,
                    // Nothing was recognised here. The words are the file's and
                    // the boxes are the model's, and there is no third thing
                    // that could be more or less sure of either.
                    confidence: None,
                    engine: engine.clone(),
                    children: Vec::new(),
                });
            }

            body.push(texts.join(" "));
            children.push(Block {
                kind: BlockKind::TableRow,
                text: String::new(),
                bbox: None,
                confidence: None,
                engine: engine.clone(),
                children: cells,
            });
        }

        Block {
            kind: BlockKind::Table,
            text: body.join("\n"),
            bbox: Some(table.bbox),
            confidence: None,
            engine: engine.clone(),
            children,
        }
    }

    /// `named` is what the page is filed under; the blocks and the primary
    /// engine are this extractor's own name, which is what the router resolves
    /// a stamp through.
    fn to_ir(&self, index: u32, body_size: f32, named: &str, found: &[FoundTable]) -> Page {
        let placed = self.placed(found, RENDER_DPI / 72.0);
        let engine = EngineId::from(ENGINE_ID);

        Page {
            index,
            size: Some(Size {
                w: self.width,
                h: self.height,
            }),
            blocks: self.blocks(body_size, &engine, &placed),
            provenance: PageProvenance {
                primary: engine,
                engines: vec![EngineId::from(named)],
                verified: false,
            },
        }
    }

    fn blocks(&self, body_size: f32, engine: &EngineId, placed: &[Placed]) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut pending: Option<Block> = None;
        let mut previous_bottom: Option<f32> = None;

        let owner = self.owner(placed);
        let mut written = vec![false; placed.len()];

        for (at, line) in self.lines.iter().enumerate() {
            let text = line.text();
            if text.trim().is_empty() {
                continue;
            }

            // A line in a table is a cell of it and not a paragraph. The table
            // is written once, where the first of its lines would have gone, so
            // that it keeps the place on the page the file put it in.
            if let Some(table) = owner[at] {
                if !written[table] {
                    written[table] = true;
                    if let Some(block) = pending.take() {
                        blocks.push(block);
                    }
                    blocks.push(self.table_block(&placed[table], engine));
                    previous_bottom = None;
                }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::FoundCell;
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

    /// A reader that answers with whatever it was told, covering the page.
    ///
    /// The geometry of a real table is the table model's business and is tested
    /// where the model is; what is tested here is what this file does with the
    /// answer — where the table goes, what happens to the lines in it, and what
    /// happens when there is no answer at all.
    struct Fake {
        rows: Vec<Vec<&'static str>>,
        fails: bool,
    }

    impl Fake {
        fn rows(rows: Vec<Vec<&'static str>>) -> Self {
            Self { rows, fails: false }
        }
    }

    impl TableReader for Fake {
        fn id(&self) -> &'static str {
            "fake"
        }

        fn tables(
            &self,
            page: &PageImage,
            _lines: &[TextLine],
        ) -> Result<Vec<FoundTable>, ExtractError> {
            if self.fails {
                return Err(ExtractError::failed("fake", "no model on disk"));
            }

            if self.rows.is_empty() {
                return Ok(Vec::new());
            }

            Ok(vec![FoundTable {
                bbox: (0.0, 0.0, page.width as f32, page.height as f32),
                rows: self
                    .rows
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|text| FoundCell {
                                text: (*text).to_owned(),
                                bbox: None,
                            })
                            .collect()
                    })
                    .collect(),
            }])
        }
    }

    /// What the front matter will say the page was read by. The name a page is
    /// filed under and the name the router resolves a stamp through are two
    /// different things, and the front matter is the first.
    fn named_by(document: &Document) -> Vec<String> {
        document.pages[0]
            .provenance
            .engines
            .iter()
            .map(|engine| engine.to_string())
            .collect()
    }

    fn extract_with(reader: Fake, bytes: &[u8]) -> Result<Document, ExtractError> {
        let probe = InputProbe::new(bytes, Some("fixture.pdf".into()), bytes.len() as u64);
        let source = SourceRef {
            path: PathBuf::from("fixture.pdf"),
            content_hash: "0".repeat(64),
            mime: "application/pdf".into(),
            size: bytes.len() as u64,
            mtime: None,
        };

        PdfTextLayer::new()
            .with_tables(Box::new(reader))
            .extract(&Input {
                probe: &probe,
                bytes,
                source: &source,
            })
    }

    /// The line the page had is a cell of the table now, and not also a
    /// paragraph underneath it — a document that says everything twice is worse
    /// than one that says it in the wrong place.
    #[test]
    fn a_found_table_replaces_the_lines_it_holds() {
        let document = extract_with(
            Fake::rows(vec![vec!["a", "b"], vec!["c", "d"]]),
            &fixture(&[(12.0, "Hello world")]),
        )
        .expect("extract");

        // The page's own blocks, not every block under them: `Document::blocks`
        // walks into a table and would count its rows and cells as things the
        // page says, which is exactly the question this is asking.
        let blocks = &document.pages[0].blocks;
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        assert_eq!(blocks[0].kind, BlockKind::Table);
        assert_eq!(blocks[0].children.len(), 2);
        assert_eq!(named_by(&document), vec!["pdfium+tables"]);
    }

    /// And the words are the file's own, which is the whole reason this path
    /// exists: nothing here was recognised and nothing may be invented.
    #[test]
    fn the_words_in_the_cells_are_the_files_own() {
        let document = extract_with(
            Fake::rows(vec![vec!["Hello", "world"]]),
            &fixture(&[(12.0, "Hello world")]),
        )
        .expect("extract");

        let rendered = crate::render::markdown(&document, "1970-01-01T00:00:00Z");

        assert!(rendered.contains("| Hello | world |"), "{rendered}");
    }

    /// A reader that finds nothing leaves a page exactly as it was, which is
    /// what every page of every PDF was before this existed.
    #[test]
    fn a_reader_that_finds_nothing_changes_nothing() {
        let bytes = fixture(&[(12.0, "Hello world")]);

        let with = extract_with(Fake { rows: Vec::new(), fails: false }, &bytes).expect("extract");
        let without = extract(&bytes).expect("extract");

        assert_eq!(
            with.blocks().map(|b| b.text.clone()).collect::<Vec<_>>(),
            without.blocks().map(|b| b.text.clone()).collect::<Vec<_>>()
        );
    }

    /// A model that is missing, corrupt or wrong about a picture must not be a
    /// way for a page not to be read. The same promise the recognition path
    /// makes about the same models.
    #[test]
    fn a_reader_that_fails_is_not_a_page_that_will_not_read() {
        let document = extract_with(
            Fake {
                rows: Vec::new(),
                fails: true,
            },
            &fixture(&[(12.0, "Hello world")]),
        )
        .expect("a page that reads is a page that reads");

        let blocks: Vec<&str> = document.blocks().map(|b| b.text.as_str()).collect();

        assert_eq!(blocks, vec!["Hello world"]);
    }

    /// Without a reader the page is filed under the name it always was, because
    /// the name is the cache key and the two readings are different documents.
    #[test]
    fn the_name_says_whether_a_table_was_looked_for() {
        let bytes = fixture(&[(12.0, "Hello world")]);

        let with = extract_with(Fake::rows(vec![vec!["a"]]), &bytes).expect("extract");
        let without = extract(&bytes).expect("extract");

        assert_eq!(named_by(&with), vec!["pdfium+tables"]);
        assert_eq!(named_by(&without), vec!["pdfium"]);
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
