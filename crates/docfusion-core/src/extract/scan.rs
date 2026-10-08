//! Reading pages by looking at them.
//!
//! The extractor for inputs that have no text to parse: standalone images, and
//! PDFs whose pages are scans. It renders or decodes each page and hands it to
//! the configured backends.

use crate::backend::{OcrBackend, PageRequest, VisionBackend};
use crate::image::PageImage;
use crate::ir::{
    Block, Conflict, DocMeta, Document, EngineId, EngineStamp, Page, PageProvenance, Size,
};
use crate::mime::Container;
use crate::verify::{self, VerifyPolicy};

use super::pdf;
use super::{ExtractError, Extractor, Fit, Input, InputProbe};

const ENGINE_ID: &str = "scan";
const ENGINE_VERSION: &str = "1.0.0";

/// The resolution pages are rendered at before a backend reads them.
///
/// Recognition depends on how many pixels cover a character, and 72 DPI — one
/// pixel per point — leaves most of them too small to read. Higher is better
/// for accuracy and worse for everything else: the image has to be encoded,
/// sent, and processed by the backend, and a page at 300 DPI is more than twice
/// the pixels of one at 200.
const DEFAULT_DPI: f32 = 200.0;

pub struct ScanExtractor {
    ocr: Option<Box<dyn OcrBackend>>,
    vision: Option<Box<dyn VisionBackend>>,
    policy: VerifyPolicy,
    dpi: f32,
}

impl ScanExtractor {
    pub fn new(ocr: Option<Box<dyn OcrBackend>>, vision: Option<Box<dyn VisionBackend>>) -> Self {
        Self {
            ocr,
            vision,
            policy: VerifyPolicy::default(),
            dpi: DEFAULT_DPI,
        }
    }

    pub fn with_policy(mut self, policy: VerifyPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn with_dpi(mut self, dpi: f32) -> Self {
        self.dpi = dpi;
        self
    }

    pub fn is_configured(&self) -> bool {
        self.ocr.is_some() || self.vision.is_some()
    }

    fn page_images(&self, input: &Input<'_>) -> Result<Vec<PageImage>, ExtractError> {
        if input.probe.container.is_image() {
            return Ok(vec![PageImage::decode(input.bytes)?]);
        }

        pdf::render_pages(input.bytes, self.dpi)
    }

    /// Reads one page, and checks it with the other engine when the policy
    /// asks for it.
    ///
    /// There is deliberately no fallback to OCR when the vision model fails. A
    /// silent downgrade produces unverified output that is indistinguishable
    /// from verified output, which is the exact failure this project is
    /// organised around not making.
    ///
    /// `budget` is the number of pages still allowed to be checked. It is spent
    /// per page rather than per document so that a scan of a thousand pages
    /// stops at the ceiling instead of running to the end of the batch.
    fn read(&self, request: &PageRequest<'_>, budget: &mut u32) -> Result<Reading, ExtractError> {
        let blocks = self.primary(request)?;

        // The check is the expensive half — another recognition pass, or
        // another billed request — so the ceiling is spent here rather than on
        // reading, and a page that is not checked says so rather than looking
        // like one that passed.
        let unchecked = Reading {
            blocks,
            conflicts: Vec::new(),
            verified: false,
        };

        let Some(verifier) = self.verifier() else {
            return Ok(unchecked);
        };

        if *budget == 0 || !self.policy.should_verify(&unchecked.blocks) {
            return Ok(unchecked);
        }
        *budget -= 1;

        let checked = verifier.recognize(request).map_err(failed)?;

        Ok(Reading {
            conflicts: verify::compare(request.page, &unchecked.blocks, &checked),
            verified: true,
            ..unchecked
        })
    }

    /// Reads one page with whichever backend is primary.
    ///
    /// The vision model is primary whenever one is configured, and the
    /// recognition engine is what checks it — the opposite of the intuition
    /// that a specialist should outrank a generalist. The reason is that the
    /// verifier has to be the one that can run on every page: local, free and
    /// fast. See the role matrix in the design.
    fn primary(&self, request: &PageRequest<'_>) -> Result<Vec<Block>, ExtractError> {
        if let Some(vision) = &self.vision {
            return vision.parse(request).map_err(failed);
        }

        let ocr = self
            .ocr
            .as_ref()
            .ok_or_else(|| ExtractError::failed(ENGINE_ID, "no backend is configured"))?;
        ocr.recognize(request).map_err(failed)
    }

    /// The engine that checks the primary's work, when there is a different one
    /// to check it with.
    ///
    /// Only the recognition engine verifies. The role matrix gives the checking
    /// job to the engine that can run on every page, and it must be a different
    /// lineage from the primary: a model asked to check itself has correlated
    /// errors and cannot find its own mistakes. So when no vision engine is
    /// configured the recognition engine *is* the primary, and nothing checks
    /// it — which the page records rather than hides.
    fn verifier(&self) -> Option<&dyn OcrBackend> {
        self.vision.as_ref()?;
        self.ocr.as_deref()
    }
}

/// Turns a backend failure into this engine's failure.
///
/// The engine is named rather than left to a generic one, because the message
/// a person sees should say which part of the pipeline broke.
fn failed(error: crate::backend::BackendError) -> ExtractError {
    ExtractError::failed(ENGINE_ID, error.to_string())
}

/// One page's reading, and what is known about how far it can be trusted.
struct Reading {
    blocks: Vec<Block>,
    conflicts: Vec<Conflict>,
    verified: bool,
}

impl Extractor for ScanExtractor {
    fn id(&self) -> EngineId {
        EngineId::from(ENGINE_ID)
    }

    fn version(&self) -> &str {
        ENGINE_VERSION
    }

    /// The cache identity of the extractor *and* of the backends behind it.
    ///
    /// The extractor's own name never changes, so keying on it alone would
    /// serve stale output the moment somebody pointed the tool at a different
    /// model. Every backend that can contribute is folded in, including a
    /// vision model's prompt version, because editing a prompt changes the
    /// output while everything else stays the same.
    fn stamp(&self) -> EngineStamp {
        EngineStamp::new(self.id(), self.version()).with_model(self.backend_identity())
    }

    fn accepts(&self, probe: &InputProbe) -> Fit {
        if !self.is_configured() {
            // Refusing here is what lets the router explain that an image needs
            // a backend rather than reporting that nothing matched.
            return Fit::No;
        }

        match probe.container {
            container if container.is_image() => Fit::Yes,
            // A PDF needs rendering only when it has no text layer, and that
            // cannot be known from its first bytes. Ordering decides: the
            // text-layer engine is registered first, so a PDF that can be
            // parsed is parsed.
            Container::Pdf => Fit::Maybe,
            _ => Fit::No,
        }
    }

    fn extract(&self, input: &Input<'_>) -> Result<Document, ExtractError> {
        let images = self.page_images(input)?;
        if images.is_empty() {
            return Err(ExtractError::unsupported("the document has no pages"));
        }

        let mut budget = self.policy.max_verify_pages;
        let mut pages = Vec::with_capacity(images.len());
        let mut conflicts: Vec<Conflict> = Vec::new();

        for (index, image) in images.iter().enumerate() {
            let reading = self.read(
                &PageRequest {
                    image,
                    page: index as u32,
                },
                &mut budget,
            )?;

            conflicts.extend(reading.conflicts);
            pages.push(Page {
                index: index as u32,
                // Pixels, not points: this engine's native unit is the rendered
                // image, and a backend reports geometry in it.
                size: Some(Size {
                    w: image.width as f32,
                    h: image.height as f32,
                }),
                blocks: reading.blocks,
                provenance: PageProvenance {
                    primary: self.id(),
                    engines: self.backend_ids(),
                    verified: reading.verified,
                },
            });
        }

        Ok(Document {
            source: input.source.clone(),
            pages,
            meta: DocMeta::default(),
            conflicts,
        })
    }
}

impl ScanExtractor {
    fn backend_identity(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        if let Some(vision) = &self.vision {
            parts.push(format!(
                "vision:{}@{}#{}",
                vision.id(),
                vision.version(),
                vision.prompt_version()
            ));
        }
        if let Some(ocr) = &self.ocr {
            parts.push(format!("ocr:{}@{}", ocr.id(), ocr.version()));
        }

        // The policy decides what the output says — which pages carry conflicts
        // and which are marked verified — so leaving it out of the key would
        // serve a cached single-engine result to a run that asked for a second
        // opinion.
        parts.push(self.policy.describe());

        parts.join("+")
    }

    fn backend_ids(&self) -> Vec<EngineId> {
        let mut ids = Vec::new();
        if let Some(vision) = &self.vision {
            ids.push(vision.id());
        }
        if let Some(ocr) = &self.ocr {
            ids.push(ocr.id());
        }
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{BackendError, Caps};
    use crate::extract::fixtures;
    use crate::ir::{BlockKind, ConflictKind, SourceRef};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn block(text: &str, engine: &str) -> Block {
        Block::text(BlockKind::Paragraph, text, EngineId::from(engine))
    }

    /// Counts calls, so a test can prove that a backend was *not* run. Not
    /// running the second engine is the point of a policy that limits when it
    /// runs, and asserting on the output alone would not show it.
    #[derive(Clone, Default)]
    struct Calls(Arc<AtomicUsize>);

    impl Calls {
        fn count(&self) -> usize {
            self.0.load(Ordering::Relaxed)
        }

        fn record(&self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    struct StubOcr {
        text: &'static str,
        calls: Calls,
    }

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
            self.calls.record();
            Ok(vec![block(self.text, "stub-ocr")])
        }
    }

    struct StubVision {
        text: &'static str,
        unavailable: bool,
        calls: Calls,
    }

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
            Caps::default()
        }

        fn parse(&self, _request: &PageRequest<'_>) -> Result<Vec<Block>, BackendError> {
            self.calls.record();
            if self.unavailable {
                return Err(BackendError::Unavailable(
                    "the endpoint is not reachable".into(),
                ));
            }
            Ok(vec![block(self.text, "stub-vision")])
        }
    }

    fn ocr() -> Box<dyn OcrBackend> {
        ocr_saying("read by ocr").0
    }

    /// A recognition engine, and a handle on whether it was asked to run.
    fn ocr_saying(text: &'static str) -> (Box<dyn OcrBackend>, Calls) {
        let calls = Calls::default();
        let backend: Box<dyn OcrBackend> = Box::new(StubOcr {
            text,
            calls: calls.clone(),
        });
        (backend, calls)
    }

    fn vision(unavailable: bool) -> Box<dyn VisionBackend> {
        vision_saying("read by vision", unavailable).0
    }

    fn vision_saying(text: &'static str, unavailable: bool) -> (Box<dyn VisionBackend>, Calls) {
        let calls = Calls::default();
        let backend: Box<dyn VisionBackend> = Box::new(StubVision {
            text,
            unavailable,
            calls: calls.clone(),
        });
        (backend, calls)
    }

    fn source(size: usize) -> SourceRef {
        SourceRef {
            path: PathBuf::from("fixture"),
            content_hash: "0".repeat(64),
            mime: "application/octet-stream".into(),
            size: size as u64,
            mtime: None,
        }
    }

    fn extract(
        extractor: &ScanExtractor,
        bytes: &[u8],
        name: &str,
    ) -> Result<Document, ExtractError> {
        let probe = InputProbe::new(bytes, Some(name.to_owned()), bytes.len() as u64);
        extractor.extract(&Input {
            probe: &probe,
            bytes,
            source: &source(bytes.len()),
        })
    }

    fn image() -> Vec<u8> {
        fixtures::png(20, 10)
    }

    fn probe(bytes: &[u8], name: &str) -> InputProbe {
        InputProbe::new(bytes, Some(name.to_owned()), bytes.len() as u64)
    }

    #[test]
    fn an_unconfigured_extractor_refuses_everything() {
        let extractor = ScanExtractor::new(None, None);

        assert_eq!(extractor.accepts(&probe(&image(), "scan.png")), Fit::No);
        assert_eq!(extractor.accepts(&probe(&fixtures::pdf(&[]), "scan.pdf")), Fit::No);
        assert!(!extractor.is_configured());
    }

    #[test]
    fn a_configured_extractor_takes_images_outright_and_pdfs_speculatively() {
        let extractor = ScanExtractor::new(None, Some(vision(false)));

        assert_eq!(extractor.accepts(&probe(&image(), "scan.png")), Fit::Yes);
        assert_eq!(extractor.accepts(&probe(&fixtures::pdf(&[]), "scan.pdf")), Fit::Maybe);
        assert_eq!(extractor.accepts(&probe(b"hello", "notes.txt")), Fit::No);
    }

    #[test]
    fn reads_an_image_file() {
        let extractor = ScanExtractor::new(None, Some(vision(false)));

        let document = extract(&extractor, &image(), "scan.png").expect("extract");

        let texts: Vec<&str> = document.blocks().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, vec!["read by vision"]);
    }

    #[test]
    fn page_size_is_the_rendered_pixel_size() {
        let extractor = ScanExtractor::new(None, Some(vision(false)));

        let document = extract(&extractor, &image(), "scan.png").expect("extract");
        let size = document.pages[0].size.expect("size");

        assert_eq!((size.w, size.h), (20.0, 10.0));
    }

    /// The vision model is primary and OCR is the verifier, not the other way
    /// round: the verifier has to be the one that can run on every page.
    #[test]
    fn vision_is_primary_when_both_are_configured() {
        let extractor = ScanExtractor::new(Some(ocr()), Some(vision(false)));

        let document = extract(&extractor, &image(), "scan.png").expect("extract");

        let texts: Vec<&str> = document.blocks().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, vec!["read by vision"]);
    }

    #[test]
    fn the_ocr_backend_is_recorded_as_available_on_the_page() {
        let extractor = ScanExtractor::new(Some(ocr()), Some(vision(false)));

        let document = extract(&extractor, &image(), "scan.png").expect("extract");
        let engines: Vec<String> = document.pages[0]
            .provenance
            .engines
            .iter()
            .map(ToString::to_string)
            .collect();

        assert_eq!(engines, vec!["stub-vision", "stub-ocr"]);
    }

    /// A silent downgrade would produce unverified output that is
    /// indistinguishable from verified output, which is the failure this
    /// project is organised around not making.
    #[test]
    fn a_failing_vision_backend_does_not_quietly_fall_back_to_ocr() {
        let extractor = ScanExtractor::new(Some(ocr()), Some(vision(true)));

        let error = extract(&extractor, &image(), "scan.png").expect_err("should fail");

        assert!(
            error.to_string().contains("not reachable"),
            "the backend's own message should survive: {error}"
        );
    }

    /// Rendering is the only way a scan is read at all, so the two halves have
    /// to be tested together rather than separately.
    #[test]
    fn reads_a_scanned_pdf_by_rendering_its_pages() {
        let extractor = ScanExtractor::new(None, Some(vision(false)));
        let scanned = fixtures::pdf(&[]);

        let document = extract(&extractor, &scanned, "scan.pdf").expect("extract");

        assert_eq!(document.pages.len(), 1);
        let size = document.pages[0].size.expect("size");
        assert!(size.w > 0.0 && size.h > 0.0, "the page should have been rendered");
        assert_eq!(
            document.blocks().next().unwrap().text,
            "read by vision"
        );
    }

    #[test]
    fn the_engine_stamp_covers_every_backend() {
        let extractor = ScanExtractor::new(Some(ocr()), Some(vision(false)));

        let model = extractor.stamp().model.expect("a composite identity");

        assert!(model.contains("stub-vision@2.0.0#prompt-v1"), "{model}");
        assert!(model.contains("stub-ocr@1.0.0"), "{model}");
    }

    /// Pointing the tool at a different model has to invalidate previous
    /// output, and the extractor's own name never changes.
    #[test]
    fn changing_a_backend_changes_the_stamp() {
        let with_vision = ScanExtractor::new(None, Some(vision(false))).stamp();
        let with_ocr = ScanExtractor::new(Some(ocr()), None).stamp();

        assert_ne!(with_vision.model, with_ocr.model);
    }

    #[test]
    fn a_page_records_which_extractor_produced_it() {
        let extractor = ScanExtractor::new(None, Some(vision(false)));

        let document = extract(&extractor, &image(), "scan.png").expect("extract");

        assert_eq!(document.pages[0].provenance.primary.as_str(), "scan");
        assert!(!document.pages[0].provenance.verified);
    }

    // ── Verification ─────────────────────────────────────────────────────────

    fn disagreeing(policy: VerifyPolicy) -> (ScanExtractor, Calls) {
        let (recogniser, calls) = ocr_saying("Revenue increased by 12%");
        let (reader, _) = vision_saying("Revenue increased by 13%", false);

        (
            ScanExtractor::new(Some(recogniser), Some(reader)).with_policy(policy),
            calls,
        )
    }

    /// The point of the whole step: two engines that read the page differently
    /// produce a record of the disagreement rather than one of them quietly
    /// winning.
    #[test]
    fn a_disagreement_is_recorded_in_the_document() {
        let (extractor, _) = disagreeing(VerifyPolicy::always());

        let document = extract(&extractor, &image(), "scan.png").expect("extract");

        assert_eq!(document.conflicts.len(), 1);
        let conflict = &document.conflicts[0];
        assert_eq!(conflict.page, 0);
        assert_eq!(conflict.kind, ConflictKind::TextMismatch);
        assert_eq!(conflict.variants.len(), 2, "both readings are kept");
        assert!(
            document.pages[0].provenance.verified,
            "the page records that it was checked"
        );
    }

    /// Turning verification off has to mean the second engine does not run —
    /// not that its answer is discarded. The cost is the reason the policy
    /// exists.
    #[test]
    fn a_policy_of_off_runs_only_one_engine() {
        let (extractor, calls) = disagreeing(VerifyPolicy::off());

        let document = extract(&extractor, &image(), "scan.png").expect("extract");

        assert_eq!(calls.count(), 0, "the verifier was never asked to read");
        assert!(document.conflicts.is_empty());
        assert!(!document.pages[0].provenance.verified);
    }

    #[test]
    fn engines_that_agree_produce_no_conflict() {
        let (recogniser, calls) = ocr_saying("Revenue increased by 12%");
        let (reader, _) = vision_saying("Revenue  increased  by 12%", false);
        let extractor = ScanExtractor::new(Some(recogniser), Some(reader))
            .with_policy(VerifyPolicy::always());

        let document = extract(&extractor, &image(), "scan.png").expect("extract");

        assert_eq!(calls.count(), 1, "the check did run");
        assert!(document.conflicts.is_empty(), "{:?}", document.conflicts);
        assert!(document.pages[0].provenance.verified);
    }

    /// The default buys a second opinion where the text holds a value. A vision
    /// model reports no confidence, so the field trigger is the only reason it
    /// can fire on one.
    #[test]
    fn the_default_policy_checks_a_page_holding_an_amount() {
        let (recogniser, calls) = ocr_saying("Total: $1,234.00");
        let (reader, _) = vision_saying("Total: $1,234.00", false);
        let extractor = ScanExtractor::new(Some(recogniser), Some(reader));

        let document = extract(&extractor, &image(), "scan.png").expect("extract");

        assert_eq!(calls.count(), 1);
        assert!(document.pages[0].provenance.verified);
    }

    #[test]
    fn the_default_policy_leaves_ordinary_prose_alone() {
        let (recogniser, calls) = ocr_saying("a paragraph of ordinary words");
        let (reader, _) = vision_saying("a paragraph of ordinary words", false);
        let extractor = ScanExtractor::new(Some(recogniser), Some(reader));

        let document = extract(&extractor, &image(), "scan.png").expect("extract");

        assert_eq!(calls.count(), 0, "nothing about this page warranted a check");
        assert!(!document.pages[0].provenance.verified);
    }

    /// A thousand-page scan must not be able to run up an unbounded bill.
    #[test]
    fn the_ceiling_bounds_how_many_pages_are_checked() {
        let (recogniser, calls) = ocr_saying("read by ocr");
        let (reader, _) = vision_saying("read by vision", false);
        let extractor = ScanExtractor::new(Some(recogniser), Some(reader))
            .with_policy(VerifyPolicy {
                max_verify_pages: 1,
                ..VerifyPolicy::always()
            });

        let scanned = fixtures::pdf_pages(&[&[], &[], &[]]);
        let document = extract(&extractor, &scanned, "scan.pdf").expect("extract");

        assert_eq!(document.pages.len(), 3);
        assert_eq!(calls.count(), 1, "the ceiling is spent once, not per page");
        assert!(document.pages[0].provenance.verified);
        assert!(!document.pages[1].provenance.verified);
        assert!(!document.pages[2].provenance.verified);
    }

    #[test]
    fn a_ceiling_of_zero_checks_nothing() {
        let (extractor, calls) = disagreeing(VerifyPolicy {
            max_verify_pages: 0,
            ..VerifyPolicy::always()
        });

        let document = extract(&extractor, &image(), "scan.png").expect("extract");

        assert_eq!(calls.count(), 0);
        assert!(document.conflicts.is_empty());
    }

    /// A backend that cannot be reached is a broken engine, not an input
    /// nothing can read. Reporting it as the latter sent a reader to check a
    /// configuration that was already correct, with the real reason trailing in
    /// parentheses behind a sentence contradicting it.
    #[test]
    fn an_unreachable_backend_is_a_failure_not_a_decline() {
        let extractor = ScanExtractor::new(Some(ocr()), Some(vision(true)));

        let error = extract(&extractor, &image(), "scan.png").expect_err("should fail");

        assert!(
            matches!(error, ExtractError::Failed { .. }),
            "a broken backend is a failure: {error}"
        );
        assert!(
            !error.to_string().contains("no OCR or vision backend is configured"),
            "and the message must not contradict itself: {error}"
        );
    }

    /// With no vision engine there is no second lineage to check the first, and
    /// the page says so rather than claiming a check that never happened.
    #[test]
    fn the_only_engine_cannot_verify_itself() {
        let extractor = ScanExtractor::new(Some(ocr()), None).with_policy(VerifyPolicy::always());

        let document = extract(&extractor, &image(), "scan.png").expect("extract");

        assert!(document.conflicts.is_empty());
        assert!(!document.pages[0].provenance.verified);
    }

    /// The ceiling is spent per document, so a batch of scans cannot each spend
    /// the whole budget.
    #[test]
    fn the_ceiling_is_per_document() {
        let (extractor, calls) = disagreeing(VerifyPolicy {
            max_verify_pages: 1,
            ..VerifyPolicy::always()
        });

        let _ = extract(&extractor, &image(), "first.png").expect("extract");
        let _ = extract(&extractor, &image(), "second.png").expect("extract");

        assert_eq!(calls.count(), 2, "each document gets its own ceiling");
    }
}
