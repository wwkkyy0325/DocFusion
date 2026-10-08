//! Extraction engines and the router that chooses between them.
//!
//! An engine declares how well it fits an input, and the router picks the best
//! candidate. An engine that inspects the input and then declines — the usual
//! case for a PDF with no text layer — hands control to the next candidate
//! rather than failing the run.

pub mod docx;
pub mod ooxml;
pub mod pdf;
pub mod text;

use crate::ir::{Document, EngineId, EngineStamp, SourceRef};
use crate::mime::{Container, sniff};

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

    #[error(transparent)]
    Io(#[from] std::io::Error),
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
        self.engines
            .iter()
            .any(|engine| engine.accepts(probe) != Fit::No)
    }

    pub fn extract(
        &self,
        probe: &InputProbe,
        source: &SourceRef,
        bytes: &[u8],
    ) -> Result<Document, ExtractError> {
        let mut candidates: Vec<&dyn Extractor> = self
            .engines
            .iter()
            .map(|engine| engine.as_ref())
            .filter(|engine| engine.accepts(probe) != Fit::No)
            .collect();
        candidates.sort_by_key(|engine| engine.accepts(probe));

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
        container if container.is_image() => {
            "no OCR or vision backend is configured yet, so images cannot be read".to_owned()
        }
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
