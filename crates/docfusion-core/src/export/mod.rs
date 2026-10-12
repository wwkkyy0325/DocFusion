//! Taking a document, or a piece of one, out of this program as a file.
//!
//! **The formats are chosen by what they keep, not by what is easy to write.**
//! A button that copies gives you the words; an export that gave you the words
//! too would be a second button doing the first one's job. So an export here
//! always keeps something the words do not have — the shape of a table, the
//! emphasis in a paragraph, the heading levels — and the formats offered for a
//! block are the ones that suit what that block is.
//!
//! That is why plain text is offered for a whole document and not for a block,
//! and why the two spreadsheet formats are offered for a table and nothing else.

mod docx;
mod xlsx;
mod xml;

use crate::render;

/// One table's rows, each row a list of cells, each cell already the document's
/// own words with the bars and the padding taken off.
///
/// The shape both spreadsheet writers are handed, so that the one place that
/// reads a Markdown table is this file and not each of them.
pub(crate) type Table = Vec<Vec<String>>;

/// What an export can be written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// The document's own Markdown, which is what this program produces and
    /// what everything else is derived from.
    Markdown,
    /// A complete HTML page, so that it opens in a browser rather than showing
    /// its own source.
    Html,
    /// The words, and no structure.
    Text,
    /// A table as a table, for a spreadsheet.
    Csv,
    /// A document as Word holds one, so that what came out of the scanner can
    /// be worked in rather than only read.
    Docx,
    /// A table as a workbook: the cells as cells, and a figure as a figure.
    Xlsx,
}

impl Format {
    /// Every format, in the order a menu should offer them.
    pub const ALL: &'static [Format] = &[
        Format::Markdown,
        Format::Html,
        Format::Docx,
        Format::Xlsx,
        Format::Csv,
        Format::Text,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Html => "html",
            Format::Text => "txt",
            Format::Csv => "csv",
            Format::Docx => "docx",
            Format::Xlsx => "xlsx",
        }
    }

    pub fn extension(self) -> &'static str {
        self.as_str()
    }

    /// Whether this format is a spreadsheet.
    ///
    /// The one distinction that decides what a format suits, and it is the whole
    /// of the rule below: a spreadsheet is a table and only a table.
    pub fn spreadsheet(self) -> bool {
        matches!(self, Format::Csv | Format::Xlsx)
    }

    /// Whether this format is a file and cannot be written anywhere else.
    ///
    /// The two Office formats are zips. A zip written to a terminal is a screen
    /// of noise, and a caller that piped one into something expecting text would
    /// have nothing to read.
    pub fn binary(self) -> bool {
        matches!(self, Format::Docx | Format::Xlsx)
    }

    /// Whether this format can be used for a block, given whether it is a table.
    ///
    /// One rule, and it is about the spreadsheets alone: **a table can be a
    /// spreadsheet and nothing else can**. A paragraph written as comma
    /// separated values is a paragraph with quotes round it. Everything else can
    /// be written as anything — a table as Markdown is still a table, which is
    /// what the first attempt at this got backwards.
    pub fn suits(self, table: bool) -> bool {
        !self.spreadsheet() || table
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|format| format.as_str() == name.trim().to_ascii_lowercase())
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether writing this format to a file should begin with a byte-order mark.
///
/// **Prose files should not**, and a spreadsheet should. Excel on Windows reads
/// a `.csv` as the system's legacy code page unless something tells it
/// otherwise, and what tells it is a byte-order mark — without one, a table of
/// Chinese comes up as mojibake, which is the single most common way an export
/// that worked produces a file nobody can read. The others need nothing: `.html`
/// carries its own charset, and a `.md` or `.txt` with a mark in it is a file
/// that shows `﻿` at the front of every diff.
pub fn wants_a_mark(format: Format) -> bool {
    format == Format::Csv
}

/// A block of a document, or the whole of it, written as `format`.
///
/// `block` is the index of a top-level block as [`render::blocks`] returns them,
/// which is also the nth element the window drew. `None` is the whole document.
///
/// **`None` when the format does not suit the thing being asked for** — a CSV
/// of a paragraph — rather than a best effort. A caller that gets an empty
/// string cannot tell it from an empty document, and writing a file nobody asked
/// for is worse than not writing one.
///
/// Bytes rather than a string, because two of the formats are zips. What comes
/// back is the file's content and nothing else: the byte-order mark a
/// spreadsheet wants is the writer's to add, since only a file gets one — see
/// [`wants_a_mark`].
pub fn write(markdown: &str, format: Format, block: Option<usize>) -> Option<Vec<u8>> {
    let Some(index) = block else {
        return whole(markdown, format);
    };

    // Plain text is not offered for a block at all, and the refusal is here
    // rather than among the formats because it is not about the format: the
    // words with no structure are what the copy button gives, and a second
    // button doing the first one's job is a button that should not be there.
    if format == Format::Text {
        return None;
    }

    let blocks = render::blocks(markdown);
    let found = blocks.get(index)?;

    if !format.suits(found.table) {
        return None;
    }

    Some(match format {
        Format::Markdown => found.markdown.clone().into_bytes(),
        Format::Html => page(&found.html).into_bytes(),
        Format::Docx => docx::of(&found.markdown),
        Format::Xlsx => xlsx::of(&tables(&found.markdown))?,
        Format::Csv => csv_of(&tables(&found.markdown))?.into_bytes(),
        Format::Text => unreachable!("answered above"),
    })
}

fn whole(markdown: &str, format: Format) -> Option<Vec<u8>> {
    Some(match format {
        Format::Markdown => markdown.to_owned().into_bytes(),
        Format::Html => page(&render::html(markdown)).into_bytes(),
        Format::Text => render::plain(markdown).into_bytes(),
        Format::Docx => docx::of(markdown),
        Format::Xlsx => xlsx::of(&tables(markdown))?,
        Format::Csv => csv_of(&tables(markdown))?.into_bytes(),
    })
}

/// A block's HTML as a page rather than as a fragment.
///
/// A bare fragment saved as `.html` is a file that opens as text in some
/// viewers and as mojibake in others, because nothing in it says what it is.
/// The charset matters here for the same reason the byte-order mark does for a
/// spreadsheet, and it is one line.
fn page(body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"zh\">\n<head>\n<meta charset=\"utf-8\">\n\
         <title>DocFusion</title>\n</head>\n<body>\n{body}\n</body>\n</html>\n"
    )
}

/// Every table in a fragment, as rows of cells.
///
/// Read from the Markdown and not from the parsed tree, because the Markdown is
/// what this program wrote and what it can read back without guessing — and the
/// cells are already the document's own words, which is all a spreadsheet wants.
///
/// A fragment is a block's Markdown or a document's, and the answer is the same
/// either way: a block is one table, a document is however many it holds, and
/// the writers below do not have to care which they were handed.
fn tables(markdown: &str) -> Vec<Table> {
    render::blocks(markdown)
        .into_iter()
        .filter(|block| block.table)
        .map(|block| rows(&block.markdown))
        .collect()
}

/// One table's rows, without the bars and without the padding.
fn rows(markdown: &str) -> Table {
    let mut table = Table::new();

    for line in markdown.lines() {
        // The rule under the header is Markdown's, and has no cell in it.
        let trimmed = line.trim();
        if !trimmed.starts_with('|') || is_rule(trimmed) {
            continue;
        }

        table.push(cells(trimmed));
    }

    table
}

/// Tables as comma separated values, one after another.
///
/// A document is not a table, so asking for one as CSV is asking for the tables
/// in it — which is a thing somebody wants (a report whose numbers are all in
/// tables) and is the only reading of the request that is not nonsense. The
/// blank line between them is what stops two tables becoming one.
fn csv_of(tables: &[Table]) -> Option<String> {
    (!tables.is_empty()).then(|| {
        tables
            .iter()
            .map(csv)
            .collect::<Vec<_>>()
            .join("\r\n")
    })
}

/// One table as comma separated values.
fn csv(table: &Table) -> String {
    let mut out = String::new();

    for row in table {
        let fields: Vec<String> = row.iter().map(|cell| field(cell)).collect();

        out.push_str(&fields.join(","));
        out.push_str("\r\n");
    }

    out
}

/// Whether a table line is the `|---|---|` under the header.
fn is_rule(line: &str) -> bool {
    line.chars()
        .all(|character| matches!(character, '|' | '-' | ':' | ' '))
}

/// The cells of one row, without the bars and without the padding.
///
/// **Split on the bars that are bars.** A cell holding a bar is written `\|`
/// — that is how Markdown escapes one — so splitting on every bar splits in the
/// middle of a value, and a table whose cells hold a bar is a table somebody
/// has already had trouble with.
fn cells(line: &str) -> Vec<String> {
    let inner = line.trim().trim_matches('|');
    let mut cells = vec![String::new()];
    let mut characters = inner.chars().peekable();

    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.next() {
                Some('|') => last(&mut cells).push('|'),
                Some(other) => last(&mut cells).push(other),
                None => last(&mut cells).push('\\'),
            },
            '|' => cells.push(String::new()),
            _ => last(&mut cells).push(character),
        }
    }

    cells.into_iter().map(|cell| cell.trim().to_owned()).collect()
}

/// The cell being written into, which is always there: one is pushed before
/// anything is read.
fn last(cells: &mut [String]) -> &mut String {
    cells.last_mut().expect("one, pushed above")
}

/// One value, quoted if it has to be.
///
/// RFC 4180's rule, and it is the rule rather than a preference because the
/// alternative is a file that opens with the columns in the wrong places.
fn field(value: &str) -> String {
    if value.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of an export, for the formats that are text. A binary one has no
    /// business being read as a string and would fail here rather than quietly.
    fn text(markdown: &str, format: Format, block: Option<usize>) -> Option<String> {
        write(markdown, format, block)
            .map(|bytes| String::from_utf8(bytes).expect("a text format writes text"))
    }

    const DOCUMENT: &str = "# 标题\n\n一段话。\n\n| 名称 | 数量 |\n|---|---|\n| 甲方 | 12 |\n| 乙方 | 3 |\n";

    #[test]
    fn markdown_of_the_whole_document_is_the_document() {
        assert_eq!(text(DOCUMENT, Format::Markdown, None).unwrap(), DOCUMENT);
    }

    /// The document's Markdown is the file as it is, front matter and all —
    /// which is what the caller passes in and what this must not tidy up.
    #[test]
    fn markdown_of_the_whole_document_keeps_the_front_matter() {
        let with_front = format!("---\ntitle: Report\n---\n{DOCUMENT}");

        assert!(text(&with_front, Format::Markdown, None).unwrap().contains("title: Report"));
    }

    #[test]
    fn html_is_a_page_rather_than_a_fragment() {
        let html = text(DOCUMENT, Format::Html, None).unwrap();

        assert!(html.starts_with("<!doctype html>"), "{html}");
        assert!(html.contains("charset=\"utf-8\""), "no charset, so a browser guesses: {html}");
        assert!(html.contains("<table>"));
    }

    /// The words, with a table flattened into what a table flattened into text
    /// is: rows on their own lines, cells in columns, and **the cells apart**.
    /// Joining them by the rule for words would run 名称 and 数量 together, which
    /// reads as one word and is two.
    #[test]
    fn text_is_the_words_and_a_table_is_its_cells_apart() {
        assert_eq!(
            text(DOCUMENT, Format::Text, None).unwrap(),
            "标题\n一段话。\n名称\t数量\n甲方\t12\n乙方\t3"
        );
    }

    // ── one block ────────────────────────────────────────────────────────────

    #[test]
    fn a_block_comes_out_as_itself() {
        let heading = text(DOCUMENT, Format::Markdown, Some(0)).unwrap();
        assert_eq!(heading.trim(), "# 标题");

        let table = text(DOCUMENT, Format::Markdown, Some(2)).unwrap();
        assert!(table.contains("| 名称 | 数量 |"), "{table}");
    }

    /// A table exported as CSV is a table — which is the whole reason somebody
    /// presses export on one rather than copy.
    #[test]
    fn a_table_comes_out_as_a_spreadsheet() {
        let csv = text(DOCUMENT, Format::Csv, Some(2)).unwrap();

        assert_eq!(csv, "名称,数量\r\n甲方,12\r\n乙方,3\r\n");
    }

    /// The header's rule is Markdown's and is not a row of the table.
    #[test]
    fn the_rule_under_a_header_is_not_a_row() {
        let csv = text(DOCUMENT, Format::Csv, Some(2)).unwrap();

        assert!(!csv.contains("---"), "{csv}");
        assert_eq!(csv.lines().count(), 3);
    }

    #[test]
    fn a_value_with_a_comma_in_it_is_quoted() {
        let markdown = "| a |\n|---|\n| one, two |\n| say \"hi\" |\n";

        let csv = text(markdown, Format::Csv, Some(0)).unwrap();

        assert!(csv.contains("\"one, two\""), "{csv}");
        assert!(csv.contains("\"say \"\"hi\"\"\""), "{csv}");
    }

    /// **A format that does not suit the block is refused, not approximated.**
    /// A paragraph written as comma separated values is a paragraph with quotes
    /// round it, and writing that file is worse than writing none.
    #[test]
    fn a_format_that_does_not_suit_the_block_is_refused() {
        for format in [Format::Csv, Format::Xlsx] {
            assert!(write(DOCUMENT, format, Some(0)).is_none(), "a heading as {format}");
            assert!(write(DOCUMENT, format, Some(1)).is_none(), "a paragraph as {format}");
        }

        assert!(
            text(DOCUMENT, Format::Text, Some(0)).is_none(),
            "a block as plain text is what the copy button is for"
        );
    }

    // ── the two that are files rather than text ──────────────────────────────

    /// A table can come out as a workbook and a document as a Word file — the
    /// two things somebody who has just had a filing read actually wants, and
    /// the whole reason these formats are here.
    ///
    /// `PK` is the first two bytes of a zip, which is what both of them are.
    #[test]
    fn a_table_can_be_a_workbook_and_a_document_can_be_a_word_file() {
        let workbook = write(DOCUMENT, Format::Xlsx, Some(2)).expect("a table as a workbook");
        assert_eq!(workbook.get(..2), Some(b"PK".as_slice()), "a workbook is a zip");

        let word = write(DOCUMENT, Format::Docx, None).expect("a document as a Word file");
        assert_eq!(word.get(..2), Some(b"PK".as_slice()), "a Word file is a zip");
    }

    /// A Word file is not a table's business and a workbook is not a
    /// paragraph's, which is the same rule the CSV above follows.
    #[test]
    fn a_document_of_tables_is_a_workbook_too_and_a_document_without_is_none() {
        let two = "# A\n\n| a |\n|---|\n| 1 |\n\n# B\n\n| b |\n|---|\n| 2 |\n";

        assert!(write(two, Format::Xlsx, None).is_some());
        assert!(write("# A\n\njust words\n", Format::Xlsx, None).is_none());

        // And the Word file is the one that does not care.
        assert!(write("# A\n\njust words\n", Format::Docx, None).is_some());
    }

    /// **Read back by the readers this program already has.**
    ///
    /// A `.docx` and an `.xlsx` are packages with a shape, and "the XML looks
    /// right" is not the same as "a reader opens it" — the two are exactly the
    /// kinds of file where being nearly right produces something that is not a
    /// document at all. So what is written here goes to the same parsers the
    /// pipeline uses on somebody else's Word file and Excel workbook: the two
    /// halves of one question, which is whether what this program writes is
    /// what it reads.
    #[test]
    fn what_this_program_writes_is_what_this_program_reads() {
        let word = read_back(
            write(DOCUMENT, Format::Docx, None).expect("a Word file"),
            "fixture.docx",
            &crate::extract::docx::DocxParser,
        );
        assert!(word.contains("标题"), "{word}");
        assert!(word.contains("甲方"), "{word}");

        // The table half, through the workbook: the cells are where they were.
        let workbook = read_back(
            write(DOCUMENT, Format::Xlsx, Some(2)).expect("a workbook"),
            "fixture.xlsx",
            &crate::extract::xlsx::XlsxParser,
        );
        assert!(workbook.contains("名称"), "{workbook}");
        assert!(workbook.contains("12"), "{workbook}");
    }

    fn read_back(bytes: Vec<u8>, name: &str, parser: &dyn crate::extract::Extractor) -> String {
        let probe =
            crate::extract::InputProbe::new(&bytes, Some(name.to_owned()), bytes.len() as u64);
        let source = crate::ir::SourceRef {
            path: std::path::PathBuf::from(name),
            content_hash: "0".repeat(64),
            mime: "application/zip".into(),
            size: bytes.len() as u64,
            mtime: None,
        };

        let document = parser
            .extract(&crate::extract::Input {
                probe: &probe,
                bytes: &bytes,
                source: &source,
            })
            .expect("the file this program just wrote");

        document
            .blocks()
            .map(|block| block.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// **The two Office formats are files and cannot be printed.**
    ///
    /// They are zips: on a terminal that is a screenful of noise, and piped into
    /// something expecting text it is nothing to read at all. Which formats are
    /// which is one question with one answer, asked here so that the two front
    /// ends do not each have to know.
    #[test]
    fn the_office_formats_are_files_and_the_rest_are_not() {
        assert!(Format::Docx.binary() && Format::Xlsx.binary());

        for format in [Format::Markdown, Format::Html, Format::Text, Format::Csv] {
            assert!(!format.binary(), "{format} is text");
        }
    }

    #[test]
    fn a_block_that_is_not_there_is_refused() {
        assert!(text(DOCUMENT, Format::Markdown, Some(99)).is_none());
    }

    // ── the document as a table ──────────────────────────────────────────────

    /// Asking for a document as CSV asks for the tables in it, and a document
    /// with none is not a table with no rows — it is a request that cannot be
    /// answered.
    #[test]
    fn a_document_of_tables_is_all_of_them_and_a_document_without_is_none() {
        let two = "# A\n\n| a |\n|---|\n| 1 |\n\n# B\n\n| b |\n|---|\n| 2 |\n";
        let both = text(two, Format::Csv, None).unwrap();

        assert!(both.contains("a\r\n1"), "{both}");
        assert!(both.contains("b\r\n2"), "{both}");
        assert!(both.lines().filter(|line| line.is_empty()).count() >= 1, "no gap between them");

        assert!(text("# A\n\njust words\n", Format::Csv, None).is_none());
    }

    // ── the file's first bytes ───────────────────────────────────────────────

    /// Excel reads a `.csv` as the legacy code page unless a byte-order mark
    /// says otherwise, and a table of Chinese comes up as mojibake —
    /// the most common way an export that worked produces a file nobody can
    /// read.
    ///
    /// **And a mark in front of a zip is not a zip.** The two Office formats are
    /// the reason this rule is "the comma separated one" and not "the
    /// spreadsheets": `.xlsx` is a spreadsheet and three bytes at the front of it
    /// are three bytes Excel will not open the file past.
    #[test]
    fn only_the_comma_separated_one_wants_a_mark() {
        assert!(wants_a_mark(Format::Csv));

        for format in [Format::Markdown, Format::Text, Format::Html, Format::Docx, Format::Xlsx] {
            assert!(!wants_a_mark(format), "{format} does not want one");
        }
    }

    #[test]
    fn the_format_can_be_named_and_read_back() {
        for format in Format::ALL {
            assert_eq!(Format::parse(format.as_str()), Some(*format));
        }

        assert_eq!(Format::parse(" HTML "), Some(Format::Html));
        // The one somebody will try that this program does not write: a PDF
        // needs a font embedded and a page laid out, which is a different
        // program from a reader of documents.
        assert_eq!(Format::parse("pdf"), None);
    }
}
