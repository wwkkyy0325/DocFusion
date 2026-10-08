//! Word document extraction.
//!
//! A docx is a ZIP archive whose `word/document.xml` holds the body. Paragraph
//! content lives in runs, headings are declared by paragraph properties, and
//! tables nest rows and cells.

use crate::ir::{Block, BlockKind, DocMeta, Document, EngineId, Page, PageProvenance};
use crate::mime::Container;

use super::ooxml::{self, Element};
use super::{ExtractError, Extractor, Fit, Input, InputProbe};

const ENGINE_ID: &str = "docx";
const ENGINE_VERSION: &str = "1.0.0";

const DOCUMENT_PART: &str = "word/document.xml";

pub struct DocxParser;

impl Extractor for DocxParser {
    fn id(&self) -> EngineId {
        EngineId::from(ENGINE_ID)
    }

    fn version(&self) -> &str {
        ENGINE_VERSION
    }

    /// `Maybe`, not `Yes`: a docx, an xlsx and a pptx are all ZIP archives, and
    /// only the parts inside tell them apart.
    fn accepts(&self, probe: &InputProbe) -> Fit {
        match probe.container {
            Container::Zip => Fit::Maybe,
            _ => Fit::No,
        }
    }

    fn extract(&self, input: &Input<'_>) -> Result<Document, ExtractError> {
        let engine = self.id();
        let mut archive =
            ooxml::open(input.bytes).map_err(|message| ExtractError::failed(engine.clone(), message))?;

        let Some(xml) = ooxml::entry(&mut archive, DOCUMENT_PART)
            .map_err(|message| ExtractError::failed(engine.clone(), message))?
        else {
            return Err(ExtractError::unsupported(
                "this archive has no word/document.xml, so it is not a Word document",
            ));
        };

        let root =
            ooxml::parse(&xml).map_err(|message| ExtractError::failed(engine.clone(), message))?;
        let body = root.descendant("body").ok_or_else(|| {
            ExtractError::failed(engine.clone(), "the Word document has no body")
        })?;

        Ok(Document {
            source: input.source.clone(),
            pages: vec![Page {
                index: 0,
                // A Word document has no fixed page geometry until it is laid
                // out for printing, which is not what is being extracted.
                size: None,
                blocks: blocks(body, &engine),
                provenance: PageProvenance {
                    primary: engine.clone(),
                    engines: vec![engine],
                    verified: false,
                },
            }],
            meta: DocMeta::default(),
            conflicts: Vec::new(),
        })
    }
}

fn blocks(body: &Element, engine: &EngineId) -> Vec<Block> {
    let mut blocks = Vec::new();

    for child in &body.children {
        match child.name.as_str() {
            "p" => {
                if let Some(block) = paragraph(child, engine) {
                    blocks.push(block);
                }
            }
            "tbl" => blocks.push(table(child, engine)),
            // Property and section elements carry no content of their own.
            _ => {}
        }
    }

    blocks
}

/// Builds a block for one paragraph, or `None` for one with no text.
///
/// Empty paragraphs are dropped rather than turned into blank blocks: Markdown
/// separates blocks with blank lines already, so keeping them would only add
/// noise the renderer had to filter back out.
fn paragraph(node: &Element, engine: &EngineId) -> Option<Block> {
    let text = paragraph_text(node);
    if text.trim().is_empty() {
        return None;
    }

    let kind = match heading_level(node) {
        Some(level) => BlockKind::Heading { level },
        None if is_list_item(node) => BlockKind::ListItem,
        None => BlockKind::Paragraph,
    };

    Some(Block {
        kind,
        text,
        bbox: None,
        confidence: None,
        engine: engine.clone(),
        children: Vec::new(),
    })
}

/// Determines a heading level from paragraph properties.
///
/// `outlineLvl` is preferred over the style name because it is a number rather
/// than an identifier, so it survives localisation — a Chinese Word document
/// names its heading styles differently but still writes the outline level.
fn heading_level(node: &Element) -> Option<u8> {
    let properties = node.child("pPr")?;

    if let Some(level) = properties.child("outlineLvl").and_then(|n| n.attribute("val"))
        && let Ok(level) = level.parse::<u8>()
    {
        // Outline levels are zero-based; heading levels are not.
        return Some((level + 1).clamp(1, 6));
    }

    let style = properties.child("pStyle")?.attribute("val")?;
    style_heading_level(style)
}

fn style_heading_level(style: &str) -> Option<u8> {
    let lowered = style.to_ascii_lowercase();

    if lowered == "title" {
        return Some(1);
    }

    if lowered.starts_with("heading") {
        let digits: String = style.chars().filter(char::is_ascii_digit).collect();
        if let Ok(level) = digits.parse::<u8>() {
            return Some(level.clamp(1, 6));
        }
    }

    None
}

fn is_list_item(node: &Element) -> bool {
    node.child("pPr")
        .and_then(|properties| properties.child("numPr"))
        .is_some()
}

/// Concatenates a paragraph's visible text.
///
/// Recurses rather than reading direct children because content is nested:
/// a hyperlink wraps its own runs, and a smart tag wraps those.
fn paragraph_text(node: &Element) -> String {
    let mut text = String::new();
    collect_text(node, &mut text);
    text
}

fn collect_text(node: &Element, out: &mut String) {
    for child in &node.children {
        match child.name.as_str() {
            "t" => out.push_str(&child.text),
            // A tab is content, not formatting; collapsing it loses alignment
            // in the many documents that use tabs to lay out columns.
            "tab" => out.push('\t'),
            "br" | "cr" => out.push('\n'),
            // Text deleted by a tracked change is not part of the document.
            "delText" => {}
            _ => collect_text(child, out),
        }
    }
}

fn table(node: &Element, engine: &EngineId) -> Block {
    let rows = node
        .children_named("tr")
        .map(|row| Block {
            kind: BlockKind::TableRow,
            text: String::new(),
            bbox: None,
            confidence: None,
            engine: engine.clone(),
            children: row
                .children_named("tc")
                .map(|cell| Block {
                    kind: BlockKind::TableCell,
                    text: cell
                        .children_named("p")
                        .map(paragraph_text)
                        .collect::<Vec<_>>()
                        .join("\n"),
                    bbox: None,
                    confidence: None,
                    engine: engine.clone(),
                    children: Vec::new(),
                })
                .collect(),
        })
        .collect();

    Block {
        kind: BlockKind::Table,
        text: String::new(),
        bbox: None,
        confidence: None,
        engine: engine.clone(),
        children: rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::SourceRef;
    use std::io::{Cursor, Write};
    use std::path::PathBuf;
    use zip::write::SimpleFileOptions;

    /// Builds a docx in memory. Fixtures are generated rather than committed so
    /// the repository carries no opaque binaries.
    fn fixture(body: &str) -> Vec<u8> {
        let document = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>{body}</w:body>
</w:document>"#
        );

        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(DOCUMENT_PART, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(document.as_bytes()).unwrap();
        writer
            .start_file("[Content_Types].xml", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"<Types/>").unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn extract(bytes: &[u8]) -> Result<Document, ExtractError> {
        let probe = InputProbe::new(bytes, Some("fixture.docx".into()), bytes.len() as u64);
        let source = SourceRef {
            path: PathBuf::from("fixture.docx"),
            content_hash: "0".repeat(64),
            mime: "application/zip".into(),
            size: bytes.len() as u64,
            mtime: None,
        };
        DocxParser.extract(&Input {
            probe: &probe,
            bytes,
            source: &source,
        })
    }

    fn texts(document: &Document) -> Vec<String> {
        document.blocks().map(|block| block.text.clone()).collect()
    }

    #[test]
    fn accepts_zip_containers_speculatively() {
        let zip = InputProbe::new(b"PK\x03\x04", None, 4);
        let pdf = InputProbe::new(b"%PDF-1.7", None, 8);
        assert_eq!(DocxParser.accepts(&zip), Fit::Maybe);
        assert_eq!(DocxParser.accepts(&pdf), Fit::No);
    }

    #[test]
    fn extracts_paragraph_text() {
        let document = extract(&fixture(
            "<w:p><w:r><w:t>Hello world</w:t></w:r></w:p>",
        ))
        .expect("extract");
        assert_eq!(texts(&document), vec!["Hello world"]);
    }

    /// Text inside a paragraph is split across runs whenever formatting
    /// changes, so the runs must be joined rather than each becoming a block.
    #[test]
    fn joins_runs_within_a_paragraph() {
        let document = extract(&fixture(
            "<w:p><w:r><w:t>Hello </w:t></w:r><w:r><w:t>world</w:t></w:r></w:p>",
        ))
        .expect("extract");
        assert_eq!(texts(&document), vec!["Hello world"]);
    }

    #[test]
    fn style_names_produce_headings() {
        let document = extract(&fixture(
            "<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p>\
             <w:p><w:r><w:t>Body</w:t></w:r></w:p>",
        ))
        .expect("extract");

        let blocks: Vec<&Block> = document.blocks().collect();
        assert_eq!(blocks[0].kind, BlockKind::Heading { level: 1 });
        assert_eq!(blocks[1].kind, BlockKind::Paragraph);
    }

    /// The outline level is a number, so it survives localisation; a Chinese
    /// Word document names its heading styles differently but still writes it.
    #[test]
    fn outline_levels_produce_headings_regardless_of_style_name() {
        let document = extract(&fixture(
            "<w:p><w:pPr><w:pStyle w:val=\"2\"/><w:outlineLvl w:val=\"1\"/></w:pPr>\
             <w:r><w:t>第二章</w:t></w:r></w:p>",
        ))
        .expect("extract");

        let block = document.blocks().next().unwrap();
        assert_eq!(block.kind, BlockKind::Heading { level: 2 });
        assert_eq!(block.text, "第二章");
    }

    #[test]
    fn a_title_style_is_a_first_level_heading() {
        let document = extract(&fixture(
            "<w:p><w:pPr><w:pStyle w:val=\"Title\"/></w:pPr><w:r><w:t>T</w:t></w:r></w:p>",
        ))
        .expect("extract");
        assert_eq!(
            document.blocks().next().unwrap().kind,
            BlockKind::Heading { level: 1 }
        );
    }

    #[test]
    fn numbered_paragraphs_become_list_items() {
        let document = extract(&fixture(
            "<w:p><w:pPr><w:numPr><w:ilvl w:val=\"0\"/></w:numPr></w:pPr>\
             <w:r><w:t>First</w:t></w:r></w:p>",
        ))
        .expect("extract");
        assert_eq!(document.blocks().next().unwrap().kind, BlockKind::ListItem);
    }

    #[test]
    fn empty_paragraphs_are_dropped() {
        let document = extract(&fixture(
            "<w:p><w:r><w:t>One</w:t></w:r></w:p><w:p/><w:p><w:r><w:t>Two</w:t></w:r></w:p>",
        ))
        .expect("extract");
        assert_eq!(texts(&document), vec!["One", "Two"]);
    }

    #[test]
    fn tabs_and_breaks_are_preserved() {
        let document = extract(&fixture(
            "<w:p><w:r><w:t>a</w:t><w:tab/><w:t>b</w:t><w:br/><w:t>c</w:t></w:r></w:p>",
        ))
        .expect("extract");
        assert_eq!(texts(&document), vec!["a\tb\nc"]);
    }

    #[test]
    fn deleted_revision_text_is_excluded() {
        let document = extract(&fixture(
            "<w:p><w:r><w:t>kept</w:t></w:r><w:del><w:r><w:delText>removed</w:delText></w:r></w:del></w:p>",
        ))
        .expect("extract");
        assert_eq!(texts(&document), vec!["kept"]);
    }

    #[test]
    fn text_wrapped_in_a_hyperlink_is_still_read() {
        let document = extract(&fixture(
            "<w:p><w:hyperlink r:id=\"rId1\"><w:r><w:t>link text</w:t></w:r></w:hyperlink></w:p>",
        ))
        .expect("extract");
        assert_eq!(texts(&document), vec!["link text"]);
    }

    #[test]
    fn tables_keep_their_row_and_cell_structure() {
        let document = extract(&fixture(
            "<w:tbl>\
               <w:tr><w:tc><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc>\
                     <w:tc><w:p><w:r><w:t>B1</w:t></w:r></w:p></w:tc></w:tr>\
               <w:tr><w:tc><w:p><w:r><w:t>A2</w:t></w:r></w:p></w:tc>\
                     <w:tc><w:p><w:r><w:t>B2</w:t></w:r></w:p></w:tc></w:tr>\
             </w:tbl>",
        ))
        .expect("extract");

        // A grid needs both dimensions, so rows must stay distinct.
        let table = document.blocks().next().unwrap();
        assert_eq!(table.kind, BlockKind::Table);
        assert_eq!(table.children.len(), 2, "two rows");

        let cells: Vec<&str> = table.children[0]
            .children
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
        assert_eq!(cells, vec!["A1", "B1"]);
        assert_eq!(table.children[1].children[1].text, "B2");
    }

    #[test]
    fn an_archive_without_a_document_part_declines() {
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
