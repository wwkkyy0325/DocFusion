//! Engines that read a page by looking at it.
//!
//! Two traits rather than one, because the two roles are the foundation of this
//! project's value proposition and the type system should not let them be
//! confused: a recognition engine is local, promptless and cheap enough to run
//! on every page, and it is what *verifies*. A vision model is prompted,
//! possibly remote and paid, and it is what *is verified*.
//!
//! Neither trait knows how the other works, which is what makes the role matrix
//! in the design expressible: whichever of the two did not produce the primary
//! result is the one that checks it.

use crate::image::PageImage;
use crate::ir::{Block, EngineStamp};

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// The engine is not configured, not installed, or not reachable.
    ///
    /// Separate from a failure because the response differs: an unavailable
    /// backend needs setting up, a failing one needs looking at.
    #[error("{0}")]
    Unavailable(String),

    #[error("{0}")]
    Failed(String),
}

/// What a backend can report beyond the text itself.
///
/// Verification is built on these. Without bounding boxes there is nothing to
/// align two engines' output on, and without confidence there is nothing to
/// threshold — either way the check can only degrade to comparing whole
/// documents, which OCR noise makes useless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Caps {
    pub bounding_boxes: bool,
    pub confidence: bool,
}

/// How a page image reaches a backend.
///
/// Borrowed, because a rendered page is megabytes and a backend has no reason
/// to take ownership of it.
pub struct PageRequest<'a> {
    pub image: &'a PageImage,
    /// Zero-based, for engines whose output depends on which page they are
    /// looking at, and for reporting which page failed.
    pub page: u32,
}

/// A recognition engine: no prompt, no model of the world, just reading.
pub trait OcrBackend: Send + Sync {
    fn id(&self) -> crate::ir::EngineId;

    fn version(&self) -> &str;

    fn capabilities(&self) -> Caps;

    fn stamp(&self) -> EngineStamp {
        EngineStamp::new(self.id(), self.version())
    }

    fn recognize(&self, request: &PageRequest<'_>) -> Result<Vec<Block>, BackendError>;
}

/// A generative vision model, prompted to produce structured Markdown.
pub trait VisionBackend: Send + Sync {
    fn id(&self) -> crate::ir::EngineId;

    fn version(&self) -> &str;

    /// Identifies the prompt as well as the model.
    ///
    /// A prompt is part of the engine's identity: editing one changes the
    /// output while the model stays the same, so it has to reach the cache key
    /// rather than being a private detail.
    fn prompt_version(&self) -> &str;

    fn capabilities(&self) -> Caps;

    fn stamp(&self) -> EngineStamp {
        EngineStamp::new(self.id(), self.version()).with_prompt_version(self.prompt_version())
    }

    fn parse(&self, request: &PageRequest<'_>) -> Result<Vec<Block>, BackendError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::EngineId;

    struct StubOcr;

    impl OcrBackend for StubOcr {
        fn id(&self) -> EngineId {
            EngineId::from("stub-ocr")
        }

        fn version(&self) -> &str {
            "1.0.0"
        }

        fn capabilities(&self) -> Caps {
            Caps {
                bounding_boxes: true,
                confidence: true,
            }
        }

        fn recognize(&self, _request: &PageRequest<'_>) -> Result<Vec<Block>, BackendError> {
            Ok(Vec::new())
        }
    }

    struct StubVision;

    impl VisionBackend for StubVision {
        fn id(&self) -> EngineId {
            EngineId::from("stub-vision")
        }

        fn version(&self) -> &str {
            "2.0.0"
        }

        fn prompt_version(&self) -> &str {
            "prompt-v1"
        }

        fn capabilities(&self) -> Caps {
            Caps {
                bounding_boxes: false,
                confidence: false,
            }
        }

        fn parse(&self, _request: &PageRequest<'_>) -> Result<Vec<Block>, BackendError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn an_ocr_engine_stamps_its_id_and_version() {
        let stamp = StubOcr.stamp();

        assert_eq!(stamp.id.as_str(), "stub-ocr");
        assert_eq!(stamp.version, "1.0.0");
        assert_eq!(stamp.model, None);
    }

    /// A prompt is part of what produced a result, so it has to reach the cache
    /// key. Leaving it out means editing a prompt silently serves old output.
    #[test]
    fn a_vision_engine_stamps_its_prompt_version() {
        let stamp = StubVision.stamp();

        assert_eq!(stamp.id.as_str(), "stub-vision");
        assert_eq!(stamp.prompt_version.as_deref(), Some("prompt-v1"));
    }

    #[test]
    fn capability_defaults_claim_nothing() {
        let caps = Caps::default();

        assert!(!caps.bounding_boxes);
        assert!(!caps.confidence);
    }
}
