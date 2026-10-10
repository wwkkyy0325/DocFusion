//! Finding the structure on a page, and putting the recognised words inside it.
//!
//! The bundled recogniser reads every page and cannot say what any of it is: it
//! returns text, on a line at a time, and a page of headings and a page of
//! prose come out identical. On the corpus this was measured on it found **0 of
//! 64 headings and 0 of 9 tables** — not a bug in it, since a text detector and
//! a recogniser are all it is, but a ceiling.
//!
//! PP-DocLayout_plus-L is the other half. It is a detector and not a reader:
//! it puts a box around each region of a page and names twenty kinds of them,
//! and reads none of the words. The two together give what neither gives alone,
//! and that is what this module is — the join.
//!
//! Three things are decided here and nowhere else: how the model's twenty
//! classes become our ten block kinds, which words belong to which region, and
//! how a region's lines become rows and cells. All three are pure functions of
//! the model's output and the recogniser's, which is why almost all of this file
//! runs without the weights on disk.

use std::path::PathBuf;
use std::sync::Mutex;

use docfusion_core::backend::BackendError;
use docfusion_core::extract::{ExtractError, FoundCell, FoundTable, TableReader, TextLine};
use docfusion_core::image::PageImage;
use docfusion_core::ir::{Block, BlockKind, EngineId, Rect};
use ort::session::Session;
use ort::value::TensorRef;
use rapidocr_core::types::{OcrLine, Quad};

use crate::layout;
use crate::table;

/// The side the page is squashed to before the model sees it.
///
/// A square, and the aspect ratio is not kept, because that is what the model
/// was trained on. It is also the coordinate system its answers come back in,
/// which the two inputs named for undoing that do not actually undo. See
/// [`Region::on_page`].
const SIDE: usize = 800;

/// Below this the detector is guessing, and a guess about a heading lands in
/// the document as a heading.
const FLOOR: f32 = 0.5;

/// How much of the space a box may cover before it stops counting as evidence
/// of where the columns are.
///
/// A row spanning a whole table crosses every column, and a title spanning a
/// whole page crosses every column, so counting either would weld the columns
/// into one — losing a table, or reading a two-column page as one. Neither
/// disappears: a spanning line is still a cell, and a spanning region is still
/// read, just not asked where the boundaries are.
const SPANNING: f32 = 0.8;

/// How wide a gap has to be, as a share of the page, before it is a gutter
/// between columns rather than a ragged right margin.
///
/// **One per cent, and the number came from being wrong.** At two per cent the
/// first two-column page measured came out as one column: its gutter is 26
/// pixels on a 1370-pixel page, which is 1.9%, and the boundary sat 1.4 pixels
/// the wrong side of it. Nothing about the page was ambiguous — the two columns
/// are 39% of the width each with a clean corridor between them — so the
/// threshold was.
///
/// Smaller is the safe direction here, because the boxes being merged are whole
/// detected regions and not lines: a region already spans its column, so two
/// regions of one column overlap heavily, and a gap between them means a gutter.
/// The error the other way — reading one column as two — is what the corpus is
/// for, and it did not appear at this setting.
const COLUMN_GUTTER: f32 = 0.01;

/// How little of a page's height a strip has to cover before it is white.
///
/// Five per cent of a page is a hundred pixels of a two-thousand-pixel scan — a
/// running head, a rule, a page number. Anything that is not a column.
const COLUMN_FILL: f32 = 0.05;

/// How much room between two lines of the same column before they are two
/// cells rather than one wrapped one.
///
/// A fraction of the line height, because that is the only scale a page carries
/// with it. Wrapped lines touch or nearly touch; the gap to the next row is at
/// least the padding between rows, which in any table worth reading is more.
const WRAP_GAP: f32 = 0.5;

/// What a region is, in the terms this program has for blocks.
///
/// Seven rather than the model's twenty: the classes below that are not
/// distinguished here are distinctions the document format cannot carry
/// anyway — a running header and a footnote are both a paragraph once they are
/// Markdown — and pretending otherwise would be a field nothing reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    Title,
    Section,
    Text,
    Table,
    Figure,
    Formula,
    Code,
}

/// The model's twenty classes, in its own order.
///
/// This is the whole of the judgement this module makes about a page, and it is
/// made in one place on purpose. Several of these could be argued either way:
/// `header` and `footer` are running matter that a reader would rather not see
/// in the body, and they are read as ordinary text here because every other
/// extractor in this project reads them as ordinary text too — dropping them
/// only in the layout pass would be one rule in one place for no stated reason.
const CLASSES: [RegionKind; 20] = [
    RegionKind::Section, // paragraph_title
    RegionKind::Figure,  // image
    RegionKind::Text,    // text
    RegionKind::Text,    // number
    RegionKind::Text,    // abstract
    RegionKind::Text,    // content
    RegionKind::Text,    // figure_title
    RegionKind::Formula, // formula
    RegionKind::Table,   // table
    RegionKind::Text,    // reference
    RegionKind::Title,   // doc_title
    RegionKind::Text,    // footnote
    RegionKind::Text,    // header
    RegionKind::Code,    // algorithm
    RegionKind::Text,    // footer
    RegionKind::Figure,  // seal
    RegionKind::Figure,  // chart
    RegionKind::Text,    // formula_number
    RegionKind::Text,    // aside_text
    RegionKind::Text,    // reference_content
];

impl From<RegionKind> for BlockKind {
    fn from(kind: RegionKind) -> Self {
        match kind {
            // The model distinguishes a document title from a section title but
            // not one section level from another, so there are two levels and
            // no more. Nesting them by guesswork would be an outline that is
            // wrong in a way nobody can see.
            RegionKind::Title => BlockKind::Heading { level: 1 },
            RegionKind::Section => BlockKind::Heading { level: 2 },
            RegionKind::Text => BlockKind::Paragraph,
            RegionKind::Table => BlockKind::Table,
            RegionKind::Figure => BlockKind::Figure,
            RegionKind::Formula => BlockKind::Formula,
            RegionKind::Code => BlockKind::Code,
        }
    }
}

/// A region the detector found, in page pixels.
///
/// Pixels rather than the IR's normalized coordinates, because everything this
/// module does with them is a comparison against the recogniser's boxes, which
/// are in pixels too. The conversion happens once, on the way out.
///
/// Pixels of the *page*, which is not what the model answers in. See
/// [`Region::on_page`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    kind: RegionKind,
    score: f32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Region {
    fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    fn area(&self) -> f32 {
        self.width().max(0.0) * (self.y1 - self.y0).max(0.0)
    }

    fn holds(&self, x: f32, y: f32) -> bool {
        self.x0 <= x && x <= self.x1 && self.y0 <= y && y <= self.y1
    }

    /// The same region, in the coordinates of the page it came from.
    ///
    /// Two multiplications, because the page was squashed into a square and the
    /// model answers in the square. Both axes are scaled by their own ratio —
    /// this is not the aspect-preserving step, it is the undoing of one that
    /// was never there.
    fn on_page(mut self, width: f32, height: f32) -> Self {
        let (across, down) = (width / SIDE as f32, height / SIDE as f32);

        self.x0 *= across;
        self.x1 *= across;
        self.y0 *= down;
        self.y1 *= down;

        self
    }

    /// How much of `box` this region covers.
    fn overlap(&self, box_: (f32, f32, f32, f32)) -> f32 {
        let (x0, y0, x1, y1) = box_;
        let width = (self.x1.min(x1) - self.x0.max(x0)).max(0.0);
        let height = (self.y1.min(y1) - self.y0.max(y0)).max(0.0);

        width * height
    }
}

/// The model, loaded on the first page that needs it.
///
/// One session rather than the several the recogniser keeps. A page costs about
/// 405 ms here against seconds in the recogniser, so the queue this creates is
/// a tenth of the work it is queued behind; a second copy would be another
/// 124 MB of weights resident to save that tenth.
/// A page as the models want it: three channels, no alpha.
///
/// The image type the pipeline carries is RGBA because that is what a renderer
/// hands out and what an encoder takes; ONNX Runtime is handed RGB because that
/// is what these models were trained on. Both of them need this, so it lives
/// here rather than with either.
pub(crate) fn to_rgb(image: &PageImage) -> image::RgbImage {
    let mut rgb = image::RgbImage::new(image.width, image.height);

    for (pixel, channels) in rgb.pixels_mut().zip(image.pixels.chunks_exact(4)) {
        *pixel = image::Rgb([channels[0], channels[1], channels[2]]);
    }

    rgb
}

pub struct Detector {
    model: PathBuf,
    session: Mutex<Option<Session>>,
}

impl Detector {
    pub fn new(model: PathBuf) -> Self {
        Self {
            model,
            session: Mutex::new(None),
        }
    }

    /// Finds the regions on a page, in page pixels.
    pub fn detect(&self, page: &image::RgbImage) -> Result<Vec<Region>, BackendError> {
        let mut guard = self
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if guard.is_none() {
            *guard = Some(self.load()?);
        }
        let session = guard.as_mut().expect("loaded immediately above");

        // Squashed to a square without keeping the ratio, which is what the
        // model was trained on — and it answers in that square's coordinates,
        // not the page's. See `Region::on_page`.
        let square = image::imageops::resize(
            page,
            SIDE as u32,
            SIDE as u32,
            image::imageops::FilterType::Triangle,
        );
        let pixels_count = SIDE * SIDE;

        // Planar, not interleaved, and scaled to 0-1. The model's own
        // `inference.yml` says mean 0 and std 1, which reads as "leave the
        // bytes alone" and gives a best score of 0.049 and no boxes at all.
        // Measured against the same page, this gives 0.985.
        let raw = square.as_raw();
        let mut pixels = vec![0.0f32; 3 * pixels_count];
        for (index, pixel) in raw.chunks_exact(3).enumerate() {
            for (channel, byte) in pixel.iter().enumerate() {
                pixels[channel * pixels_count + index] = f32::from(*byte) / 255.0;
            }
        }

        // `im_shape` and `scale_factor` are what the model is meant to map its
        // boxes back through, and what they actually do is scale them by the
        // factor twice over: told a page is 2200 by 1700, it puts a title 4.5
        // times too far right and 7.6 times too far down, which is the square
        // ratio squared each way. So it is told the truth — that it was given
        // an 800×800 square, which it was — and the page is put back below,
        // where the arithmetic is two multiplications and can be read.
        //
        // This is worth stating because the reference implementation passes the
        // page's shape here and uses the boxes it gets back, and the boxes it
        // gets back are wrong by exactly this much. On a page close to square
        // they land near enough to their region to look right in a drawing.
        let shape = [SIDE as f32, SIDE as f32];
        let factor = [1.0f32, 1.0];

        let image = TensorRef::from_array_view(([1usize, 3, SIDE, SIDE], &pixels[..]))
            .map_err(|error| BackendError::Failed(error.to_string()))?;
        let shape = TensorRef::from_array_view(([1usize, 2], &shape[..]))
            .map_err(|error| BackendError::Failed(error.to_string()))?;
        let scale = TensorRef::from_array_view(([1usize, 2], &factor[..]))
            .map_err(|error| BackendError::Failed(error.to_string()))?;

        let outputs = session
            .run(ort::inputs![
                "image" => image,
                "im_shape" => shape,
                "scale_factor" => scale
            ])
            .map_err(|error| {
                BackendError::Failed(format!("the structure model failed on this page: {error}"))
            })?;

        let (_, found) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|error| BackendError::Failed(error.to_string()))?;

        let width = page.width() as f32;
        let height = page.height() as f32;

        Ok(decode(found)
            .into_iter()
            .map(|region| region.on_page(width, height))
            .collect())
    }

    fn load(&self) -> Result<Session, BackendError> {
        // Offered rather than chosen. Whether this machine has a DirectML
        // device is answered by handing one to ONNX Runtime and seeing what
        // comes back, which is the same answer as inspecting the machine and a
        // great deal less code — the design's rule about probing backends
        // rather than detecting hardware.
        //
        // A machine without one is the ordinary case and not an error, so a
        // build that fails there falls back to the processor rather than
        // refusing to read the page. What it must not do is fall back
        // *silently*: nothing downstream can tell which provider ran, which is
        // the reason this is off by default rather than on.
        #[cfg(feature = "directml")]
        if let Ok(session) = self.build(true) {
            return Ok(session);
        }

        self.build(false).map_err(|error| self.failed(error))
    }

    #[cfg_attr(not(feature = "directml"), allow(unused_mut, unused_variables))]
    fn build(&self, on_gpu: bool) -> ort::Result<Session> {
        let mut builder = Session::builder()?;

        #[cfg(feature = "directml")]
        if on_gpu {
            builder = builder.with_execution_providers([
                ort::ep::DirectML::default().build().error_on_failure(),
            ])?;
        }

        builder.commit_from_file(&self.model)
    }

    fn failed(&self, error: impl std::fmt::Display) -> BackendError {
        BackendError::Unavailable(format!(
            "the structure model at {} could not be loaded: {error}",
            self.model.display()
        ))
    }
}

/// Whether the structure model is here, checked once for the process.
///
/// The check reads 124 MB, so it is made once rather than per page or per
/// backend. It is what decides whether the local engine calls itself
/// `rapidocr` or `rapidocr+layout`, and that name is the cache key: a run with
/// this model and a run without it produce different output for the same bytes,
/// and must not be served each other's. Which is also why the answer is taken
/// before any work rather than when the first page needs it — an id that
/// changed half way through a run would be two names for one run's output.
///
/// The cost of deciding it once is that fetching the model while a window is
/// open does not reach that window until it is restarted. That is why
/// [`forget`] exists: the window fetches the model itself, so the process that
/// downloaded it is the process that has to notice, and a button that appeared
/// to work but changed nothing until the next launch would be a button that
/// did not work.
///
/// Between documents is early enough. The name is per page, and a page's name
/// is taken from the backend that read it, so nothing changes under a read in
/// progress — the next document simply gets the other name.
pub fn available() -> bool {
    AVAILABLE.get(|| layout::present(&layout::default_root()))
}

/// Forgets what was found, so the next question looks at the disk again.
///
/// For the one caller that can change the answer: a fetch that just wrote the
/// model. Between documents is early enough, for the reason above.
pub fn forget() {
    AVAILABLE.forget()
}

/// The answer to [`available`], once it has been asked and until it is
/// forgotten.
static AVAILABLE: layout::Cached = layout::Cached::new();

/// Where the model would be, whether or not it is.
pub fn model_path() -> PathBuf {
    layout::path_in(&layout::default_root())
}

/// Reads the model's output.
///
/// Flat, six numbers per region: the class, the score, and the two corners.
/// The boxes come back in the coordinates of the original page rather than of
/// the 800×800 square, because the model was told how to map them back.
fn decode(found: &[f32]) -> Vec<Region> {
    found
        .chunks_exact(6)
        .filter_map(|row| {
            if row[1] < FLOOR {
                return None;
            }

            let label = row[0].round();
            if label < 0.0 {
                return None;
            }

            Some(Region {
                kind: *CLASSES.get(label as usize)?,
                score: row[1],
                x0: row[2],
                y0: row[3],
                x1: row[4],
                y1: row[5],
            })
        })
        .collect()
}

/// Puts the recognised words inside the regions that were found.
///
/// The whole of the join, and pure: two lists in, blocks out. Everything hard
/// about it is geometry, which is exactly the part that should not need a
/// quarter of a gigabyte of weights on disk to be tested.
pub fn blocks_from(
    lines: &[OcrLine],
    regions: &[Region],
    tables: Option<Tables<'_>>,
    width: u32,
    height: u32,
    engine: &EngineId,
) -> Vec<Block> {
    let lines: Vec<&OcrLine> = lines
        .iter()
        .filter(|line| !line.text.trim().is_empty())
        .collect();

    let mut held: Vec<Vec<usize>> = vec![Vec::new(); regions.len()];
    let mut loose: Vec<usize> = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        match region_for(extent(line), regions) {
            Some(region) => held[region].push(index),
            None => loose.push(index),
        }
    }

    // Ordered by the piece of the page each came from and not by the words
    // inside it, so that a short line hanging above its region cannot reorder
    // two regions. A word the detector put nowhere is still a word on the page,
    // so it goes through the same ordering as the rest.
    let mut boxes: Vec<(f32, f32, f32, f32)> = Vec::new();
    let mut blocks: Vec<Block> = Vec::new();

    for (region, indices) in regions.iter().zip(&held) {
        if let Some(block) = region_block(region, indices, &lines, tables, width, height, engine) {
            boxes.push((region.x0, region.y0, region.x1, region.y1));
            blocks.push(block);
        }
    }

    for index in loose {
        blocks.push(one_block(
            BlockKind::Paragraph,
            std::slice::from_ref(&index),
            &lines,
            width,
            height,
            engine,
        ));
        boxes.push(extent(lines[index]));
    }

    let mut waiting: Vec<Option<Block>> = blocks.into_iter().map(Some).collect();

    page_order(&boxes, width as f32, height as f32)
        .into_iter()
        .filter_map(|index| waiting[index].take())
        .collect()
}

/// The order a person reads a page in.
///
/// Top to bottom and then left to right is right for one column and wrong for
/// two: on a two-column page it reads the left column's first line, the right
/// column's first line, then the left column's second. **Measured on the corpus,
/// the characters survive that and the meaning does not** — a Chinese exam
/// paper scores 0.63 on content and 0.85 once order counts, and a Chinese
/// newspaper 0.03 and 0.80. That gap is this function's whole reason.
///
/// So the page is cut into columns first and into bands second. Anything
/// spanning the width — a title, an abstract, a full-width figure — separates
/// the bands, and between two of them the columns are read one after another,
/// which is what a person does with a paper or a newspaper.
///
/// The single-column case is answered by the same code it always was, to the
/// comparison, so a page this cannot help is not a page it can hurt.
fn page_order(boxes: &[(f32, f32, f32, f32)], page_width: f32, page_height: f32) -> Vec<usize> {
    let mut order: Vec<usize> = (0..boxes.len()).collect();

    if boxes.len() < 2 {
        return order;
    }

    let columns = page_columns(boxes, page_width, page_height);
    if columns.len() < 2 {
        order.sort_by(|left, right| upper_then_left(boxes, *left, *right));
        return order;
    }

    let spanning = page_width * SPANNING;
    let (mut wide, narrow): (Vec<usize>, Vec<usize>) = order
        .into_iter()
        .partition(|index| boxes[*index].2 - boxes[*index].0 >= spanning);

    wide.sort_by(|left, right| boxes[*left].1.total_cmp(&boxes[*right].1));

    let mut out = Vec::with_capacity(boxes.len());
    let mut top = f32::NEG_INFINITY;

    for band in wide {
        columns_between(&mut out, &narrow, &columns, boxes, top, boxes[band].1);
        out.push(band);
        top = boxes[band].3;
    }

    columns_between(&mut out, &narrow, &columns, boxes, top, f32::INFINITY);

    out
}

/// Appends the boxes whose middle falls between two bands, a column at a time.
///
/// The middle rather than the top, because a box that crosses a band boundary
/// belongs to whichever side it is mostly on, and a top edge can be above a
/// band without any of the box being in it.
fn columns_between(
    out: &mut Vec<usize>,
    narrow: &[usize],
    columns: &[(f32, f32)],
    boxes: &[(f32, f32, f32, f32)],
    from: f32,
    to: f32,
) {
    for column in 0..columns.len() {
        let mut inside: Vec<usize> = narrow
            .iter()
            .copied()
            .filter(|index| {
                let (x0, y0, x1, y1) = boxes[*index];
                let middle = (y0 + y1) / 2.0;

                middle >= from
                    && middle < to
                    && column_of((x0 + x1) / 2.0, columns) == column
            })
            .collect();

        inside.sort_by(|left, right| boxes[*left].1.total_cmp(&boxes[*right].1));
        out.extend(inside);
    }
}

/// The vertical corridors of white that run the length of a page.
///
/// A corridor is measured, not inferred: the page is cut into narrow strips and
/// each strip is asked **how much of the page's height is covered at this x**.
/// A gutter is a run of strips that is almost entirely empty.
///
/// **The obvious way is to merge the boxes' horizontal spans and look at the
/// gaps, and a real page shows why that is wrong.** On the first two-column page
/// measured the columns are 39% of the width each with a clean 26-pixel corridor
/// between them, and a **running head nineteen pixels tall** lying across the top
/// of that corridor welded the two columns into one — the page is 2017 pixels
/// tall and the head covers 0.9% of it. A box that crosses a gap is not evidence
/// that the gap is filled, and counting spans cannot tell the two apart.
fn page_columns(
    boxes: &[(f32, f32, f32, f32)],
    page_width: f32,
    page_height: f32,
) -> Vec<(f32, f32)> {
    /// Thin enough that a gutter is several strips wide, few enough to be free.
    const STRIPS: usize = 256;

    let strip = page_width / STRIPS as f32;
    if strip <= 0.0 || page_height <= 0.0 {
        return vec![(0.0, page_width)];
    }

    let mut covered = [0.0f32; STRIPS];
    for &(x0, y0, x1, y1) in boxes {
        let tall = (y1 - y0).max(0.0);
        if tall <= 0.0 || x1 <= x0 {
            continue;
        }

        let first = (x0 / strip).floor().max(0.0) as usize;
        let last = (x1 / strip).ceil().max(0.0) as usize;

        for height in covered.iter_mut().take(last.min(STRIPS)).skip(first.min(STRIPS)) {
            *height += tall;
        }
    }

    let empty = page_height * COLUMN_FILL;
    let mut columns: Vec<(f32, f32)> = Vec::new();

    for (index, height) in covered.iter().enumerate() {
        if *height <= empty {
            continue;
        }

        let left = index as f32 * strip;

        match columns.last_mut() {
            // White inside a column is not a gutter when the column carries on
            // the other side of it, which is what a paragraph break looks like.
            Some(last) if left - last.1 < page_width * COLUMN_GUTTER => {
                last.1 = left + strip;
            }
            _ => columns.push((left, left + strip)),
        }
    }

    if columns.is_empty() {
        vec![(0.0, page_width)]
    } else {
        columns
    }
}

fn upper_then_left(
    boxes: &[(f32, f32, f32, f32)],
    left: usize,
    right: usize,
) -> std::cmp::Ordering {
    boxes[left]
        .1
        .total_cmp(&boxes[right].1)
        .then(boxes[left].0.total_cmp(&boxes[right].0))
}

/// Which region a recognised line belongs to.
///
/// The centre decides, because a line sits inside the region that holds it and
/// only grazes the neighbours. Falling back to the greatest overlap and then to
/// nothing at all keeps a line that straddles two regions from vanishing.
fn region_for(box_: (f32, f32, f32, f32), regions: &[Region]) -> Option<usize> {
    let (x0, y0, x1, y1) = box_;
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);

    let mut smallest: Option<(usize, f32)> = None;
    for (index, region) in regions.iter().enumerate() {
        if region.holds(cx, cy) && smallest.is_none_or(|(_, area)| region.area() < area) {
            smallest = Some((index, region.area()));
        }
    }
    if let Some((index, _)) = smallest {
        return Some(index);
    }

    let mut best: Option<(usize, f32)> = None;
    for (index, region) in regions.iter().enumerate() {
        let overlap = region.overlap(box_);
        if overlap > 0.0 && best.is_none_or(|(_, area)| overlap > area) {
            best = Some((index, overlap));
        }
    }

    best.map(|(index, _)| index)
}

/// What the table model needs, when it is here.
///
/// Both together or neither: a grid of cells is no use without the picture it
/// was read from, and both are absent on an installation that has not
/// downloaded the model. Which is the ordinary case, and it is why everything
/// below has a fallback rather than an error.
#[derive(Clone, Copy)]
pub struct Tables<'a> {
    pub page: &'a image::RgbImage,
    pub reader: &'a table::Reader,
}

/// The block a region becomes, if it becomes one.
fn region_block(
    region: &Region,
    indices: &[usize],
    lines: &[&OcrLine],
    tables: Option<Tables<'_>>,
    width: u32,
    height: u32,
    engine: &EngineId,
) -> Option<Block> {
    if indices.is_empty() {
        // A figure with no words in it is still worth carrying: its box is
        // where the picture is, and a pass that extracts assets will read that
        // box. Every other empty region is a region with nothing to say, and a
        // heading with no text renders as a bare `#`.
        return (region.kind == RegionKind::Figure).then(|| Block {
            kind: BlockKind::Figure,
            text: String::new(),
            bbox: Some(normalized(region.x0, region.y0, region.x1, region.y1, width, height)),
            confidence: Some(region.score),
            engine: engine.clone(),
            children: Vec::new(),
        });
    }

    if region.kind == RegionKind::Table {
        // The model first, and the geometry under it. The model reads a grid
        // and a box for every cell, which is the half of a table that
        // whitespace cannot give; the geometry is what the project had before
        // it and still answers when the model is not here or does not
        // understand the table.
        if let Some(found) = tables
            .and_then(|tables| laid_out(region, indices, lines, tables, width, height, engine))
        {
            return Some(found);
        }

        if let Some(geometry) = table_block(region, indices, lines, width, height, engine) {
            return Some(geometry);
        }
    }

    // Including a table that could not be reconstructed, and this is the case
    // that matters: a `Table` with no rows renders as nothing at all, so a
    // region called a table and built as a paragraph is the difference between
    // words in the document and words gone. The kind falls with it — a list of
    // one column is a paragraph, and saying otherwise would be a table that
    // isn't there.
    let kind = match region.kind {
        RegionKind::Table => BlockKind::Paragraph,
        other => other.into(),
    };

    Some(one_block(kind, indices, lines, width, height, engine))
}

/// One block from the lines that belong to it, joined in reading order.
fn one_block(
    kind: BlockKind,
    indices: &[usize],
    lines: &[&OcrLine],
    width: u32,
    height: u32,
    engine: &EngineId,
) -> Block {
    let text = reading_order(indices, lines)
        .into_iter()
        .map(|index| lines[index].text.trim())
        .collect::<Vec<_>>()
        .join("\n");

    Block {
        kind,
        text,
        bbox: Some(union(indices, lines, width, height)),
        // The least confident line in it. A block only as sure as its first
        // line would let an uncertain one hide behind a confident one, which is
        // exactly what a policy reading confidence to decide whether to check
        // the page must not be told.
        confidence: Some(least_confident(indices, lines)),
        engine: engine.clone(),
        children: Vec::new(),
    }
}

/// A table, as rows and cells.
///
/// The columns are worked out once for the table and not once per row. That is
/// the whole of it: a row-by-row reading gives rows of different lengths, and a
/// Markdown table with a row of the wrong length is silently padded into
/// something that looks deliberate and is not.
fn table_block(
    region: &Region,
    indices: &[usize],
    lines: &[&OcrLine],
    width: u32,
    height: u32,
    engine: &EngineId,
) -> Option<Block> {
    // One line is a paragraph that happens to sit in a table's box, and one
    // column is a list. Neither is a grid, and calling either one a table would
    // put a Markdown table in the document that says nothing a sentence would
    // not have said better.
    if indices.len() < 2 {
        return None;
    }

    let columns = columns_of(region, indices, lines);
    if columns.len() < 2 {
        return None;
    }

    let pieces = pieces_of(&columns, indices, lines);

    let mut children: Vec<Block> = Vec::new();
    let mut body: Vec<String> = Vec::new();

    for row in stack(&pieces) {
        let mut cells: Vec<Block> = Vec::with_capacity(columns.len());
        let mut written: Vec<String> = Vec::with_capacity(columns.len());

        for column in 0..columns.len() {
            let held = row
                .iter()
                .map(|index| &pieces[*index])
                .find(|piece| piece.column == column);

            let text = held
                .map(|piece| piece.text(lines))
                .unwrap_or_default();

            written.push(text.clone());
            cells.push(Block {
                kind: BlockKind::TableCell,
                text,
                // Empty for a cell the table does not have. There is nothing
                // there to have been sure about.
                bbox: held.map(|piece| union(&piece.lines, lines, width, height)),
                confidence: held.map(|piece| least_confident(&piece.lines, lines)),
                engine: engine.clone(),
                children: Vec::new(),
            });
        }

        let covered: Vec<usize> = row
            .iter()
            .flat_map(|index| pieces[*index].lines.iter().copied())
            .collect();

        body.push(written.join(" "));

        children.push(Block {
            kind: BlockKind::TableRow,
            text: String::new(),
            bbox: Some(union(&covered, lines, width, height)),
            confidence: Some(least_confident(&covered, lines)),
            engine: engine.clone(),
            children: cells,
        });
    }

    Some(Block {
        kind: BlockKind::Table,
        // The cells again, in a form that can be searched and compared. The
        // renderer reads the children, but everything downstream of the
        // renderer reads text, and a table with none would be a hole.
        text: body.join("\n"),
        bbox: Some(normalized(region.x0, region.y0, region.x1, region.y1, width, height)),
        confidence: Some(least_confident(indices, lines)),
        engine: engine.clone(),
        children,
    })
}

/// A table the model read, as rows and cells.
///
/// The words are the same words the geometry would have used; what is different
/// is where they go. The model answers with a box for every cell, so a line
/// belongs to the cell that holds it — no columns to infer, no gutter to guess,
/// and an eleven-column statement comes out with its columns intact.
///
/// `None` when the model has nothing to say, which leaves the caller its
/// geometry rather than a table with no cells in it.
fn laid_out(
    region: &Region,
    indices: &[usize],
    lines: &[&OcrLine],
    tables: Tables<'_>,
    width: u32,
    height: u32,
    engine: &EngineId,
) -> Option<Block> {
    let left = region.x0.max(0.0) as u32;
    let top = region.y0.max(0.0) as u32;
    let across = (region.x1.ceil() as u32).saturating_sub(left);
    let down = (region.y1.ceil() as u32).saturating_sub(top);

    if across < 4 || down < 4 {
        return None;
    }

    let picture = image::imageops::crop_imm(tables.page, left, top, across, down).to_image();
    let grid = tables.reader.grid(&picture).ok()?;

    laid_out_grid(
        Sheet {
            grid: &grid,
            left: left as f32,
            top: top as f32,
        },
        region,
        indices,
        lines,
        width,
        height,
        engine,
    )
}

/// Where every cell sits, once the rows a cell reaches down into are counted.
///
/// One entry per **grid position**, row by row: the cell that holds it, and how
/// many columns that cell covers, or `None` for a position that is the inside
/// of a cell which started in a row above.
///
/// The rows below a tall cell are the whole reason this is not just a running
/// column count. A cell covering three rows holds one column in each of them,
/// and a row that did not know that would start its own cells three columns too
/// early: measured on an equipment register with one merged label, the label
/// landed in the header and the three rows under it lost their item and their
/// count.
///
/// The positions inside a cell are kept as empty ones rather than filled in.
/// Markdown cannot say "this cell covers three rows" — GFM has no span of any
/// kind — so the choice is between an empty position and the same words written
/// three times, and writing words the page does not have is the one thing this
/// project does not do.
fn seatings(rows: &[Vec<table::Cell>]) -> Vec<Vec<Option<(usize, usize)>>> {
    let mut seats: Vec<Vec<Option<(usize, usize)>>> = Vec::new();

    // By column, the first row the cell there is **not** in: zero for a column
    // nothing reaches into. An end rather than a countdown, because a countdown
    // has to be decremented once per row and the row that decrements the value
    // it just wrote is a row that erases its own span.
    let mut through: Vec<usize> = Vec::new();

    for (index_of_row, cells) in rows.iter().enumerate() {
        let mut row: Vec<Option<(usize, usize)>> = Vec::new();
        let mut column = 0;

        for (index, cell) in cells.iter().enumerate() {
            // Past the columns a cell above still holds.
            while through
                .get(column)
                .is_some_and(|end| index_of_row < *end)
            {
                row.push(None);
                column += 1;
            }

            let across = cell.across.max(1);
            let down = cell.down.max(1);

            while row.len() < column + across {
                row.push(None);
            }
            row[column] = Some((index, across));

            if down > 1 {
                through.resize(through.len().max(column + across), 0);
                for held in through[column..column + across].iter_mut() {
                    *held = (*held).max(index_of_row + down);
                }
            }

            column += across;
        }

        // A row can end before the columns a cell above still holds.
        while through
            .get(row.len())
            .is_some_and(|end| index_of_row < *end)
        {
            row.push(None);
        }

        seats.push(row);
    }

    seats
}

/// Where one cell sits: in the grid, and on the page.
#[derive(Clone, Copy)]
struct Place {
    row: usize,
    column: usize,
    at: (f32, f32, f32, f32),
}

/// A grid the model answered with, and where on the page it came from.
///
/// The model reads a crop and answers in the crop's coordinates; everything
/// else here speaks the page's, and the difference is these two numbers.
#[derive(Clone, Copy)]
struct Sheet<'a> {
    grid: &'a table::Grid,
    left: f32,
    top: f32,
}

/// A grid the model answered with, as blocks.
///
/// Separate from the reading so that it can be tested without a model: what the
/// words do once the boxes are known is a pure question about geometry, and the
/// part that needs 7.4 MB of weights is six lines above this.
fn laid_out_grid(
    sheet: Sheet<'_>,
    region: &Region,
    indices: &[usize],
    lines: &[&OcrLine],
    width: u32,
    height: u32,
    engine: &EngineId,
) -> Option<Block> {
    let (grid, left, top) = (sheet.grid, sheet.left, sheet.top);

    if grid.rows.is_empty() || grid.columns() < 2 {
        return None;
    }

    // The model's boxes are in the crop's coordinates and the rest of this
    // module speaks the page's.
    let on_page = |token: usize| -> Option<(f32, f32, f32, f32)> {
        let (x0, y0, x1, y1) = grid.boxes.get(token).copied()?;
        Some((x0 + left, y0 + top, x1 + left, y1 + top))
    };

    // Where every cell sits in the grid, worked out before a single word is
    // placed, because a line has to be able to choose between all of them.
    let seats = seatings(&grid.rows);
    let widest = seats.iter().map(Vec::len).max().unwrap_or(0);

    let mut places: Vec<Place> = Vec::new();

    for (row, seats) in seats.iter().enumerate() {
        for (column, seat) in seats.iter().enumerate() {
            let Some((cell, _)) = seat else {
                continue;
            };

            if let Some(at) = on_page(grid.rows[row][*cell].token) {
                places.push(Place { row, column, at });
            }
        }
    }

    let mut written: Vec<Vec<Vec<usize>>> = seats
        .iter()
        .map(|row| row.iter().map(|_| Vec::new()).collect())
        .collect();

    for index in indices {
        let line = extent(lines[*index]);
        let (cx, cy) = ((line.0 + line.2) / 2.0, (line.1 + line.3) / 2.0);

        // The cell that holds most of it, and failing that the nearest, because
        // a word inside a table belongs to a cell even when the model drew the
        // cell slightly wrong — and a word outside every cell is still a word,
        // which the geometry this replaced never lost and this must not either.
        let mut best = (f32::NEG_INFINITY, None);
        for place in &places {
            let at = place.at;
            let held = ((line.2.min(at.2) - line.0.max(at.0)).max(0.0))
                * ((line.3.min(at.3) - line.1.max(at.1)).max(0.0));

            let away = ((cx - (at.0 + at.2) / 2.0).powi(2) + (cy - (at.1 + at.3) / 2.0).powi(2))
                .sqrt();

            // Overlap first and distance second, in one number: a cell holding
            // the line always beats a cell holding none of it.
            let score = if held > 0.0 { held } else { -(away + 1.0) };

            if score > best.0 {
                best = (score, Some((place.row, place.column)));
            }
        }

        if let Some((row, column)) = best.1
            && let Some(place) = written.get_mut(row).and_then(|cells| cells.get_mut(column))
        {
            place.push(*index);
        }
    }

    let mut children: Vec<Block> = Vec::new();
    let mut body: Vec<String> = Vec::new();

    // Every row the model opened is kept, including the empty ones. Dropping
    // them looks like an obvious tidy — a blank line between every pair of rows
    // is not a table anybody can read — and **measured, it took the tables from
    // seven of nine back to one**, because the model puts its header in a row
    // of its own and a row with no words in it is still the row the headings
    // are above. Kept, with a note, rather than re-learned.
    for (row, cells) in written.iter().enumerate() {
        let mut blocks: Vec<Block> = Vec::with_capacity(widest);
        let mut texts: Vec<String> = Vec::with_capacity(widest);

        for (column, inside) in cells.iter().enumerate() {
            let held = places
                .iter()
                .find(|place| place.row == row && place.column == column)
                .map(|place| place.at);


            let inside: Vec<usize> = inside.clone();
            let text = inside
                .iter()
                .map(|index| lines[*index].text.trim())
                .collect::<Vec<_>>()
                .join(" ");

            texts.push(text.clone());
            blocks.push(Block {
                kind: BlockKind::TableCell,
                text,
                bbox: held.map(|(x0, y0, x1, y1)| normalized(x0, y0, x1, y1, width, height)),
                confidence: (!inside.is_empty()).then(|| least_confident(&inside, lines)),
                engine: engine.clone(),
                children: Vec::new(),
            });
        }

        // Short of the widest row, so that the cells line up.
        while texts.len() < widest {
            texts.push(String::new());
            blocks.push(Block {
                kind: BlockKind::TableCell,
                text: String::new(),
                bbox: None,
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
            children: blocks,
        });
    }

    Some(Block {
        kind: BlockKind::Table,
        text: body.join("
"),
        bbox: Some(normalized(region.x0, region.y0, region.x1, region.y1, width, height)),
        confidence: Some(least_confident(indices, lines)),
        engine: engine.clone(),
        children,
    })
}

/// The tables on a page, found by the layout model and read by the table model,
/// with the words supplied by whoever called.
///
/// The same two models the recognition path uses and, below them, deliberately
/// the same code: where a word goes in a cell is one question with one answer,
/// and answering it twice is how the two answers come to differ. What changes is
/// where the words come from. A page of a scan has to be recognised; a page of a
/// PDF that carries a text layer already says what it is, and recognising it
/// again would be inventing a worse copy of what the file holds.
pub struct PageTables {
    regions: Detector,
    cells: table::Reader,
}

impl PageTables {
    pub fn new(regions: Detector, cells: table::Reader) -> Self {
        Self { regions, cells }
    }
}

impl TableReader for PageTables {
    fn id(&self) -> &'static str {
        "tables"
    }

    fn tables(
        &self,
        page: &PageImage,
        lines: &[TextLine],
    ) -> Result<Vec<FoundTable>, ExtractError> {
        let rgb = to_rgb(page);
        let (width, height) = (rgb.width(), rgb.height());

        // The words as the geometry below wants them. The score is one because
        // nothing here was recognised, and nothing below reads it as a
        // recognition confidence — it only has to not be zero.
        let given: Vec<OcrLine> = lines
            .iter()
            .map(|line| OcrLine {
                bbox: Quad::from_xyxy(line.bbox.0, line.bbox.1, line.bbox.2, line.bbox.3),
                text: line.text.clone(),
                score: 1.0,
            })
            .collect();
        let given: Vec<&OcrLine> = given.iter().collect();

        let regions = self
            .regions
            .detect(&rgb)
            .map_err(|error| ExtractError::failed("tables", error.to_string()))?;

        let engine = EngineId::from("tables");
        let mut found = Vec::new();

        for (index, region) in regions.iter().enumerate() {
            if region.kind != RegionKind::Table {
                continue;
            }

            // The lines this table holds and no others, by the same rule the
            // recognition path holds them by.
            let inside: Vec<usize> = (0..given.len())
                .filter(|at| region_for(extent(given[*at]), &regions) == Some(index))
                .collect();

            if inside.is_empty() {
                continue;
            }

            let Some(block) = laid_out(
                region,
                &inside,
                &given,
                Tables {
                    page: &rgb,
                    reader: &self.cells,
                },
                width,
                height,
                &engine,
            ) else {
                continue;
            };

            found.push(as_found(&block, width, height));
        }

        Ok(found)
    }
}

/// The block the floor below built, as the caller's own shape.
///
/// The boxes are normalized because that is what the IR speaks and the caller
/// works in pixels of the page it rendered, so this is the one place the two
/// meet.
fn as_found(block: &Block, width: u32, height: u32) -> FoundTable {
    let pixels = |rect: Option<Rect>| {
        rect.map(|rect| {
            (
                rect.x * width as f32,
                rect.y * height as f32,
                (rect.x + rect.w) * width as f32,
                (rect.y + rect.h) * height as f32,
            )
        })
    };

    let rows = block
        .children
        .iter()
        .filter(|child| child.kind == BlockKind::TableRow)
        .map(|row| {
            row.children
                .iter()
                .map(|cell| FoundCell {
                    text: cell.text.clone(),
                    bbox: pixels(cell.bbox),
                })
                .collect()
        })
        .collect();

    FoundTable {
        bbox: pixels(block.bbox).unwrap_or((0.0, 0.0, width as f32, height as f32)),
        rows,
    }
}

/// Where the columns are.
///
/// The gaps that run through the whole table, and nothing else. Every line in a
/// column covers some of the same horizontal space as every other line in it,
/// so the columns are what is left when the lines' spans are merged and the
/// gaps between them are the gutters.
///
/// A gap has to be at least a line high to count. Two words in the same cell
/// are never that far apart, and the gap between two columns always is, which
/// makes the line height the one scale a page carries with it.
fn columns_of(region: &Region, indices: &[usize], lines: &[&OcrLine]) -> Vec<(f32, f32)> {
    let gutter = median_height(indices, lines);
    let spanning = region.width() * SPANNING;

    let mut spans: Vec<(f32, f32)> = indices
        .iter()
        .map(|index| (left_of(lines[*index]), right_of(lines[*index])))
        .filter(|(left, right)| right - left < spanning)
        .collect();

    if spans.is_empty() {
        return vec![(region.x0, region.x1)];
    }

    spans.sort_by(|left, right| left.0.total_cmp(&right.0));

    let mut columns: Vec<(f32, f32)> = vec![spans[0]];
    for (left, right) in &spans[1..] {
        let last = columns.last_mut().expect("just pushed");
        if left - last.1 < gutter {
            last.1 = last.1.max(*right);
        } else {
            columns.push((*left, *right));
        }
    }

    columns
}

/// Which column a line sits in, by its centre.
///
/// The nearest column as well as the containing one, because a line whose box
/// overhangs its region would otherwise belong to nothing and disappear from
/// the table it is a part of.
fn column_of(centre: f32, columns: &[(f32, f32)]) -> usize {
    let mut best = 0;
    let mut distance = f32::INFINITY;

    for (index, (left, right)) in columns.iter().enumerate() {
        if *left <= centre && centre <= *right {
            return index;
        }

        let to = if centre < *left {
            *left - centre
        } else {
            centre - *right
        };
        if to < distance {
            distance = to;
            best = index;
        }
    }

    best
}

/// One cell's worth of lines, and which column it is in.
///
/// Cells before rows, and that order is the whole of why a wrapped cell stays
/// one cell. Two lines of one cell sit on top of each other and do not overlap
/// at all, so read as lines first they are two rows — a row of `Widgets and`
/// and a row of `gadgets`, with the rest of the table empty beside them.
/// Grouped into a cell first, its box is as tall as both lines and the columns
/// beside it land on it and not on one half of it.
struct Piece {
    column: usize,
    top: f32,
    bottom: f32,
    left: f32,
    lines: Vec<usize>,
}

impl Piece {
    /// A cell's own lines are one wrapped sentence, so a space. A cell cannot
    /// hold a newline: Markdown would break the row in two if it did.
    fn text(&self, lines: &[&OcrLine]) -> String {
        self.lines
            .iter()
            .map(|index| lines[*index].text.trim())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn pieces_of(columns: &[(f32, f32)], indices: &[usize], lines: &[&OcrLine]) -> Vec<Piece> {
    let mut within: Vec<Vec<usize>> = vec![Vec::new(); columns.len()];
    for index in indices {
        within[column_of(centre_x(lines[*index]), columns)].push(*index);
    }

    let mut pieces: Vec<Piece> = Vec::new();

    for (column, mut held) in within.into_iter().enumerate() {
        held.sort_by(|left, right| {
            top_of(lines[*left])
                .total_cmp(&top_of(lines[*right]))
                .then(left_of(lines[*left]).total_cmp(&left_of(lines[*right])))
        });

        let wrap = median_height(&held, lines) * WRAP_GAP;
        let mut piece: Option<Piece> = None;

        for index in held {
            let (top, bottom) = (top_of(lines[index]), bottom_of(lines[index]));

            match &mut piece {
                // Still the same cell: nothing to write with between the two.
                Some(piece) if top - piece.bottom < wrap => {
                    piece.bottom = piece.bottom.max(bottom);
                    piece.lines.push(index);
                }
                _ => {
                    // The previous cell is done with.
                    if let Some(finished) = piece.take() {
                        pieces.push(finished);
                    }

                    piece = Some(Piece {
                        column,
                        top,
                        bottom,
                        left: left_of(lines[index]),
                        lines: vec![index],
                    });
                }
            }
        }

        if let Some(finished) = piece {
            pieces.push(finished);
        }
    }

    pieces
}

/// Groups cells into rows by vertical overlap.
///
/// A row is read off a cell and not off a line, so a row that is tall because
/// one of its cells wraps still meets the single-line cells beside it.
fn stack(pieces: &[Piece]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..pieces.len()).collect();
    order.sort_by(|left, right| {
        pieces[*left]
            .top
            .total_cmp(&pieces[*right].top)
            .then(pieces[*left].left.total_cmp(&pieces[*right].left))
    });

    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut band: Option<(f32, f32)> = None;

    for index in order {
        let piece = &pieces[index];

        match band {
            Some((top, bottom)) if same_row(piece.top, piece.bottom, top, bottom) => {
                rows.last_mut().expect("a band belongs to a row").push(index);
                band = Some((top.min(piece.top), bottom.max(piece.bottom)));
            }
            _ => {
                rows.push(vec![index]);
                band = Some((piece.top, piece.bottom));
            }
        }
    }

    rows
}

/// Whether a line sits on the same row as the one being built.
///
/// Compared against the row's own extent rather than the previous line's, so
/// that a row of mixed type sizes does not chain away down the page.
fn same_row(top: f32, bottom: f32, band_top: f32, band_bottom: f32) -> bool {
    let overlap = bottom.min(band_bottom) - top.max(band_top);
    let shortest = (bottom - top).min(band_bottom - band_top);

    shortest > 0.0 && overlap > shortest * 0.5
}

fn reading_order(indices: &[usize], lines: &[&OcrLine]) -> Vec<usize> {
    let mut order: Vec<usize> = indices.to_vec();
    order.sort_by(|left, right| {
        top_of(lines[*left])
            .total_cmp(&top_of(lines[*right]))
            .then(left_of(lines[*left]).total_cmp(&left_of(lines[*right])))
    });

    order
}

/// The boxes of `indices`, together, in the IR's coordinates.
fn union(indices: &[usize], lines: &[&OcrLine], width: u32, height: u32) -> Rect {
    let mut list = indices.iter().map(|index| extent(lines[*index]));
    let (x0, y0, mut x1, mut y1) = list.next().unwrap_or((0.0, 0.0, 0.0, 0.0));

    for (_, _, right, bottom) in list {
        x1 = x1.max(right);
        y1 = y1.max(bottom);
    }

    normalized(x0, y0, x1, y1, width, height)
}

fn least_confident(indices: &[usize], lines: &[&OcrLine]) -> f32 {
    indices
        .iter()
        .map(|index| lines[*index].score.clamp(0.0, 1.0))
        .fold(1.0, f32::min)
}

/// The height a page's lines are, in the middle of the ones that are here.
fn median_height(indices: &[usize], lines: &[&OcrLine]) -> f32 {
    let mut heights: Vec<f32> = indices
        .iter()
        .map(|index| {
            let (_, top, _, bottom) = extent(lines[*index]);
            bottom - top
        })
        .filter(|height| *height > 0.0)
        .collect();

    if heights.is_empty() {
        return 0.0;
    }

    heights.sort_by(f32::total_cmp);
    heights[heights.len() / 2]
}

/// A quad's extent, in pixels.
///
/// A quad is four arbitrary corners, because detection returns rotated boxes.
/// Every comparison here is against an axis-aligned box, so the corners are
/// reduced to their extent — right for near-horizontal text, and the only thing
/// that can be right for anything else.
fn extent(line: &OcrLine) -> (f32, f32, f32, f32) {
    let xs = line.bbox.points.map(|point| point[0]);
    let ys = line.bbox.points.map(|point| point[1]);

    (
        xs.iter().copied().fold(f32::INFINITY, f32::min),
        ys.iter().copied().fold(f32::INFINITY, f32::min),
        xs.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        ys.iter().copied().fold(f32::NEG_INFINITY, f32::max),
    )
}

fn top_of(line: &OcrLine) -> f32 {
    extent(line).1
}

fn bottom_of(line: &OcrLine) -> f32 {
    extent(line).3
}

fn left_of(line: &OcrLine) -> f32 {
    extent(line).0
}

fn right_of(line: &OcrLine) -> f32 {
    extent(line).2
}

fn centre_x(line: &OcrLine) -> f32 {
    let (left, _, right, _) = extent(line);
    (left + right) / 2.0
}

fn normalized(x0: f32, y0: f32, x1: f32, y1: f32, width: u32, height: u32) -> Rect {
    let (width, height) = (width.max(1) as f32, height.max(1) as f32);

    let left = (x0 / width).clamp(0.0, 1.0);
    let top = (y0 / height).clamp(0.0, 1.0);
    let right = (x1 / width).clamp(0.0, 1.0);
    let bottom = (y1 / height).clamp(0.0, 1.0);

    Rect::new(
        left,
        top,
        (right - left).clamp(0.0, 1.0 - left),
        (bottom - top).clamp(0.0, 1.0 - top),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rapidocr_core::types::Quad;

    const WIDTH: u32 = 1000;
    const HEIGHT: u32 = 2000;

    fn engine() -> EngineId {
        EngineId::from("rapidocr+layout")
    }

    fn line(text: &str, x0: f32, y0: f32, x1: f32, y1: f32) -> OcrLine {
        OcrLine {
            bbox: Quad::from_xyxy(x0, y0, x1, y1),
            text: text.to_owned(),
            score: 0.9,
        }
    }

    fn region(kind: RegionKind, x0: f32, y0: f32, x1: f32, y1: f32) -> Region {
        Region {
            kind,
            score: 0.95,
            x0,
            y0,
            x1,
            y1,
        }
    }

    fn blocks(lines: &[OcrLine], regions: &[Region]) -> Vec<Block> {
        // No table model: these tests are about the geometry under it, which is
        // also what a page gets on an installation that never downloaded one.
        blocks_from(lines, regions, None, WIDTH, HEIGHT, &engine())
    }

    fn kinds(found: &[Block]) -> Vec<BlockKind> {
        found.iter().map(|block| block.kind.clone()).collect()
    }

    fn text_of(block: &Block) -> String {
        block.text.clone()
    }

    /// Six numbers per region: the class, the score and the two corners.
    #[test]
    fn decoding_reads_the_class_the_score_and_the_box() {
        let raw = [7.0, 0.97, 10.0, 20.0, 30.0, 40.0];

        let found = decode(&raw);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, RegionKind::Formula);
        assert_eq!(found[0].score, 0.97);
        assert_eq!((found[0].x0, found[0].y0, found[0].x1, found[0].y1), (10.0, 20.0, 30.0, 40.0));
    }

    /// A guess about a heading lands in the document as a heading.
    #[test]
    fn decoding_drops_anything_the_model_is_not_sure_of() {
        let raw = [
            2.0, 0.49, 0.0, 0.0, 1.0, 1.0, //
            2.0, 0.51, 0.0, 0.0, 1.0, 1.0,
        ];

        assert_eq!(decode(&raw).len(), 1);
    }

    #[test]
    fn decoding_ignores_a_class_the_model_does_not_have() {
        let raw = [20.0, 0.99, 0.0, 0.0, 1.0, 1.0];

        assert!(decode(&raw).is_empty());
    }

    /// The model always returns its full set of slots, most of them empty.
    #[test]
    fn decoding_of_an_empty_page_finds_nothing() {
        assert!(decode(&[]).is_empty());
    }

    #[test]
    fn every_class_maps_to_a_kind() {
        for (index, kind) in CLASSES.iter().enumerate() {
            let raw = [index as f32, 0.9, 0.0, 0.0, 1.0, 1.0];

            assert_eq!(decode(&raw)[0].kind, *kind, "class {index}");
        }
    }

    #[test]
    fn a_document_title_is_a_top_level_heading() {
        assert_eq!(
            BlockKind::from(RegionKind::Title),
            BlockKind::Heading { level: 1 }
        );
        assert_eq!(
            BlockKind::from(RegionKind::Section),
            BlockKind::Heading { level: 2 }
        );
    }

    /// The measurement this whole module exists for: prose with a heading over
    /// it comes back as a heading and a paragraph, where the recogniser alone
    /// produced two paragraphs.
    #[test]
    fn a_heading_region_becomes_a_heading() {
        let lines = [
            line("Annual Report", 100.0, 100.0, 400.0, 140.0),
            line("Revenue increased by 12% this year.", 100.0, 300.0, 700.0, 330.0),
        ];
        let regions = [
            region(RegionKind::Title, 90.0, 90.0, 420.0, 150.0),
            region(RegionKind::Text, 90.0, 290.0, 720.0, 340.0),
        ];

        let found = blocks(&lines, &regions);

        assert_eq!(
            kinds(&found),
            vec![
                BlockKind::Heading { level: 1 },
                BlockKind::Paragraph
            ]
        );
        assert_eq!(found[0].text, "Annual Report");
    }

    /// The failure the corpus measured and this function exists to fix.
    ///
    /// Two columns read top-to-bottom-then-left-to-right come out interleaved:
    /// left one, right one, left two, right two. The characters are all there,
    /// which is why the content score looks fine and the reading-order score
    /// does not.
    #[test]
    fn a_two_column_page_is_read_a_column_at_a_time() {
        let lines = [
            line("left one", 100.0, 200.0, 400.0, 240.0),
            line("right one", 600.0, 200.0, 900.0, 240.0),
            line("left two", 100.0, 300.0, 400.0, 340.0),
            line("right two", 600.0, 300.0, 900.0, 340.0),
        ];
        let regions = [
            region(RegionKind::Text, 90.0, 190.0, 420.0, 350.0),
            region(RegionKind::Text, 590.0, 190.0, 910.0, 350.0),
        ];

        let found = blocks(&lines, &regions);
        let texts: Vec<&str> = found.iter().map(|block| block.text.as_str()).collect();

        assert_eq!(texts, vec!["left one\nleft two", "right one\nright two"]);
    }

    /// A title runs across both columns, so it comes first and the columns
    /// below it are still read separately.
    #[test]
    fn a_full_width_title_precedes_the_columns_under_it() {
        let lines = [
            line("A Title Across The Page", 100.0, 100.0, 900.0, 140.0),
            line("left one", 100.0, 300.0, 400.0, 340.0),
            line("right one", 600.0, 300.0, 900.0, 340.0),
            line("left two", 100.0, 400.0, 400.0, 440.0),
            line("right two", 600.0, 400.0, 900.0, 440.0),
        ];
        let regions = [
            region(RegionKind::Title, 90.0, 90.0, 910.0, 150.0),
            region(RegionKind::Text, 90.0, 290.0, 420.0, 450.0),
            region(RegionKind::Text, 590.0, 290.0, 910.0, 450.0),
        ];

        let found = blocks(&lines, &regions);
        let texts: Vec<&str> = found.iter().map(|block| block.text.as_str()).collect();

        assert_eq!(
            texts,
            vec![
                "A Title Across The Page",
                "left one\nleft two",
                "right one\nright two"
            ]
        );
    }

    /// Three columns, which is what a newspaper is and what the corpus scores
    /// worst after the exam papers.
    #[test]
    fn a_three_column_page_is_read_left_to_right_a_column_at_a_time() {
        let lines = [
            line("a1", 100.0, 300.0, 300.0, 340.0),
            line("b1", 500.0, 300.0, 700.0, 340.0),
            line("c1", 900.0, 300.0, 1100.0, 340.0),
            line("a2", 100.0, 400.0, 300.0, 440.0),
            line("b2", 500.0, 400.0, 700.0, 440.0),
            line("c2", 900.0, 400.0, 1100.0, 440.0),
        ];
        let regions = [
            region(RegionKind::Text, 90.0, 290.0, 320.0, 450.0),
            region(RegionKind::Text, 490.0, 290.0, 720.0, 450.0),
            region(RegionKind::Text, 890.0, 290.0, 1120.0, 450.0),
        ];

        let found = blocks(&lines, &regions);
        let texts: Vec<&str> = found.iter().map(|block| block.text.as_str()).collect();

        assert_eq!(texts, vec!["a1\na2", "b1\nb2", "c1\nc2"]);
    }

    /// The case that must not regress: one column is read exactly as it was,
    /// and a page this cannot help must not be a page it can hurt.
    #[test]
    fn a_one_column_page_is_read_in_the_same_order_as_before() {
        let lines = [
            line("second", 100.0, 400.0, 700.0, 440.0),
            line("first", 100.0, 300.0, 700.0, 340.0),
            line("third", 100.0, 500.0, 700.0, 540.0),
        ];
        let regions = [
            region(RegionKind::Text, 90.0, 390.0, 710.0, 450.0),
            region(RegionKind::Text, 90.0, 290.0, 710.0, 350.0),
            region(RegionKind::Text, 90.0, 490.0, 710.0, 550.0),
        ];

        let found = blocks(&lines, &regions);
        let texts: Vec<&str> = found.iter().map(|block| block.text.as_str()).collect();

        assert_eq!(texts, vec!["first", "second", "third"]);
    }

    /// A ragged right margin is not a column boundary.
    #[test]
    fn a_short_line_does_not_start_a_column_of_its_own() {
        let lines = [
            line("a full line of text here", 100.0, 300.0, 900.0, 340.0),
            line("short", 100.0, 400.0, 300.0, 440.0),
        ];
        let regions = [
            region(RegionKind::Text, 90.0, 290.0, 910.0, 350.0),
            region(RegionKind::Text, 90.0, 390.0, 310.0, 450.0),
        ];

        let found = blocks(&lines, &regions);
        let texts: Vec<&str> = found.iter().map(|block| block.text.as_str()).collect();

        assert_eq!(texts, vec!["a full line of text here", "short"]);
    }

    /// The point of the whole exercise: a table the geometry cannot lay out,
    /// laid out by the cells the model found.
    ///
    /// Two rows of three, every word in its own cell, and the box the model
    /// gave each cell is where the column would have been guessed at. Without
    /// the model these five lines are three columns of loose text.
    #[test]
    fn a_grid_from_the_model_puts_every_word_in_its_cell() {
        let lines = [
            line("Item", 110.0, 210.0, 180.0, 240.0),
            line("Qty", 510.0, 210.0, 560.0, 240.0),
            line("Price", 810.0, 210.0, 880.0, 240.0),
            line("Widget", 110.0, 310.0, 200.0, 340.0),
            line("3", 510.0, 310.0, 520.0, 340.0),
            line("12.00", 810.0, 310.0, 870.0, 340.0),
        ];
        let region = region(RegionKind::Table, 100.0, 200.0, 900.0, 350.0);

        let cell = |token: usize, across: usize| table::Cell {
            token,
            across,
            down: 1,
        };
        let grid = table::Grid {
            rows: vec![
                vec![cell(0, 1), cell(1, 1), cell(2, 1)],
                vec![cell(3, 1), cell(4, 1), cell(5, 1)],
            ],
            // The model answers in the crop's coordinates, and this crop starts
            // at (100, 200) of the page.
            boxes: vec![
                (10.0, 10.0, 90.0, 40.0),
                (410.0, 10.0, 460.0, 40.0),
                (710.0, 10.0, 790.0, 40.0),
                (10.0, 110.0, 100.0, 140.0),
                (410.0, 110.0, 420.0, 140.0),
                (710.0, 110.0, 770.0, 140.0),
            ],
        };

        let indices: Vec<usize> = (0..lines.len()).collect();
        let found = laid_out_grid(
            Sheet { grid: &grid, left: 100.0, top: 200.0 },
            &region,
            &indices,
            &lines.iter().collect::<Vec<_>>(),
            WIDTH,
            HEIGHT,
            &engine(),
        )
        .expect("a table");

        assert_eq!(found.kind, BlockKind::Table);
        assert_eq!(
            cells(&found),
            vec![
                vec!["Item", "Qty", "Price"],
                vec!["Widget", "3", "12.00"],
            ]
        );
    }

    /// A cell covering two columns keeps the cells after it under their own
    /// headings, even though Markdown cannot say the two are one.
    #[test]
    fn a_spanned_cell_does_not_shift_the_cells_after_it() {
        let lines = [
            line("A crossed heading", 110.0, 210.0, 500.0, 240.0),
            line("beside it", 610.0, 210.0, 800.0, 240.0),
            line("left", 110.0, 310.0, 300.0, 340.0),
            line("middle", 400.0, 310.0, 500.0, 340.0),
            line("right", 610.0, 310.0, 800.0, 340.0),
        ];
        let region = region(RegionKind::Table, 100.0, 200.0, 900.0, 350.0);

        let cell = |token: usize, across: usize| table::Cell {
            token,
            across,
            down: 1,
        };
        let grid = table::Grid {
            rows: vec![
                vec![cell(0, 2), cell(1, 1)],
                vec![cell(2, 1), cell(3, 1), cell(4, 1)],
            ],
            boxes: vec![
                (10.0, 10.0, 490.0, 40.0),
                (510.0, 10.0, 790.0, 40.0),
                (10.0, 110.0, 200.0, 140.0),
                (300.0, 110.0, 400.0, 140.0),
                (510.0, 110.0, 790.0, 140.0),
            ],
        };

        let indices: Vec<usize> = (0..lines.len()).collect();
        let found = laid_out_grid(
            Sheet { grid: &grid, left: 100.0, top: 200.0 },
            &region,
            &indices,
            &lines.iter().collect::<Vec<_>>(),
            WIDTH,
            HEIGHT,
            &engine(),
        )
        .expect("a table");

        assert_eq!(
            cells(&found),
            vec![
                vec!["A crossed heading", "", "beside it"],
                vec!["left", "middle", "right"],
            ]
        );
    }

    /// A cell three rows tall holds its column in the two rows under it, so
    /// those rows start their own cells in the right columns.
    ///
    /// Without this they start them at the left, and every word after the merge
    /// lands one cell over: measured on an equipment register with one merged
    /// label, the label went into the header and the rows under it lost their
    /// item and their count.
    #[test]
    fn a_row_a_tall_cell_reaches_into_keeps_its_columns() {
        let tall = |token: usize, down: usize| table::Cell {
            token,
            across: 1,
            down,
        };

        let rows = vec![
            vec![tall(0, 3), tall(1, 1), tall(2, 1)],
            vec![tall(3, 1), tall(4, 1)],
            vec![tall(5, 1), tall(6, 1)],
            vec![tall(7, 1), tall(8, 1), tall(9, 1)],
        ];

        let seats = seatings(&rows);

        // Three wide, with the first position held by the cell above.
        assert_eq!(seats[0].len(), 3);
        assert_eq!(seats[0][0], Some((0, 1)));
        assert_eq!(seats[1].len(), 3, "the row put its cells after the held one");
        assert_eq!(seats[1][0], None, "the first column is still the same cell");
        assert_eq!(seats[1][1], Some((0, 1)), "and its first cell is the next one over");
        assert_eq!(seats[2][0], None);

        // Out of its reach, the row starts its own cells again.
        assert_eq!(seats[3].len(), 3);
        assert_eq!(seats[3][0], Some((0, 1)));
    }

    /// The row under a tall cell can be shorter than the columns that cell
    /// holds, and the positions still have to be there or the table is ragged.
    #[test]
    fn a_row_that_ends_early_still_has_the_held_columns() {
        let rows = vec![
            vec![table::Cell { token: 0, across: 1, down: 1 }, table::Cell { token: 1, across: 2, down: 2 }],
            vec![table::Cell { token: 2, across: 1, down: 1 }],
        ];

        let seats = seatings(&rows);

        assert_eq!(seats[1].len(), 3, "{seats:?}");
        assert_eq!(seats[1][1], None);
        assert_eq!(seats[1][2], None);
    }

    /// A grid with nothing in it leaves the geometry its answer rather than
    /// producing a table with no cells in it.
    #[test]
    fn an_empty_grid_is_not_a_table() {
        let region = region(RegionKind::Table, 100.0, 200.0, 900.0, 350.0);
        let grid = table::Grid {
            rows: vec![vec![table::Cell { token: 0, across: 1, down: 1 }]],
            boxes: vec![(10.0, 10.0, 90.0, 40.0)],
        };

        assert!(
            laid_out_grid(
                Sheet { grid: &grid, left: 100.0, top: 200.0 },
                &region,
                &[],
                &[],
                WIDTH,
                HEIGHT,
                &engine()
            )
            .is_none()
        );
    }

    /// A real two-column page from the corpus, with the boxes the detector
    /// actually returned for it. Kept because the synthetic tests all passed
    /// while this page was still being read as one column.
    #[test]
    fn a_real_two_column_page_splits_into_two() {
        let boxes: Vec<(f32, f32, f32, f32)> = vec![
            (683.8, 213.0, 1221.9, 1736.0),
            (126.1, 211.2, 657.4, 716.3),
            (120.2, 1650.1, 657.5, 1788.0),
            (127.1, 1025.7, 654.1, 1444.1),
            (121.5, 747.4, 658.0, 880.3),
            (120.1, 1473.5, 658.0, 1583.1),
            (685.4, 1734.8, 1221.6, 1788.4),
            (529.0, 163.2, 859.4, 181.9),
            (125.1, 160.2, 168.4, 181.2),
            (471.6, 1541.0, 573.3, 1561.5),
            (260.6, 836.8, 360.1, 857.6),
            (790.2, 1181.5, 920.5, 1207.7),
            (528.9, 814.2, 625.2, 835.3),
            (689.2, 1155.0, 741.6, 1180.3),
        ];

        let columns = page_columns(&boxes, 1370.0, 2017.0);

        assert_eq!(columns.len(), 2, "the page has two columns: {columns:?}");
    }

    #[test]
    fn blocks_come_back_top_to_bottom() {
        let lines = [
            line("second", 100.0, 400.0, 400.0, 430.0),
            line("first", 100.0, 100.0, 400.0, 130.0),
        ];
        let regions = [
            region(RegionKind::Text, 90.0, 390.0, 420.0, 440.0),
            region(RegionKind::Text, 90.0, 90.0, 420.0, 140.0),
        ];

        let found = blocks(&lines, &regions);

        assert_eq!(text_of(&found[0]), "first");
        assert_eq!(text_of(&found[1]), "second");
    }

    /// Two regions side by side are one column each, and the left is read
    /// first. Reading order across more than one column is not solved here.
    #[test]
    fn regions_on_the_same_row_are_read_left_to_right() {
        let lines = [
            line("right", 600.0, 100.0, 900.0, 130.0),
            line("left", 100.0, 100.0, 400.0, 130.0),
        ];
        let regions = [
            region(RegionKind::Text, 590.0, 90.0, 920.0, 140.0),
            region(RegionKind::Text, 90.0, 90.0, 420.0, 140.0),
        ];

        let found = blocks(&lines, &regions);

        assert_eq!(text_of(&found[0]), "left");
        assert_eq!(text_of(&found[1]), "right");
    }

    /// The recogniser reports lines; a region is what says they were one
    /// paragraph, and it says it without the gap heuristic the layout-free path
    /// has to use.
    #[test]
    fn the_lines_of_one_region_are_one_block() {
        let lines = [
            line("a sentence that", 100.0, 100.0, 500.0, 130.0),
            line("wraps onto a second line", 100.0, 131.0, 520.0, 161.0),
        ];
        let regions = [region(RegionKind::Text, 90.0, 90.0, 540.0, 170.0)];

        let found = blocks(&lines, &regions);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "a sentence that\nwraps onto a second line");
    }

    #[test]
    fn a_block_is_only_as_confident_as_its_least_certain_line() {
        let mut lines = [
            line("sure", 100.0, 100.0, 400.0, 130.0),
            line("unsure", 100.0, 131.0, 400.0, 161.0),
        ];
        lines[0].score = 0.99;
        lines[1].score = 0.61;
        let regions = [region(RegionKind::Text, 90.0, 90.0, 420.0, 170.0)];

        assert_eq!(blocks(&lines, &regions)[0].confidence, Some(0.61));
    }

    /// A figure has no words, and is still worth carrying: its box is where the
    /// picture is.
    #[test]
    fn an_empty_figure_keeps_its_box() {
        let regions = [region(RegionKind::Figure, 100.0, 100.0, 900.0, 800.0)];

        let found = blocks(&[], &regions);

        assert_eq!(kinds(&found), vec![BlockKind::Figure]);
        assert_eq!(found[0].confidence, Some(0.95));

        let bbox = found[0].bbox.expect("bbox");
        assert!((bbox.x - 0.1).abs() < 1e-6, "{bbox:?}");
        assert!((bbox.h - 0.35).abs() < 1e-6, "{bbox:?}");
    }

    /// A heading with no words in it renders as a bare `#`.
    #[test]
    fn an_empty_region_of_any_other_kind_is_nothing() {
        let regions = [
            region(RegionKind::Title, 100.0, 100.0, 900.0, 200.0),
            region(RegionKind::Table, 100.0, 300.0, 900.0, 800.0),
        ];

        assert!(blocks(&[], &regions).is_empty());
    }

    /// Not a reason to lose the words.
    #[test]
    fn a_word_no_region_claimed_is_still_a_paragraph() {
        let lines = [line("stray", 100.0, 100.0, 400.0, 130.0)];

        let found = blocks(&lines, &[]);

        assert_eq!(kinds(&found), vec![BlockKind::Paragraph]);
        assert_eq!(found[0].text, "stray");
    }

    #[test]
    fn a_loose_word_keeps_its_place_in_the_order() {
        let lines = [
            line("first", 100.0, 100.0, 400.0, 130.0),
            line("stray", 100.0, 500.0, 400.0, 530.0),
            line("last", 100.0, 900.0, 400.0, 930.0),
        ];
        let regions = [
            region(RegionKind::Text, 90.0, 90.0, 420.0, 140.0),
            region(RegionKind::Text, 90.0, 890.0, 420.0, 940.0),
        ];

        let found = blocks(&lines, &regions);

        let texts: Vec<&str> = found.iter().map(|block| block.text.as_str()).collect();
        assert_eq!(texts, vec!["first", "stray", "last"]);
    }

    /// A line that straddles two regions belongs to the one holding its middle.
    #[test]
    fn a_line_between_two_regions_goes_to_the_one_holding_its_middle() {
        let lines = [line("overlap", 100.0, 140.0, 400.0, 170.0)];
        let regions = [
            region(RegionKind::Text, 90.0, 90.0, 420.0, 150.0),
            region(RegionKind::Text, 90.0, 160.0, 420.0, 220.0),
        ];

        let found = blocks(&lines, &regions);

        assert_eq!(found.len(), 1);
        // The second region's top, and the first's, decide which comes first;
        // the point here is only that the line was claimed once.
        assert_eq!(found[0].text, "overlap");
    }

    // The table is the thing the measurement turned on: 0 of 9 tables found
    // before, 7 of 7 pages with tables found after. These are the tests for
    // turning a found region into a table somebody can read.

    fn table_lines() -> Vec<OcrLine> {
        vec![
            line("Item", 100.0, 260.0, 200.0, 290.0),
            line("Qty", 500.0, 260.0, 560.0, 290.0),
            line("Price", 800.0, 260.0, 900.0, 290.0),
            line("Widget", 100.0, 320.0, 220.0, 350.0),
            line("3", 500.0, 320.0, 520.0, 350.0),
            line("12.00", 800.0, 320.0, 880.0, 350.0),
            line("Gadget", 100.0, 380.0, 230.0, 410.0),
            line("7", 500.0, 380.0, 520.0, 410.0),
            line("4.50", 800.0, 380.0, 880.0, 410.0),
        ]
    }

    fn table_region() -> Region {
        region(RegionKind::Table, 90.0, 190.0, 920.0, 420.0)
    }

    fn cells(found: &Block) -> Vec<Vec<String>> {
        found
            .children
            .iter()
            .map(|row| {
                row.children
                    .iter()
                    .map(|cell| cell.text.clone())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_table_region_becomes_rows_and_cells() {
        let found = blocks(&table_lines(), &[table_region()]);

        assert_eq!(kinds(&found), vec![BlockKind::Table]);
        assert_eq!(
            cells(&found[0]),
            vec![
                vec!["Item", "Qty", "Price"],
                vec!["Widget", "3", "12.00"],
                vec!["Gadget", "7", "4.50"],
            ]
        );
    }

    /// The failure this guards against is silent: a `Table` with no rows
    /// renders as nothing at all, so a table that is not really a table loses
    /// its words from the document.
    #[test]
    fn a_table_region_renders_as_a_markdown_table() {
        use docfusion_core::ir::{DocMeta, Document, Page, PageProvenance, Size, SourceRef};

        let found = blocks(&table_lines(), &[table_region()]);

        let document = Document {
            source: SourceRef {
                path: std::path::PathBuf::from("scan.png"),
                content_hash: "0".repeat(64),
                mime: "image/png".to_owned(),
                size: 0,
                mtime: None,
            },
            pages: vec![Page {
                index: 0,
                size: Some(Size {
                    w: WIDTH as f32,
                    h: HEIGHT as f32,
                }),
                blocks: found,
                provenance: PageProvenance {
                    primary: engine(),
                    engines: vec![engine()],
                    verified: false,
                },
            }],
            meta: DocMeta::default(),
            conflicts: Vec::new(),
        };

        let markdown = docfusion_core::render::markdown(&document, "2026-01-01T00:00:00Z");

        assert!(markdown.contains("| Item | Qty | Price |"), "{markdown}");
        assert!(markdown.contains("| Widget | 3 | 12.00 |"), "{markdown}");
        assert!(markdown.contains("| Gadget | 7 | 4.50 |"), "{markdown}");
    }

    /// A header row spanning the table crosses every column, so counting it as
    /// evidence of where the columns are would weld them into one and lose the
    /// table entirely.
    #[test]
    fn a_spanning_header_does_not_merge_the_columns() {
        let mut lines = table_lines();
        lines.push(line("Quarterly breakdown", 100.0, 200.0, 900.0, 230.0));

        let found = blocks(&lines, &[table_region()]);

        assert_eq!(kinds(&found), vec![BlockKind::Table]);
        assert_eq!(cells(&found[0]).len(), 4, "the header is a row of its own");
        assert!(
            cells(&found[0])[0].iter().any(|cell| cell == "Quarterly breakdown"),
            "{:?}",
            cells(&found[0])
        );
    }

    /// A cell whose own text wraps over two lines is one cell, not two.
    #[test]
    fn a_wrapped_cell_is_one_cell() {
        let lines = [
            line("Widgets and", 100.0, 200.0, 220.0, 230.0),
            line("gadgets", 100.0, 231.0, 180.0, 261.0),
            line("12.00", 800.0, 200.0, 880.0, 230.0),
        ];
        let regions = [region(RegionKind::Table, 90.0, 190.0, 920.0, 270.0)];

        let found = blocks(&lines, &regions);

        assert_eq!(cells(&found[0]), vec![vec!["Widgets and gadgets", "12.00"]]);
    }

    /// Rows of different heights still land on the same grid.
    #[test]
    fn rows_are_the_lines_at_the_same_height() {
        let lines = [
            line("a", 100.0, 200.0, 150.0, 220.0),
            line("b", 500.0, 200.0, 550.0, 220.0),
            line("tall", 100.0, 300.0, 200.0, 360.0),
            line("c", 500.0, 310.0, 550.0, 330.0),
        ];
        let regions = [region(RegionKind::Table, 90.0, 190.0, 920.0, 400.0)];

        let found = blocks(&lines, &regions);

        assert_eq!(
            cells(&found[0]),
            vec![vec!["a", "b"], vec!["tall", "c"]]
        );
    }

    /// A list is not a table, and calling it one would put a Markdown table in
    /// the document that says nothing a sentence would not have said better.
    #[test]
    fn one_column_is_not_a_table() {
        let lines = [
            line("first", 100.0, 200.0, 400.0, 230.0),
            line("second", 100.0, 260.0, 400.0, 290.0),
        ];
        let regions = [region(RegionKind::Table, 90.0, 190.0, 920.0, 320.0)];

        let found = blocks(&lines, &regions);

        assert_eq!(kinds(&found), vec![BlockKind::Paragraph]);
        assert_eq!(found[0].text, "first\nsecond");
    }

    #[test]
    fn one_line_is_not_a_table() {
        let lines = [line("alone", 100.0, 200.0, 400.0, 230.0)];
        let regions = [region(RegionKind::Table, 90.0, 190.0, 920.0, 320.0)];

        assert_eq!(kinds(&blocks(&lines, &regions)), vec![BlockKind::Paragraph]);
    }

    #[test]
    fn a_table_cell_carries_where_it_was() {
        let found = blocks(&table_lines(), &[table_region()]);

        let cell = &found[0].children[0].children[0];
        let bbox = cell.bbox.expect("bbox");

        assert!((bbox.x - 0.1).abs() < 1e-6, "{bbox:?}");
        assert!((bbox.w - 0.1).abs() < 1e-6, "{bbox:?}");
    }

    /// The table's own text is what everything downstream of the renderer
    /// reads, and a table with none would be a hole in the index.
    #[test]
    fn a_table_carries_its_words_as_text_too() {
        let found = blocks(&table_lines(), &[table_region()]);

        assert!(found[0].text.contains("Widget"), "{}", found[0].text);
        assert!(found[0].text.contains("12.00"), "{}", found[0].text);
    }

    #[test]
    fn a_page_with_nothing_on_it_produces_nothing() {
        assert!(blocks(&[], &[]).is_empty());
    }

    #[test]
    fn every_block_is_attributed_to_this_engine() {
        let lines = table_lines();
        let found = blocks(&lines, &[table_region()]);

        assert!(found.iter().all(|block| block.engine.as_str() == "rapidocr+layout"));
        assert!(
            found[0]
                .children
                .iter()
                .all(|row| row.engine.as_str() == "rapidocr+layout")
        );
    }

    /// Reads a real page through the real weights.
    ///
    /// Ignored by default: it needs the 124 MB model downloaded and takes a
    /// second a page. What it checks is the half the tests above cannot — that
    /// the session loads, that the three inputs are the right three, and that
    /// the boxes come back in the coordinates of the page rather than of the
    /// square the model was fed.
    /// Times the detector through this crate's own code path.
    ///
    /// A measurement rather than a test: it asserts nothing and its number
    /// depends on the machine. It exists because "the provider is configured"
    /// and "the provider is running the model" are different claims, and only a
    /// clock tells them apart — with the `directml` feature this prints 77 ms
    /// against 451 without it, on an RTX 3060.
    #[test]
    #[ignore = "a measurement, not a test"]
    fn times_the_detector() {
        assert!(
            available(),
            "no model at {}; run `docfusion doctor --fetch` first",
            model_path().display()
        );

        let page = image::RgbImage::from_pixel(1700, 2200, image::Rgb([255, 255, 255]));
        let detector = Detector::new(model_path());

        // Once to load, then measured, because the first run pays for a session
        // build that no later page ever pays again.
        let loaded = std::time::Instant::now();
        detector.detect(&page).expect("detect");
        eprintln!("first run, session and all: {:?}", loaded.elapsed());

        let started = std::time::Instant::now();
        let runs = 10;
        for _ in 0..runs {
            detector.detect(&page).expect("detect");
        }
        eprintln!(
            "{} ms a page over {runs} runs",
            started.elapsed().as_millis() as f64 / runs as f64
        );
    }

    #[test]
    #[ignore = "needs the structure model downloaded"]
    fn reads_structure_from_a_rendered_page() {
        use docfusion_core::extract::PageSource;
        use docfusion_core::extract::fixtures;

        // Asked for by name, so the model had better be here: a test that
        // quietly does nothing when it is missing is a test that passes by
        // having run on nothing.
        assert!(
            available(),
            "no model at {}; run `docfusion doctor --fetch` first",
            model_path().display()
        );

        // A page with a shape to it rather than a line on a blank sheet: the
        // detector is looking for the layout of a document, and a single word
        // alone on a page is not one — measured, it finds nothing on that at
        // all, and neither does the reference implementation.
        let pdf = fixtures::pdf(&[
            (22.0, "Annual Report"),
            (11.0, "Revenue increased by twelve percent this year, and the"),
            (11.0, "growth came from every region we operate in. Costs were"),
            (11.0, "flat, so the margin widened slightly."),
        ]);
        let pages = docfusion_core::extract::pdf::RenderedPages::open(&pdf, 200.0).expect("open");
        let page = pages.page(0).expect("render");

        let rgb = image::RgbImage::from_raw(
            page.width,
            page.height,
            page.pixels
                .chunks_exact(4)
                .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
                .collect(),
        )
        .expect("rgb");

        let detector = Detector::new(model_path());
        let regions = detector.detect(&rgb).expect("detect");

        assert!(
            !regions.is_empty(),
            "the model found nothing on a page with words on it"
        );
        assert!(
            regions.iter().all(|region| {
                region.x0 >= 0.0
                    && region.y0 >= 0.0
                    && region.x1 <= page.width as f32
                    && region.y1 <= page.height as f32
            }),
            "a box is not in the coordinates of the page: {regions:?}"
        );

        // And not merely on the page. The boxes come out of the model in the
        // coordinates of the square the page was squashed into, and reading
        // them as page coordinates puts everything it finds in the middle of a
        // page that is not square — which is still *inside* the page, and
        // passed the check above for as long as that check was the only one.
        //
        // The numbers are what makes this a check rather than a formality. On
        // this fixture the title's ink is a tenth of the way down the page and
        // ends a third of the way across it. Read as page coordinates, the very
        // same detection comes back reaching three quarters of the way across
        // and a quarter of the way down — found, named correctly, and pointed
        // at the wrong part of the page.
        let (width, height) = (page.width as f32, page.height as f32);
        let title = regions
            .iter()
            .find(|region| matches!(region.kind, RegionKind::Title | RegionKind::Section))
            .expect("a title");

        assert!(
            title.y1 < height * 0.15,
            "the title is not at the top of the page: {title:?}"
        );
        assert!(
            title.x1 < width * 0.5,
            "the title reaches too far across the page: {title:?}"
        );
    }
}
