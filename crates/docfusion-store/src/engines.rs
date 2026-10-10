//! The engines the tool ships with, assembled the same way for every caller.
//!
//! Written once because it was written twice. The command line and the window
//! have to read a document the same way or they are not the same tool, and two
//! lists that agree today are two lists that disagree after the next change.

use docfusion_core::backend::VisionBackend;
use docfusion_core::extract::pdf::PdfTextLayer;
use docfusion_core::extract::{
    Extractor, docx::DocxParser, pptx::PptxParser, scan::ScanExtractor, text::TextPassthrough,
    xlsx::XlsxParser,
};
use docfusion_core::verify::VerifyPolicy;

use crate::endpoint::{EndpointConfig, OpenAiEndpoint};
use crate::ocr::RapidOcrBackend;
use crate::{structure, table};

/// Every engine, best fit first.
///
/// The order is load-bearing: engines that are equally plausible keep their
/// registration order, and the text-layer engine comes first so that a PDF
/// which can be parsed is parsed rather than rendered into images and sent to a
/// model.
///
/// `readers` is how many pages may be read at once. It goes to both the
/// extractor's threads and the recognition engine's pipelines, because threads
/// beyond pipelines queue and pipelines beyond threads are memory nobody reads
/// a page with.
pub fn standard(
    endpoint: Option<EndpointConfig>,
    policy: VerifyPolicy,
    readers: usize,
) -> Vec<Box<dyn Extractor>> {
    let vision: Option<Box<dyn VisionBackend>> =
        endpoint.map(|config| Box::new(OpenAiEndpoint::new(config)) as Box<dyn VisionBackend>);

    // A PDF that carries a text layer never reaches the recognition engine, and
    // its tables were therefore never found at all — the document came out as
    // the cells of its tables pressed into paragraphs. The two models that find
    // a table are offered to this path as well, so that the page is shown to
    // them and the words are taken from the file.
    //
    // Offered, not required: with either model missing the PDF reader is what it
    // always was, and every page still reads.
    let pdf = match (structure::available(), table::available()) {
        (true, true) => PdfTextLayer::new().with_tables(Box::new(structure::PageTables::new(
            structure::Detector::new(structure::model_path()),
            table::Reader::new(table::model_path()),
        ))),
        _ => PdfTextLayer::new(),
    };

    vec![
        Box::new(pdf),
        Box::new(DocxParser),
        Box::new(XlsxParser),
        Box::new(PptxParser),
        Box::new(TextPassthrough),
        // The local recognition engine always joins, and costs nothing until a
        // page actually needs reading. It is what makes a scan readable with no
        // setup: the weights are in the binary, so there is nothing to configure
        // and nothing to download.
        Box::new(
            ScanExtractor::new(
                Some(Box::new(RapidOcrBackend::bundled().with_pipelines(readers))),
                vision,
            )
            .with_policy(policy)
            .with_concurrency(readers),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use docfusion_core::extract::InputProbe;

    fn router(readers: usize) -> docfusion_core::extract::Router {
        docfusion_core::extract::Router::new(standard(None, VerifyPolicy::default(), readers))
    }

    /// The promise that makes the tool useful with no setup at all.
    #[test]
    fn every_deterministic_format_has_an_engine() {
        let router = router(1);

        for (name, head) in [
            ("report.pdf", &b"%PDF-1.7"[..]),
            ("report.docx", b"PK\x03\x04"),
            ("notes.md", b"# Notes"),
            ("plain text", b"hello"),
        ] {
            let probe = InputProbe::new(head, Some(name.to_owned()), head.len() as u64);
            assert!(router.can_handle(&probe), "nothing can read {name}");
        }
    }

    /// And a scan is readable with nothing configured, which is the other half
    /// of that promise.
    #[test]
    fn an_image_is_readable_with_nothing_configured() {
        let head = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

        assert!(router(1).can_handle(&InputProbe::new(head, None, 8)));
    }

    /// A reader count of zero would read nothing. The engines are the thing
    /// that has to refuse it, since the number arrives from a configuration
    /// file rather than from this code.
    #[test]
    fn a_reader_count_of_zero_still_builds() {
        assert_eq!(standard(None, VerifyPolicy::default(), 0).len(), 6);
    }
}
