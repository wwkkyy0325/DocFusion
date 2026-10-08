//! Presentation extraction.
//!
//! A pptx is a ZIP of one XML part per slide. Each slide becomes a page, and
//! each shape becomes blocks: a shape holding a title placeholder becomes a
//! heading, and bulleted paragraphs become list items.

use crate::ir::{Block, BlockKind, DocMeta, Document, EngineId, Page, PageProvenance, Size};
use crate::mime::Container;

use super::ooxml::{self, Archive, Element};
use super::{ExtractError, Extractor, Fit, Input, InputProbe};

const ENGINE_ID: &str = "pptx";
const ENGINE_VERSION: &str = "1.0.0";

const SLIDE_PREFIX: &str = "ppt/slides/slide";
const PRESENTATION_PART: &str = "ppt/presentation.xml";

/// DrawingML measures in English Metric Units: 914 400 to the inch, so 12 700
/// to the point.
const EMU_PER_POINT: f32 = 12_700.0;

/// Placeholder types that hold the slide's title.
const TITLE_PLACEHOLDERS: [&str; 2] = ["title", "ctrTitle"];

pub struct PptxParser;

impl Extractor for PptxParser {
    fn id(&self) -> EngineId {
        EngineId::from(ENGINE_ID)
    }

    fn version(&self) -> &str {
        ENGINE_VERSION
    }

    fn accepts(&self, probe: &InputProbe) -> Fit {
        match probe.container {
            Container::Zip => Fit::Maybe,
            _ => Fit::No,
        }
    }

    fn extract(&self, input: &Input<'_>) -> Result<Document, ExtractError> {
        let engine = self.id();
        let mut archive = ooxml::open(input.bytes)
            .map_err(|message| ExtractError::failed(engine.clone(), message))?;

        let slides = slide_parts(&archive);
        if slides.is_empty() {
            return Err(ExtractError::unsupported(
                "this archive has no ppt/slides, so it is not a presentation",
            ));
        }

        // Presentation geometry is stated once, so every slide page carries it.
        let size = presentation_size(&mut archive);

        let mut pages = Vec::with_capacity(slides.len());
        for (index, part) in slides.iter().enumerate() {
            let xml = ooxml::entry(&mut archive, part)
                .map_err(|message| ExtractError::failed(engine.clone(), message))?
                .ok_or_else(|| {
                    ExtractError::failed(engine.clone(), format!("{part} disappeared"))
                })?;

            let root = ooxml::parse(&xml)
                .map_err(|message| ExtractError::failed(engine.clone(), message))?;

            pages.push(Page {
                index: index as u32,
                size,
                blocks: blocks(&root, &engine),
                provenance: PageProvenance {
                    primary: engine.clone(),
                    engines: vec![engine.clone()],
                    verified: false,
                },
            });
        }

        Ok(Document {
            source: input.source.clone(),
            pages,
            meta: DocMeta::default(),
            conflicts: Vec::new(),
        })
    }
}

/// Slide parts in presentation order.
///
/// Sorted numerically rather than by name: a ten-slide deck has `slide10.xml`,
/// which sorts before `slide2.xml` as text and would silently reorder the
/// presentation.
fn slide_parts(archive: &Archive<'_>) -> Vec<String> {
    let mut parts: Vec<String> = ooxml::part_names(archive, SLIDE_PREFIX)
        .into_iter()
        .filter(|name| name.ends_with(".xml"))
        .collect();

    parts.sort_by_key(|name| slide_number(name).unwrap_or(u32::MAX));
    parts
}

fn slide_number(part: &str) -> Option<u32> {
    let stem = part
        .strip_prefix(SLIDE_PREFIX)?
        .strip_suffix(".xml")?;
    stem.parse().ok()
}

fn presentation_size(archive: &mut Archive<'_>) -> Option<Size> {
    let xml = ooxml::entry(archive, PRESENTATION_PART).ok()??;
    let root = ooxml::parse(&xml).ok()?;
    let declared = root.descendant("sldSz")?;

    let width: f32 = declared.attribute("cx")?.parse().ok()?;
    let height: f32 = declared.attribute("cy")?.parse().ok()?;

    Some(Size {
        w: width / EMU_PER_POINT,
        h: height / EMU_PER_POINT,
    })
}

fn blocks(root: &Element, engine: &EngineId) -> Vec<Block> {
    let mut shapes = Vec::new();
    root.descendants_named("sp", &mut shapes);

    let mut blocks = Vec::new();
    for shape in shapes {
        let is_title = is_title(shape);

        let Some(text_body) = shape.child("txBody") else {
            continue;
        };

        for paragraph in text_body.children_named("p") {
            let text = paragraph_text(paragraph);
            if text.trim().is_empty() {
                continue;
            }

            blocks.push(Block {
                kind: kind(paragraph, is_title),
                text,
                bbox: None,
                confidence: None,
                engine: engine.clone(),
                children: Vec::new(),
            });
        }
    }

    blocks
}

fn is_title(shape: &Element) -> bool {
    shape
        .descendant("ph")
        .and_then(|placeholder| placeholder.attribute("type"))
        .is_some_and(|kind| TITLE_PLACEHOLDERS.contains(&kind))
}

fn kind(paragraph: &Element, is_title: bool) -> BlockKind {
    if is_title {
        // Slides have one title level; a second would be a fiction the renderer
        // would have to invent nesting for.
        return BlockKind::Heading { level: 1 };
    }

    let bulleted = paragraph.child("pPr").is_some_and(|properties| {
        properties.child("buChar").is_some() || properties.child("buAutoNum").is_some()
    });

    if bulleted {
        BlockKind::ListItem
    } else {
        BlockKind::Paragraph
    }
}

fn paragraph_text(paragraph: &Element) -> String {
    let mut text = String::new();
    collect_text(paragraph, &mut text);
    text
}

fn collect_text(node: &Element, out: &mut String) {
    for child in &node.children {
        match child.name.as_str() {
            "t" => out.push_str(&child.text),
            "tab" => out.push('\t'),
            "br" => out.push('\n'),
            _ => collect_text(child, out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::SourceRef;
    use std::io::{Cursor, Write};
    use std::path::PathBuf;
    use zip::write::SimpleFileOptions;

    const PRESENTATION: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:sldSz cx="9144000" cy="6858000"/>
</p:presentation>"#;

    fn shape(placeholder: &str, paragraph: &str) -> String {
        format!(
            "<p:sp><p:nvSpPr><p:nvPr><p:ph type=\"{placeholder}\"/></p:nvPr></p:nvSpPr>\
             <p:txBody>{paragraph}</p:txBody></p:sp>"
        )
    }

    fn plain_run(text: &str) -> String {
        format!("<a:p><a:r><a:t>{text}</a:t></a:r></a:p>")
    }

    fn bullet_run(text: &str) -> String {
        format!("<a:p><a:pPr><a:buChar char=\"&#8226;\"/></a:pPr><a:r><a:t>{text}</a:t></a:r></a:p>")
    }

    fn fixture(slides: &[String]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));

        writer
            .start_file(PRESENTATION_PART, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(PRESENTATION.as_bytes()).unwrap();

        for (index, body) in slides.iter().enumerate() {
            let slide = format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
       xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<p:cSld><p:spTree>{body}</p:spTree></p:cSld></p:sld>"#
            );
            writer
                .start_file(
                    format!("{SLIDE_PREFIX}{}.xml", index + 1),
                    SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(slide.as_bytes()).unwrap();
            // Relationship parts share the prefix and must not be mistaken for
            // slides.
            writer
                .start_file(
                    format!("ppt/slides/_rels/slide{}.xml.rels", index + 1),
                    SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(b"<Relationships/>").unwrap();
        }

        writer.finish().unwrap().into_inner()
    }

    fn extract(bytes: &[u8]) -> Result<Document, ExtractError> {
        let probe = InputProbe::new(bytes, Some("fixture.pptx".into()), bytes.len() as u64);
        let source = SourceRef {
            path: PathBuf::from("fixture.pptx"),
            content_hash: "0".repeat(64),
            mime: "application/zip".into(),
            size: bytes.len() as u64,
            mtime: None,
        };
        PptxParser.extract(&Input {
            probe: &probe,
            bytes,
            source: &source,
        })
    }

    fn texts(document: &Document, page: usize) -> Vec<String> {
        document.pages[page]
            .blocks
            .iter()
            .map(|block| block.text.clone())
            .collect()
    }

    #[test]
    fn extracts_shape_text_from_a_slide() {
        let document = extract(&fixture(&[shape("body", &plain_run("Hello"))])).expect("extract");
        assert_eq!(texts(&document, 0), vec!["Hello"]);
    }

    #[test]
    fn title_placeholders_become_headings() {
        let document = extract(&fixture(&[format!(
            "{}{}",
            shape("title", &plain_run("Agenda")),
            shape("body", &plain_run("First point"))
        )]))
        .expect("extract");

        let blocks: Vec<&Block> = document.pages[0].blocks.iter().collect();
        assert_eq!(blocks[0].kind, BlockKind::Heading { level: 1 });
        assert_eq!(blocks[0].text, "Agenda");
        assert_eq!(blocks[1].kind, BlockKind::Paragraph);
    }

    #[test]
    fn centered_titles_are_headings_too() {
        let document = extract(&fixture(&[shape("ctrTitle", &plain_run("Cover"))])).expect("extract");
        assert_eq!(
            document.pages[0].blocks[0].kind,
            BlockKind::Heading { level: 1 }
        );
    }

    #[test]
    fn bulleted_paragraphs_become_list_items() {
        let document = extract(&fixture(&[shape("body", &bullet_run("Item"))])).expect("extract");
        assert_eq!(document.pages[0].blocks[0].kind, BlockKind::ListItem);
    }

    #[test]
    fn every_slide_becomes_a_page() {
        let document = extract(&fixture(&[
            shape("title", &plain_run("One")),
            shape("title", &plain_run("Two")),
            shape("title", &plain_run("Three")),
        ]))
        .expect("extract");

        assert_eq!(document.pages.len(), 3);
        assert_eq!(texts(&document, 2), vec!["Three"]);
    }

    /// A deck with ten or more slides has `slide10.xml`, which sorts before
    /// `slide2.xml` as text. Ordering by name would silently reshuffle the
    /// presentation.
    #[test]
    fn slides_are_ordered_numerically_not_alphabetically() {
        let slides: Vec<String> = (1..=11)
            .map(|n| shape("title", &plain_run(&format!("Slide {n}"))))
            .collect();
        let document = extract(&fixture(&slides)).expect("extract");

        let order: Vec<String> = (0..11).map(|i| texts(&document, i)[0].clone()).collect();
        assert_eq!(order[9], "Slide 10");
        assert_eq!(order[10], "Slide 11");
    }

    /// EMU is the unit DrawingML measures in; reporting raw EMU would make
    /// every downstream geometry comparison meaningless.
    #[test]
    fn slide_geometry_is_converted_to_points() {
        let document = extract(&fixture(&[shape("body", &plain_run("x"))])).expect("extract");
        let size = document.pages[0].size.expect("slide size");

        assert!((size.w - 720.0).abs() < 1.0, "{size:?}");
        assert!((size.h - 540.0).abs() < 1.0, "{size:?}");
    }

    #[test]
    fn runs_within_a_paragraph_are_joined() {
        let paragraph = "<a:p><a:r><a:t>Hello </a:t></a:r><a:r><a:t>world</a:t></a:r></a:p>";
        let document = extract(&fixture(&[shape("body", paragraph)])).expect("extract");
        assert_eq!(texts(&document, 0), vec!["Hello world"]);
    }

    #[test]
    fn empty_paragraphs_are_dropped() {
        let paragraph = format!(
            "{}<a:p/>{}",
            plain_run("kept"),
            plain_run("also kept")
        );
        let document = extract(&fixture(&[shape("body", &paragraph)])).expect("extract");
        assert_eq!(texts(&document, 0), vec!["kept", "also kept"]);
    }

    #[test]
    fn an_archive_without_slides_declines() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("xl/workbook.xml", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"<workbook/>").unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        let error = extract(&bytes).expect_err("should decline");
        assert!(
            matches!(error, ExtractError::Unsupported { .. }),
            "expected a decline so routing continues, got {error}"
        );
    }
}
