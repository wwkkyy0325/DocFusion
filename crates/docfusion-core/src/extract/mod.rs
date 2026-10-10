//! Extraction engines and the router that chooses between them.
//!
//! An engine declares how well it fits an input, and the router picks the best
//! candidate. An engine that inspects the input and then declines — the usual
//! case for a PDF with no text layer — hands control to the next candidate
//! rather than failing the run.

/// Documents built in memory, for tests.
///
/// Gated so that only tests pay for it, but reachable from other crates'
/// tests: a fixture duplicated into a second crate is a fixture that drifts.
#[cfg(any(test, feature = "test-support"))]
pub mod fixtures;

pub mod docx;
pub mod ooxml;
pub mod pdf;
pub mod pptx;
pub mod scan;
pub mod text;
pub mod xlsx;

use std::sync::Arc;

use crate::image::PageImage;
use crate::ir::{Document, EngineId, EngineStamp, SourceRef};
use crate::mime::{Container, sniff};

/// The pages of an input, produced when they are asked for.
///
/// On demand rather than all at once, because a rendered page is raw RGBA8 —
/// fifteen megabytes for an A4 at 200 DPI. Returning every page of a PDF in one
/// vector needs fifteen gigabytes for a thousand-page scan, before a single
/// page has been read. This hands out one page at a time, in an `Arc` so that
/// whoever is working on it keeps it alive and it is released the moment they
/// are done.
///
/// The order is the document's, and an index means the same page every time.
pub trait PageSource: Send + Sync {
    fn len(&self) -> usize;

    /// Whether there is nothing to read.
    ///
    /// Worth saying explicitly, because a page source with no pages is the
    /// difference between a document that failed and one that was empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn page(&self, index: usize) -> Result<Arc<PageImage>, ExtractError>;
}

/// One line of text and where it sits on the page it came from.
///
/// The two paths that read a page into lines disagree about almost everything —
/// one recognises the words, the other is handed them by the file — and agree
/// about this. Everything downstream of the lines cares only about the words and
/// the boxes, so this is the shape they are passed in.
///
/// In the page image's own pixels, not the page's units: a table's cells are
/// boxes on the rendered page, and comparing a box in points against a box in
/// pixels is a mistake that looks like a bad model.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLine {
    pub text: String,
    /// Left, top, right, bottom.
    pub bbox: (f32, f32, f32, f32),
}

/// A table found on a rendered page.
///
/// Rows of cells, each with the words that belong in it and where it is. The
/// words are put in the cells by whoever found the table, because that is the
/// same question for both paths and the answer should not be written twice.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundTable {
    /// Left, top, right, bottom, in the rendered page's pixels.
    pub bbox: (f32, f32, f32, f32),
    pub rows: Vec<Vec<FoundCell>>,
}

impl FoundTable {
    /// How many columns the widest row has.
    pub fn columns(&self) -> usize {
        self.rows.iter().map(Vec::len).max().unwrap_or(0)
    }
}

/// One cell of a found table.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundCell {
    pub text: String,
    /// Left, top, right, bottom, in the rendered page's pixels. `None` when the
    /// model gave no box for the cell, which is not the same as a box at zero.
    pub bbox: Option<(f32, f32, f32, f32)>,
}

/// Finds the tables on a rendered page and says what is in them.
///
/// Implemented where the models are, because finding a table is two ONNX
/// sessions and reading one needs no session at all. Called by the extractor
/// that has the words already: a PDF whose pages carry a text layer has exact
/// text and exact boxes, and the only thing missing is which box is which cell.
/// Running a recogniser over it would be inventing a worse copy of what the file
/// already says.
pub trait TableReader: Send + Sync {
    /// Its name, for the front matter. Part of the cache key, because a page
    /// read with the table model on it is not the page read without.
    fn id(&self) -> &'static str;

    fn tables(
        &self,
        page: &PageImage,
        lines: &[TextLine],
    ) -> Result<Vec<FoundTable>, ExtractError>;
}

/// An input that is already a single page.
pub struct OnePage(Arc<PageImage>);

impl OnePage {
    pub fn new(image: PageImage) -> Self {
        Self(Arc::new(image))
    }
}

impl PageSource for OnePage {
    fn len(&self) -> usize {
        1
    }

    fn is_empty(&self) -> bool {
        false
    }

    fn page(&self, index: usize) -> Result<Arc<PageImage>, ExtractError> {
        if index != 0 {
            return Err(ExtractError::unsupported(format!(
                "this input is one page, and page {} was asked for",
                index + 1
            )));
        }

        Ok(Arc::clone(&self.0))
    }
}

/// How well an engine claims to fit an input.
///
/// Ordered from best to worst so candidates can be sorted directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fit {
    /// The container is recognised and this engine will produce a document.
    Yes,
    /// The container matches, but only opening the input reveals whether this
    /// engine applies. Sixteen magic bytes cannot say whether a PDF carries a
    /// text layer.
    Maybe,
    /// This engine cannot handle the input.
    No,
}

/// What is known about an input before committing to an engine.
#[derive(Debug, Clone)]
pub struct InputProbe {
    pub container: Container,
    pub size: u64,
    /// A hint only. Never the deciding factor — that is the whole point of
    /// sniffing the content.
    pub file_name: Option<String>,
}

impl InputProbe {
    pub fn new(head: &[u8], file_name: Option<String>, size: u64) -> Self {
        Self {
            container: sniff(head),
            file_name,
            size,
        }
    }
}

/// An input handed to an engine.
///
/// Borrowed throughout: a document may be hundreds of megabytes and engines
/// must not be handed an owned copy on spec.
pub struct Input<'a> {
    pub probe: &'a InputProbe,
    pub bytes: &'a [u8],
    /// The pipeline's record of where these bytes came from. Engines copy it
    /// into the document they return, refining `mime` when they can determine
    /// something more precise than magic-number sniffing could.
    pub source: &'a SourceRef,
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    /// The engine inspected the input and declined.
    ///
    /// This is control flow as much as an error: routing moves on to the next
    /// candidate. It is only reported to the caller when no engine is left.
    #[error("{reason}")]
    Unsupported { reason: String },

    #[error("the {engine} engine failed: {message}")]
    Failed { engine: EngineId, message: String },

    /// The engine never got to try: something it needs is not on this machine.
    ///
    /// Apart from [`ExtractError::Failed`] because the answer is different. A
    /// failed engine is a page the tool could not read, which is a thing to
    /// look into; an engine whose weights have not been downloaded is a button
    /// somebody has not pressed, and telling them the engine failed sends them
    /// looking for a bug instead. The distinction is carried as a type rather
    /// than as a sentence, because a window that matched on the wording of an
    /// error would be broken by the next edit to it.
    #[error("the {engine} engine cannot run: {message}")]
    Unavailable { engine: EngineId, message: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A page could not be turned into pixels, so nothing could read it.
    #[error(transparent)]
    Image(#[from] crate::image::ImageError),
}

impl ExtractError {
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self::Unsupported {
            reason: reason.into(),
        }
    }

    pub fn failed(engine: impl Into<EngineId>, message: impl Into<String>) -> Self {
        Self::Failed {
            engine: engine.into(),
            message: message.into(),
        }
    }

    /// The same for a backend that could not be started at all.
    pub fn unavailable(engine: impl Into<EngineId>, message: impl Into<String>) -> Self {
        Self::Unavailable {
            engine: engine.into(),
            message: message.into(),
        }
    }
}

/// A structural parser for one family of formats.
pub trait Extractor: Send + Sync {
    fn id(&self) -> EngineId;

    fn version(&self) -> &str;

    /// The cache identity of this engine.
    ///
    /// Deterministic parsers have no model or prompt, so the default suffices;
    /// model-backed engines override this to include both.
    fn stamp(&self) -> EngineStamp {
        EngineStamp::new(self.id(), self.version())
    }

    fn accepts(&self, probe: &InputProbe) -> Fit;

    fn extract(&self, input: &Input<'_>) -> Result<Document, ExtractError>;
}

/// Chooses an engine for an input and runs it.
pub struct Router {
    engines: Vec<Box<dyn Extractor>>,
}

impl Router {
    pub fn new(engines: Vec<Box<dyn Extractor>>) -> Self {
        Self { engines }
    }

    /// Whether any engine is willing to attempt this input.
    pub fn can_handle(&self, probe: &InputProbe) -> bool {
        !self.candidates(probe).is_empty()
    }

    /// The engines that might handle this input, best fit first.
    ///
    /// Exposed because a cache lookup has to try each candidate's engine stamp
    /// in the same order extraction would: whether an engine applies is only
    /// settled by opening the input, so a caller cannot know which stamp to
    /// look up without asking.
    pub fn candidates(&self, probe: &InputProbe) -> Vec<&dyn Extractor> {
        let mut candidates: Vec<&dyn Extractor> = self
            .engines
            .iter()
            .map(|engine| engine.as_ref())
            .filter(|engine| engine.accepts(probe) != Fit::No)
            .collect();
        candidates.sort_by_key(|engine| engine.accepts(probe));
        candidates
    }

    /// The cache identity of the engine with this id, if the router has it.
    ///
    /// A document records which engine produced it; this turns that back into
    /// the stamp the manifest is keyed on.
    pub fn stamp_of(&self, id: &EngineId) -> Option<EngineStamp> {
        self.engines
            .iter()
            .find(|engine| engine.id() == *id)
            .map(|engine| engine.stamp())
    }

    pub fn extract(
        &self,
        probe: &InputProbe,
        source: &SourceRef,
        bytes: &[u8],
    ) -> Result<Document, ExtractError> {
        let candidates = self.candidates(probe);

        let input = Input {
            probe,
            bytes,
            source,
        };
        let mut declines = Vec::new();

        for engine in candidates {
            match engine.extract(&input) {
                Ok(document) => return Ok(document),
                Err(ExtractError::Unsupported { reason }) => {
                    declines.push(format!("{}: {reason}", engine.id()));
                }
                Err(failure) => return Err(failure),
            }
        }

        Err(ExtractError::unsupported(no_engine_reason(
            probe, &declines,
        )))
    }
}

/// Explains why nothing could read an input.
///
/// The response to a legacy Office file or an image is very different from the
/// response to an unrecognised blob, so the message is tailored rather than
/// generic — a user should not have to guess what to do next.
fn no_engine_reason(probe: &InputProbe, declines: &[String]) -> String {
    let mut message = match probe.container {
        Container::Ole2 => "this is a legacy Office file (.doc, .xls or .ppt); \
             save it as .docx, .xlsx or .pptx, or export it to PDF, and try again"
            .to_owned(),
        Container::Pdf => "this PDF has no text layer, and no OCR or vision backend \
             is configured to read it; see the setup guide"
            .to_owned(),
        container if container.is_image() => "no OCR or vision backend is configured, \
             so images cannot be read; see the setup guide"
            .to_owned(),
        Container::Zip => {
            "this archive is not a docx, xlsx or pptx, so nothing can read it".to_owned()
        }
        Container::Unknown => "unrecognised format".to_owned(),
        _ => "no available engine can handle this input".to_owned(),
    };

    if !declines.is_empty() {
        message.push_str(&format!(" ({})", declines.join("; ")));
    }

    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{DocMeta, Page, PageProvenance, Size};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn source() -> SourceRef {
        SourceRef {
            path: PathBuf::from("input.bin"),
            content_hash: "0".repeat(64),
            mime: "application/octet-stream".into(),
            size: 0,
            mtime: None,
        }
    }

    fn document(engine: &EngineId) -> Document {
        Document {
            source: source(),
            pages: vec![Page {
                index: 0,
                size: Some(Size { w: 1.0, h: 1.0 }),
                blocks: Vec::new(),
                provenance: PageProvenance {
                    primary: engine.clone(),
                    engines: vec![engine.clone()],
                    verified: false,
                },
            }],
            meta: DocMeta::default(),
            conflicts: Vec::new(),
        }
    }

    /// Records how often `extract` was reached, so a test can tell whether a
    /// declined engine was actually tried.
    struct Stub {
        id: &'static str,
        fit: Fit,
        outcome: Outcome,
        calls: AtomicU32,
    }

    enum Outcome {
        Succeed,
        Decline,
        Fail,
    }

    impl Stub {
        fn new(id: &'static str, fit: Fit, outcome: Outcome) -> Self {
            Self {
                id,
                fit,
                outcome,
                calls: AtomicU32::new(0),
            }
        }
    }

    impl Extractor for Stub {
        fn id(&self) -> EngineId {
            EngineId::from(self.id)
        }

        fn version(&self) -> &str {
            "1.0.0"
        }

        fn accepts(&self, _probe: &InputProbe) -> Fit {
            self.fit
        }

        fn extract(&self, _input: &Input<'_>) -> Result<Document, ExtractError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            match self.outcome {
                Outcome::Succeed => Ok(document(&self.id())),
                Outcome::Decline => Err(ExtractError::unsupported("not my format")),
                Outcome::Fail => Err(ExtractError::failed(self.id, "exploded")),
            }
        }
    }

    fn probe(container: Container) -> InputProbe {
        InputProbe {
            container,
            size: 10,
            file_name: None,
        }
    }

    #[test]
    fn engines_that_decline_are_never_tried() {
        let router = Router::new(vec![
            Box::new(Stub::new("declines", Fit::No, Outcome::Succeed)),
            Box::new(Stub::new("handles", Fit::Yes, Outcome::Succeed)),
        ]);

        let document = router.extract(&probe(Container::Pdf), &source(), b"").unwrap();
        assert_eq!(document.pages[0].provenance.primary.as_str(), "handles");
    }

    /// A `Maybe` engine that declines must hand over to the next candidate —
    /// this is how a scanned PDF falls through to an OCR or vision backend.
    #[test]
    fn a_declining_engine_falls_through_to_the_next() {
        let router = Router::new(vec![
            Box::new(Stub::new("maybe", Fit::Maybe, Outcome::Decline)),
            Box::new(Stub::new("fallback", Fit::Maybe, Outcome::Succeed)),
        ]);

        let document = router.extract(&probe(Container::Pdf), &source(), b"").unwrap();
        assert_eq!(document.pages[0].provenance.primary.as_str(), "fallback");
    }

    /// Equally plausible engines keep their registration order, which is the
    /// mechanism behind a rule that matters: a PDF's text layer is tried before
    /// its pages are rendered, because parsing is free and exact while
    /// rendering costs an image and a model.
    #[test]
    fn equally_plausible_engines_keep_their_registration_order() {
        let router = Router::new(vec![
            Box::new(Stub::new("text-layer", Fit::Maybe, Outcome::Succeed)),
            Box::new(Stub::new("render", Fit::Maybe, Outcome::Succeed)),
        ]);

        let document = router.extract(&probe(Container::Pdf), &source(), b"").unwrap();

        assert_eq!(document.pages[0].provenance.primary.as_str(), "text-layer");
    }

    /// Candidates are sorted, so a `Maybe` is never tried before a `Yes`.
    #[test]
    fn certain_engines_are_tried_before_speculative_ones() {
        let router = Router::new(vec![
            Box::new(Stub::new("maybe", Fit::Maybe, Outcome::Succeed)),
            Box::new(Stub::new("certain", Fit::Yes, Outcome::Succeed)),
        ]);

        let document = router.extract(&probe(Container::Text), &source(), b"").unwrap();
        assert_eq!(document.pages[0].provenance.primary.as_str(), "certain");
    }

    /// A genuine failure must stop routing: falling through as if nothing
    /// happened would hide a broken engine behind a different one's output.
    #[test]
    fn a_hard_failure_stops_routing() {
        let router = Router::new(vec![
            Box::new(Stub::new("broken", Fit::Yes, Outcome::Fail)),
            Box::new(Stub::new("intact", Fit::Yes, Outcome::Succeed)),
        ]);

        let error = router
            .extract(&probe(Container::Pdf), &source(), b"")
            .unwrap_err();
        assert!(matches!(error, ExtractError::Failed { .. }), "got {error}");
    }

    #[test]
    fn every_declining_engine_produces_one_explanatory_error() {
        let router = Router::new(vec![
            Box::new(Stub::new("one", Fit::Yes, Outcome::Decline)),
            Box::new(Stub::new("two", Fit::Yes, Outcome::Decline)),
        ]);

        let error = router
            .extract(&probe(Container::Pdf), &source(), b"")
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("one"), "{message}");
        assert!(message.contains("two"), "{message}");
    }

    #[test]
    fn legacy_office_files_get_an_actionable_message() {
        let router = Router::new(vec![]);
        let error = router
            .extract(&probe(Container::Ole2), &source(), b"")
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("docx"), "{message}");
    }

    #[test]
    fn images_get_a_message_about_missing_backends() {
        let router = Router::new(vec![]);
        let error = router
            .extract(&probe(Container::Jpeg), &source(), b"")
            .unwrap_err();
        assert!(error.to_string().contains("OCR"), "{error}");
    }

    #[test]
    fn can_handle_reflects_engine_fits() {
        let router = Router::new(vec![Box::new(Stub::new("pdf-only", Fit::No, Outcome::Succeed))]);
        assert!(!router.can_handle(&probe(Container::Pdf)));
    }
}
