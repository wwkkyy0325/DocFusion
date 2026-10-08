//! Spreadsheet extraction.
//!
//! Reading XLSX is more involved than the other OOXML formats — shared string
//! tables, cell types, date serials — so this delegates the parsing to
//! `calamine` rather than reimplementing it. Each sheet becomes a page holding
//! its name and one table.

use std::io::Cursor;

use calamine::{Data, Range, Reader, Xlsx, open_workbook_from_rs};

use crate::ir::{Block, BlockKind, DocMeta, Document, EngineId, Page, PageProvenance};
use crate::mime::Container;

use super::ooxml;
use super::{ExtractError, Extractor, Fit, Input, InputProbe};

const ENGINE_ID: &str = "xlsx";
const ENGINE_VERSION: &str = "1.0.0";

const WORKBOOK_PART: &str = "xl/workbook.xml";

pub struct XlsxParser;

impl Extractor for XlsxParser {
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

        // Checked before handing the bytes over, because the reader reports an
        // unreadable file and a file that is simply not a spreadsheet the same
        // way — and only one of those should let routing continue.
        let mut archive = ooxml::open(input.bytes)
            .map_err(|message| ExtractError::failed(engine.clone(), message))?;
        let has_workbook = ooxml::entry(&mut archive, WORKBOOK_PART)
            .map_err(|message| ExtractError::failed(engine.clone(), message))?
            .is_some();
        drop(archive);

        if !has_workbook {
            return Err(ExtractError::unsupported(
                "this archive has no xl/workbook.xml, so it is not a spreadsheet",
            ));
        }

        let mut workbook = open_workbook_from_rs::<Xlsx<Cursor<&[u8]>>, _>(Cursor::new(input.bytes))
            .map_err(|error| {
                ExtractError::failed(engine.clone(), format!("could not read the workbook: {error}"))
            })?;

        let mut pages = Vec::new();
        for (index, name) in workbook.sheet_names().iter().enumerate() {
            let range = workbook.worksheet_range(name).map_err(|error| {
                ExtractError::failed(
                    engine.clone(),
                    format!("could not read sheet {name}: {error}"),
                )
            })?;
            pages.push(sheet(index as u32, name, &range, &engine));
        }

        Ok(Document {
            source: input.source.clone(),
            pages,
            meta: DocMeta::default(),
            conflicts: Vec::new(),
        })
    }
}

fn sheet(index: u32, name: &str, range: &Range<Data>, engine: &EngineId) -> Page {
    let mut blocks = vec![Block::text(
        BlockKind::Heading { level: 2 },
        name.to_owned(),
        engine.clone(),
    )];

    if !range.is_empty() {
        blocks.push(table(range, engine));
    }

    Page {
        index,
        // A sheet has no printed geometry until someone sets one up.
        size: None,
        blocks,
        provenance: PageProvenance {
            primary: engine.clone(),
            engines: vec![engine.clone()],
            verified: false,
        },
    }
}

fn table(range: &Range<Data>, engine: &EngineId) -> Block {
    let rows = range
        .rows()
        .map(|row| Block {
            kind: BlockKind::TableRow,
            text: String::new(),
            bbox: None,
            confidence: None,
            engine: engine.clone(),
            children: row
                .iter()
                .map(|cell| Block {
                    kind: BlockKind::TableCell,
                    text: cell_text(cell),
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

fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(text) => text.clone(),
        Data::Int(value) => value.to_string(),
        Data::Float(value) => whole_number(*value),
        Data::Bool(value) => value.to_string(),
        Data::DateTime(value) => value.to_string(),
        Data::DateTimeIso(text) | Data::DurationIso(text) => text.clone(),
        Data::Error(error) => format!("{error:?}"),
    }
}

/// Renders a float without a trailing `.0`.
///
/// Spreadsheets store every number as a float, so an integer cell arrives as
/// `42.0`. Writing that back out would be a formatting artefact rather than
/// anything the sheet actually said.
fn whole_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::SourceRef;
    use std::io::Write;
    use std::path::PathBuf;
    use zip::write::SimpleFileOptions;

    const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
</Types>"#;

    const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#;

    const WORKBOOK: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#;

    const WORKBOOK_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
</Relationships>"#;

    /// Builds a one-sheet workbook around the supplied `sheetData` body.
    fn fixture(sheet_data: &str) -> Vec<u8> {
        let sheet = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData>{sheet_data}</sheetData>
</worksheet>"#
        );

        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in [
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", ROOT_RELS),
            ("xl/workbook.xml", WORKBOOK),
            ("xl/_rels/workbook.xml.rels", WORKBOOK_RELS),
            ("xl/worksheets/sheet1.xml", sheet.as_str()),
        ] {
            writer
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn text_cell(reference: &str, value: &str) -> String {
        format!(r#"<c r="{reference}" t="inlineStr"><is><t>{value}</t></is></c>"#)
    }

    fn number_cell(reference: &str, value: &str) -> String {
        format!(r#"<c r="{reference}"><v>{value}</v></c>"#)
    }

    fn extract(bytes: &[u8]) -> Result<Document, ExtractError> {
        let probe = InputProbe::new(bytes, Some("fixture.xlsx".into()), bytes.len() as u64);
        let source = SourceRef {
            path: PathBuf::from("fixture.xlsx"),
            content_hash: "0".repeat(64),
            mime: "application/zip".into(),
            size: bytes.len() as u64,
            mtime: None,
        };
        XlsxParser.extract(&Input {
            probe: &probe,
            bytes,
            source: &source,
        })
    }

    fn rows(document: &Document) -> Vec<Vec<String>> {
        document
            .blocks()
            .filter(|block| block.kind == BlockKind::TableRow)
            .map(|row| {
                row.children
                    .iter()
                    .map(|cell| cell.text.clone())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn extracts_a_grid_of_text_cells() {
        let document = extract(&fixture(
            "<row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>Name</t></is></c>\
             <c r=\"B1\" t=\"inlineStr\"><is><t>Score</t></is></c></row>",
        ))
        .expect("extract");

        assert_eq!(rows(&document), vec![vec!["Name", "Score"]]);
    }

    #[test]
    fn names_the_sheet_as_a_heading() {
        let document = extract(&fixture(&text_cell("A1", "x"))).expect("extract");
        let heading = document.blocks().next().unwrap();

        assert_eq!(heading.kind, BlockKind::Heading { level: 2 });
        assert_eq!(heading.text, "Sheet1");
    }

    /// Every number is stored as a float, so an integer cell arrives as `42.0`.
    /// Writing that back would be a formatting artefact, not data.
    #[test]
    fn whole_numbers_are_not_shown_with_a_decimal_point() {
        let document = extract(&fixture(&format!(
            "<row r=\"1\">{}{}</row>",
            number_cell("A1", "42"),
            number_cell("B1", "1.5")
        )))
        .expect("extract");

        assert_eq!(rows(&document), vec![vec!["42", "1.5"]]);
    }

    /// The reader normalises a sheet to a rectangle, padding short rows and
    /// keeping gaps in place. That is what keeps a value under its own column
    /// heading, so it is asserted rather than assumed.
    #[test]
    fn rows_are_padded_so_columns_stay_aligned() {
        let document = extract(&fixture(&format!(
            "<row r=\"1\">{}</row><row r=\"2\">{}{}</row>",
            text_cell("A1", "only"),
            text_cell("A2", "a"),
            text_cell("C2", "c")
        )))
        .expect("extract");

        let rows = rows(&document);
        assert_eq!(rows[0], vec!["only", "", ""]);
        assert_eq!(rows[1], vec!["a", "", "c"]);
    }

    #[test]
    fn an_archive_without_a_workbook_declines() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("word/document.xml", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"<document/>").unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        let error = extract(&bytes).expect_err("should decline");
        assert!(
            matches!(error, ExtractError::Unsupported { .. }),
            "expected a decline so routing continues, got {error}"
        );
    }

    #[test]
    fn an_empty_sheet_produces_a_heading_and_no_table() {
        let document = extract(&fixture("")).expect("extract");
        let kinds: Vec<&BlockKind> = document.blocks().map(|b| &b.kind).collect();

        assert_eq!(kinds, vec![&BlockKind::Heading { level: 2 }]);
    }

    #[test]
    fn accepts_zip_containers_speculatively() {
        let zip = InputProbe::new(b"PK\x03\x04", None, 4);
        assert_eq!(XlsxParser.accepts(&zip), Fit::Maybe);
    }
}
