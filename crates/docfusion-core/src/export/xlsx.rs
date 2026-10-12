//! A table as a workbook.
//!
//! CSV is the table format that opens anywhere; this is the one that is a
//! spreadsheet. The cells arrive as cells and a number arrives as a number,
//! which is the whole reason to write one rather than the other — a column of
//! figures stored as text is a column Excel will sort alphabetically and will
//! not add up.
//!
//! One sheet per table, named after nothing in particular: a sheet's name is
//! thirty-one characters of a namespace with its own rules about what may be in
//! it, and a heading that happens to break one of them would be a workbook
//! nobody can open.

use std::fmt::Write as _;

use super::xml;
use super::Table;

/// What a sheet is called when nothing better is at hand.
///
/// A sheet's name is thirty-one characters of a namespace with rules of its
/// own — no `[`, `]`, `*`, `?`, `/`, `\` — and a document's own heading is
/// exactly the sort of string that breaks one of them.
const SHEET_NAME: &str = "Sheet";

/// A workbook with one sheet per table.
///
/// **`None` for nothing to write.** A workbook with no sheets in it is not a
/// thing anybody asked for, and the same answer CSV gives to the same request —
/// see the module above for why a request that cannot be answered is refused
/// rather than met with an empty file.
pub fn of(tables: &[Table]) -> Option<Vec<u8>> {
    if tables.is_empty() {
        return None;
    }

    let mut content_types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" \
         ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/xl/workbook.xml\" \
         ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>",
    );

    for at in 1..=tables.len() {
        write!(
            content_types,
            "<Override PartName=\"/xl/worksheets/sheet{at}.xml\" ContentType=\"\
             application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"
        )
        .expect("a String is always writable");
    }

    content_types.push_str("</Types>");

    let mut workbook = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <sheets>",
    );

    let mut relationships = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );

    for at in 1..=tables.len() {
        write!(
            workbook,
            "<sheet name=\"{SHEET_NAME}{at}\" sheetId=\"{at}\" r:id=\"rId{at}\"/>"
        )
        .expect("a String is always writable");

        write!(
            relationships,
            "<Relationship Id=\"rId{at}\" Type=\"\
             http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" \
             Target=\"worksheets/sheet{at}.xml\"/>"
        )
        .expect("a String is always writable");
    }

    workbook.push_str("</sheets></workbook>");
    relationships.push_str("</Relationships>");

    // Held in a binding of its own because the file list borrows them; `zip`
    // writes what it is handed and keeps nothing.
    let names: Vec<String> = (1..=tables.len())
        .map(|at| format!("xl/worksheets/sheet{at}.xml"))
        .collect();

    // `[Content_Types].xml` first; see `xml::zip`.
    let mut files: Vec<(&str, String)> = vec![
        ("[Content_Types].xml", content_types),
        (
            "_rels/.rels",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
             <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
             <Relationship Id=\"rId1\" Type=\"\
             http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" \
             Target=\"xl/workbook.xml\"/></Relationships>"
                .to_owned(),
        ),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", relationships),
    ];

    for (name, table) in names.iter().zip(tables) {
        files.push((name.as_str(), sheet(table)));
    }

    Some(xml::zip(&files))
}

/// One sheet: the rows, in order, with a reference on every cell.
fn sheet(rows: &[Vec<String>]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
         <sheetData>",
    );

    for (at, row) in rows.iter().enumerate() {
        let number = at + 1;
        write!(out, "<row r=\"{number}\">").expect("a String is always writable");

        for (column, cell) in row.iter().enumerate() {
            let reference = format!("{}{number}", column_name(column));

            match number_of(cell) {
                Some(value) => write!(out, "<c r=\"{reference}\"><v>{value}</v></c>"),
                None => write!(
                    out,
                    "<c r=\"{reference}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",
                    xml::escape(cell)
                ),
            }
            .expect("a String is always writable");
        }

        out.push_str("</row>");
    }

    out.push_str("</sheetData></worksheet>");
    out
}

/// A cell as a number, when writing it as text would lose something.
///
/// **Only when the number and the text say the same thing**, and that one
/// condition is the whole rule. `007` is not seven, `12.50` is not twelve and a
/// half — both are values whose *text* carries a digit the number does not, and
/// a column of money that has lost its last place is worse than a column Excel
/// cannot add up. Everything that survives the round trip is a number; `1,000`,
/// `1.2.3`, `2024-03-01` and a phone number do not parse at all.
///
/// A cell that is not a number stays exactly as the document wrote it.
fn number_of(cell: &str) -> Option<String> {
    let value: f64 = cell.parse().ok()?;

    // `inf` and `NaN` parse and are not numbers a spreadsheet has a cell for.
    if !value.is_finite() {
        return None;
    }

    let written = value.to_string();
    (written == cell).then_some(written)
}

/// The name of a column, counting from zero: `A`, `B`, … `Z`, `AA`.
///
/// An xlsx cell is addressed by name and not by index, which is the one piece of
/// arithmetic in the format that is not obvious: after `Z` comes `AA`, so it is
/// base twenty-six with no zero.
fn column_name(mut column: usize) -> String {
    let mut out = String::new();

    loop {
        out.insert(0, (b'A' + (column % 26) as u8) as char);
        if column < 26 {
            return out;
        }
        column = column / 26 - 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    fn table(rows: &[&[&str]]) -> Table {
        rows.iter()
            .map(|row| row.iter().map(|cell| (*cell).to_owned()).collect())
            .collect()
    }

    fn part(bytes: &[u8], name: &str) -> String {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("a zip");
        let mut body = String::new();
        archive
            .by_name(name)
            .unwrap_or_else(|_| panic!("no {name} in the workbook"))
            .read_to_string(&mut body)
            .expect("utf-8");
        body
    }

    #[test]
    fn a_cell_is_where_a_spreadsheet_looks_for_it() {
        let bytes = of(&[table(&[&["名称", "数量"], &["甲方", "12"]])]).expect("a workbook");
        let sheet = part(&bytes, "xl/worksheets/sheet1.xml");

        assert!(sheet.contains("<c r=\"A1\" t=\"inlineStr\"><is><t xml:space=\"preserve\">名称</t></is></c>"), "{sheet}");
        assert!(sheet.contains("<c r=\"B2\"><v>12</v></c>"), "a figure is a number: {sheet}");
    }

    /// The whole reason this format is here rather than CSV alone: a figure the
    /// document wrote as a figure is a figure Excel can add up.
    #[test]
    fn a_number_is_a_number_and_the_text_that_is_not_one_stays_text() {
        assert_eq!(number_of("12").as_deref(), Some("12"));
        assert_eq!(number_of("-3.5").as_deref(), Some("-3.5"));

        // Each of these is a value whose text says something the number does
        // not, and the text is what the document said.
        assert_eq!(number_of("007"), None);
        assert_eq!(number_of("12.50"), None);
        assert_eq!(number_of("1,000"), None);
        assert_eq!(number_of("1.2.3"), None);
        assert_eq!(number_of("2024-03-01"), None);
        assert_eq!(number_of("13800138000\n"), None);
        assert_eq!(number_of(""), None);
        assert_eq!(number_of("inf"), None);
    }

    #[test]
    fn columns_are_named_the_way_a_spreadsheet_names_them() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(column_name(27), "AB");
        assert_eq!(column_name(51), "AZ");
        assert_eq!(column_name(52), "BA");
        assert_eq!(column_name(701), "ZZ");
        assert_eq!(column_name(702), "AAA");
    }

    /// Two tables are two sheets, and both are in the package the workbook
    /// claims to hold — a sheet the content types do not mention is a workbook
    /// Excel refuses to open.
    #[test]
    fn every_table_is_a_sheet_and_every_sheet_is_announced() {
        let bytes = of(&[table(&[&["a"]]), table(&[&["b"]])]).expect("a workbook");
        let types = part(&bytes, "[Content_Types].xml");
        let workbook = part(&bytes, "xl/workbook.xml");

        assert!(types.contains("/xl/worksheets/sheet2.xml"), "{types}");
        assert!(workbook.contains("name=\"Sheet2\""), "{workbook}");
        assert!(part(&bytes, "xl/worksheets/sheet2.xml").contains(">b<"));
    }

    /// A document is somebody else's text. An ampersand written raw into the
    /// sheet is a workbook that will not open at all.
    #[test]
    fn a_cell_holding_markup_is_escaped() {
        let bytes = of(&[table(&[&["a & b <c>"]])]).expect("a workbook");
        let sheet = part(&bytes, "xl/worksheets/sheet1.xml");

        assert!(sheet.contains("a &amp; b &lt;c&gt;"), "{sheet}");
        assert!(!sheet.contains("a & b"), "{sheet}");
    }
}
