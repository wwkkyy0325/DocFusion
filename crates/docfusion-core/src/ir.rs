//! Core intermediate representation.
//!
//! Every extractor produces a [`Document`], and every renderer and verifier
//! consumes one. The IR is deliberately block-structured rather than a plain
//! string: cross-engine verification needs to align regions, compare them, and
//! record disagreements, none of which is possible with flat text.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Identifies an extraction engine.
///
/// Engines are pluggable, so this is an open string rather than an enum.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EngineId(String);

impl EngineId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EngineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for EngineId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for EngineId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// The full identity of an engine configuration.
///
/// Together with the content hash this forms the cache key. Every component
/// must participate: re-running an unchanged model with an edited prompt
/// produces different output, so `prompt_version` cannot be omitted without
/// silently serving stale results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineStamp {
    pub id: EngineId,
    pub version: String,
    pub model: Option<String>,
    pub prompt_version: Option<String>,
}

impl EngineStamp {
    pub fn new(id: impl Into<EngineId>, version: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            version: version.into(),
            model: None,
            prompt_version: None,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_prompt_version(mut self, prompt_version: impl Into<String>) -> Self {
        self.prompt_version = Some(prompt_version.into());
        self
    }
}

/// A rectangle in normalized page coordinates.
///
/// Both axes run from `0.0` to `1.0`, with the origin at the top-left. Engines
/// see different resolutions (a PDF library works in points, a vision model in
/// pixels), so normalizing is what makes cross-engine alignment possible.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// Creates a rectangle. Coordinates must already be normalized.
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        debug_assert!(
            (0.0..=1.0).contains(&x)
                && (0.0..=1.0).contains(&y)
                && x + w <= 1.0 + f32::EPSILON
                && y + h <= 1.0 + f32::EPSILON,
            "rect must be normalized to 0..=1, got {x},{y},{w},{h}"
        );
        Self { x, y, w, h }
    }

    pub fn area(&self) -> f32 {
        self.w * self.h
    }

    /// Intersection over union, in `0.0..=1.0`.
    ///
    /// Returns `0.0` for disjoint and for degenerate (zero-area) rectangles
    /// rather than producing a NaN, so callers can compare thresholds safely.
    pub fn iou(&self, other: &Rect) -> f32 {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = (self.x + self.w).min(other.x + other.w);
        let y2 = (self.y + self.h).min(other.y + other.h);

        if x2 <= x1 || y2 <= y1 {
            return 0.0;
        }

        let intersection = (x2 - x1) * (y2 - y1);
        let union = self.area() + other.area() - intersection;

        if union <= 0.0 {
            0.0
        } else {
            intersection / union
        }
    }
}

/// Page dimensions in the engine's native unit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

/// Where a document came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    pub path: PathBuf,
    /// BLAKE3 of the file's bytes, as lowercase hex.
    pub content_hash: String,
    /// Content-sniffed MIME type, never derived from the file extension.
    pub mime: String,
    pub size: u64,
    /// Seconds since the Unix epoch, when the filesystem reports it.
    pub mtime: Option<i64>,
}

/// Document-level metadata, mostly populated from the source file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DocMeta {
    pub title: Option<String>,
    pub author: Option<String>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BlockKind {
    Heading { level: u8 },
    Paragraph,
    ListItem,
    /// A table. Its children are [`BlockKind::TableRow`] blocks.
    Table,
    /// A table row. Its children are [`BlockKind::TableCell`] blocks.
    ///
    /// Rows are explicit rather than flattened into a cell list because a
    /// renderer cannot lay out a grid without knowing where rows end.
    TableRow,
    /// A table cell. Its `text` holds the cell's content.
    TableCell,
    Figure,
    Formula,
    Code,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
    pub bbox: Option<Rect>,
    /// Engine-reported confidence, `0.0..=1.0`, when the engine provides one.
    pub confidence: Option<f32>,
    pub engine: EngineId,
    /// Nested content, such as the cells of a table or the items of a list.
    pub children: Vec<Block>,
}

impl Block {
    /// Creates a leaf block with no geometry or confidence.
    pub fn text(kind: BlockKind, text: impl Into<String>, engine: EngineId) -> Self {
        Self {
            kind,
            text: text.into(),
            bbox: None,
            confidence: None,
            engine,
            children: Vec::new(),
        }
    }
}

/// Which engines contributed to a page, and whether it was verified.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageProvenance {
    pub primary: EngineId,
    /// Every engine that contributed, including `primary`.
    pub engines: Vec<EngineId>,
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub index: u32,
    /// Page geometry, for formats that have a notion of pages.
    ///
    /// `None` for formats that do not — plain text has no page size, and
    /// inventing one would be a lie the renderer and verifier would have to
    /// work around.
    pub size: Option<Size>,
    pub blocks: Vec<Block>,
    pub provenance: PageProvenance,
}

/// The class of disagreement between engines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConflictKind {
    /// Both engines found a block in the same place, but read different text.
    TextMismatch,
    /// One engine found a block the other did not, or they disagree on where
    /// it is. Usually a reading-order error or a dropped region — the most
    /// dangerous class, because the text that *is* present looks fine.
    StructuralMismatch,
    /// A region present in one engine's output is absent from the other's.
    Missing,
}

/// One engine's version of a disputed region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub engine: EngineId,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResolutionSource {
    User,
    Arbiter,
    Heuristic,
}

/// The outcome of adjudicating a conflict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    pub chosen: EngineId,
    pub text: String,
    pub by: ResolutionSource,
}

/// A disagreement between engines.
///
/// Conflicts live in the IR rather than in a log file so that they travel with
/// the artifact. Specialized parsers fail loudly and general models fail
/// silently; recording disagreements is what makes the silent failures loud.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conflict {
    pub page: u32,
    pub bbox: Option<Rect>,
    pub kind: ConflictKind,
    pub variants: Vec<Variant>,
    pub resolved: Option<Resolution>,
}

/// A fully extracted document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub source: SourceRef,
    pub pages: Vec<Page>,
    pub meta: DocMeta,
    pub conflicts: Vec<Conflict>,
}

impl Document {
    /// Every block on every page, in reading order, depth-first.
    pub fn blocks(&self) -> impl Iterator<Item = &Block> {
        self.pages.iter().flat_map(|page| flatten(&page.blocks))
    }
}

fn flatten(blocks: &[Block]) -> Box<dyn Iterator<Item = &Block> + '_> {
    Box::new(
        blocks
            .iter()
            .flat_map(|block| std::iter::once(block).chain(flatten(&block.children))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-6;

    fn assert_approx(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < EPSILON,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn identical_rects_overlap_completely() {
        let a = Rect::new(0.0, 0.0, 1.0, 1.0);
        assert_approx(a.iou(&a), 1.0);
    }

    #[test]
    fn disjoint_rects_have_zero_iou() {
        let a = Rect::new(0.0, 0.0, 0.4, 0.4);
        let b = Rect::new(0.6, 0.6, 0.4, 0.4);
        assert_approx(a.iou(&b), 0.0);
    }

    #[test]
    fn edge_touching_rects_have_zero_iou() {
        // Adjacency is not overlap: the shared edge has no area.
        let a = Rect::new(0.0, 0.0, 0.5, 1.0);
        let b = Rect::new(0.5, 0.0, 0.5, 1.0);
        assert_approx(a.iou(&b), 0.0);
    }

    #[test]
    fn contained_rect_yields_area_ratio() {
        // Intersection 0.25, union 1.0 (the larger rectangle).
        let outer = Rect::new(0.0, 0.0, 1.0, 1.0);
        let inner = Rect::new(0.25, 0.25, 0.5, 0.5);
        assert_approx(outer.iou(&inner), 0.25);
        assert_approx(inner.iou(&outer), 0.25);
    }

    #[test]
    fn partial_overlap_matches_hand_computation() {
        // Intersection 0.0625; union 0.25 + 0.25 - 0.0625 = 0.4375.
        let a = Rect::new(0.0, 0.0, 0.5, 0.5);
        let b = Rect::new(0.25, 0.25, 0.5, 0.5);
        assert_approx(a.iou(&b), 0.0625 / 0.4375);
    }

    #[test]
    fn degenerate_rects_do_not_produce_nan() {
        let empty = Rect::new(0.0, 0.0, 0.0, 0.0);
        let normal = Rect::new(0.0, 0.0, 1.0, 1.0);
        assert_approx(empty.iou(&empty), 0.0);
        assert_approx(empty.iou(&normal), 0.0);
        assert_approx(normal.iou(&empty), 0.0);
    }

    #[test]
    fn iou_is_symmetric() {
        let a = Rect::new(0.1, 0.2, 0.5, 0.3);
        let b = Rect::new(0.3, 0.1, 0.4, 0.6);
        assert_approx(a.iou(&b), b.iou(&a));
    }

    #[test]
    fn document_iterates_nested_blocks_depth_first() {
        let engine = EngineId::from("test");
        let cell = Block::text(BlockKind::Paragraph, "cell", engine.clone());
        let mut table = Block::text(BlockKind::Table, "", engine.clone());
        table.children.push(cell);
        let para = Block::text(BlockKind::Paragraph, "body", engine.clone());

        let doc = Document {
            source: SourceRef {
                path: PathBuf::from("x.pdf"),
                content_hash: "0".repeat(64),
                mime: "application/pdf".into(),
                size: 1,
                mtime: None,
            },
            pages: vec![Page {
                index: 0,
                size: Some(Size { w: 612.0, h: 792.0 }),
                blocks: vec![table, para],
                provenance: PageProvenance {
                    primary: engine.clone(),
                    engines: vec![engine],
                    verified: false,
                },
            }],
            meta: DocMeta::default(),
            conflicts: Vec::new(),
        };

        let texts: Vec<&str> = doc.blocks().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, vec!["", "cell", "body"]);
    }

    #[test]
    fn document_survives_a_json_round_trip() {
        let pdfium = EngineId::from("pdfium");
        let glm = EngineId::from("glm-ocr");

        let doc = Document {
            source: SourceRef {
                path: PathBuf::from("report.pdf"),
                content_hash: "a".repeat(64),
                mime: "application/pdf".into(),
                size: 4096,
                mtime: Some(1_700_000_000),
            },
            pages: vec![Page {
                index: 0,
                size: Some(Size { w: 612.0, h: 792.0 }),
                blocks: vec![Block {
                    kind: BlockKind::Heading { level: 1 },
                    text: "Annual Report".into(),
                    bbox: Some(Rect::new(0.1, 0.05, 0.8, 0.06)),
                    confidence: Some(0.98),
                    engine: pdfium.clone(),
                    children: Vec::new(),
                }],
                provenance: PageProvenance {
                    primary: pdfium.clone(),
                    engines: vec![pdfium.clone(), glm.clone()],
                    verified: true,
                },
            }],
            meta: DocMeta {
                title: Some("Annual Report".into()),
                author: None,
                language: Some("en".into()),
            },
            conflicts: vec![Conflict {
                page: 0,
                bbox: Some(Rect::new(0.1, 0.05, 0.8, 0.06)),
                kind: ConflictKind::TextMismatch,
                variants: vec![
                    Variant {
                        engine: pdfium,
                        text: "Annual Report".into(),
                    },
                    Variant {
                        engine: glm,
                        text: "Annual Reports".into(),
                    },
                ],
                resolved: Some(Resolution {
                    chosen: EngineId::from("pdfium"),
                    text: "Annual Report".into(),
                    by: ResolutionSource::User,
                }),
            }],
        };

        let json = serde_json::to_string(&doc).expect("serialize");
        let parsed: Document = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(doc, parsed);
    }
}
