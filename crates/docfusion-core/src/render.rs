//! Rendering the IR as Markdown.
//!
//! Extracted text is plain text that was never meant to be Markdown. A
//! paragraph that happens to begin with `#` or `-` would be reinterpreted as
//! structure the document never had, so text is escaped before it is written.

use crate::ir::{Block, BlockKind, ConflictKind, Document};
use crate::text::{is_cjk, needs_space};

/// Characters that change the meaning of a line when they begin one.
const BLOCK_MARKERS: [char; 8] = ['#', '>', '-', '+', '*', '=', '~', '`'];

/// Upper bound on a slug, leaving room for a directory and an extension inside
/// the Windows path limit.
const MAX_SLUG_LENGTH: usize = 100;

/// Names Windows refuses to use for a file, whatever the extension.
const RESERVED_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Renders Markdown as HTML, for a viewer.
///
/// The other half of the renderer: the command line writes Markdown because a
/// knowledge base is files, and a window shows HTML because a webview is a
/// browser. Both start from the same text and neither owns the document.
///
/// **Raw HTML in the source is escaped rather than passed through.** Extracted
/// text is not trusted markup — it is whatever was on a page, which for a
/// hostile document is whatever its author put there — and this output is
/// handed to a webview, where passed-through markup is script. `comrak`'s
/// default is to escape it, and the default is right.
///
/// The front matter is dropped rather than shown. It is a machine's record of
/// where the document came from, not something the document says.
pub fn html(markdown: &str) -> String {
    let mut options = comrak::Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.front_matter_delimiter = Some("---".to_owned());
    // Raw HTML in the source is escaped rather than passed through. This is
    // `comrak`'s default, said out loud because it is the one setting here that
    // is a security property rather than a preference: the output goes into a
    // webview, where markup that passes through is script that runs.
    options.render.escape = true;

    comrak::markdown_to_html(markdown, &options)
}

/// Renders a document as Markdown with YAML front matter.
///
/// `created_at` is supplied rather than read from the clock, because the
/// renderer must produce the same bytes for the same input — otherwise a
/// re-render would look like a change to anything comparing the output.
pub fn markdown(document: &Document, created_at: &str) -> String {
    let mut markdown = frontmatter(document, created_at);

    let mut previous: Option<BlockKind> = None;
    for page in &document.pages {
        for block in &page.blocks {
            if let Some(previous) = &previous
                && !adjacent_within_one_block(previous, &block.kind)
            {
                markdown.push('\n');
            }
            previous = Some(block.kind.clone());
            block_markdown(&mut markdown, block);
        }
    }

    conflicts_markdown(&mut markdown, document);
    markdown
}

/// Records the engines' disagreements at the foot of the document.
///
/// In the artifact rather than in a log beside it, because the artifact is what
/// gets read, indexed and trusted: a specialized parser fails loudly and a
/// general model fails silently, so a disagreement is the one thing the
/// pipeline must not lose. A log is a file nobody opens.
///
/// Each reading is shown verbatim, in an indented block so that nothing in the
/// extracted text is read as Markdown structure. What is deliberately absent is
/// any suggestion of which reading is right — the tool has no basis for
/// preferring one, and a report that quietly picked would be the silent
/// resolution this exists to prevent.
fn conflicts_markdown(out: &mut String, document: &Document) {
    if document.conflicts.is_empty() {
        return;
    }

    out.push_str("\n## Conflicts\n");

    for conflict in &document.conflicts {
        out.push_str(&format!(
            "\nPage {}: {}.\n\n",
            conflict.page + 1,
            describe(&conflict.kind)
        ));

        for variant in &conflict.variants {
            // The engine, and what it read the region as, on one line: it is
            // the same sentence. `rapidocr` read this, and read it as running
            // text and nothing else.
            out.push_str(&format!(
                "    {} — headings {}, lists {}, tables {}, text {}\n",
                variant.engine.as_str(),
                variant.shapes.headings,
                variant.shapes.lists,
                variant.shapes.tables,
                variant.shapes.text,
            ));
            for line in variant.text.lines() {
                out.push_str("    ");
                out.push_str(line);
                out.push('\n');
            }
            out.push('\n');
        }
    }
}

fn describe(kind: &ConflictKind) -> &'static str {
    match kind {
        ConflictKind::TextMismatch => "the engines read it differently",
        ConflictKind::StructuralMismatch => "one engine found text the other did not",
    }
}

/// How much of the document a second engine checked, as `checked of total`.
///
/// A page nobody checked and a page that passed are different things, and the
/// ceiling on verification means a long scan can be partly one and partly the
/// other. Reporting only the conflicts would leave the unchecked pages looking
/// as good as the verified ones.
fn verification(document: &Document) -> String {
    let verified = document
        .pages
        .iter()
        .filter(|page| page.provenance.verified)
        .count();

    format!("{verified}/{}", document.pages.len())
}

/// Whether two neighbouring blocks belong to the same construct, and so must
/// not be separated by a blank line.
///
/// Adjacent list items are one list. A blank line between them makes it a
/// *loose* list, which a reader renders with each item wrapped in a paragraph —
/// the same words, presented as something else. Every other pair is genuinely
/// separate and wants the blank line, which is what stops two paragraphs from
/// running together.
fn adjacent_within_one_block(previous: &BlockKind, current: &BlockKind) -> bool {
    matches!(
        (previous, current),
        (BlockKind::ListItem, BlockKind::ListItem)
    )
}

/// No page separators: a knowledge base is read as continuous prose, and a
/// horizontal rule between every page of a long PDF is noise. Page structure
/// stays available in the IR for anything that needs it.
fn frontmatter(document: &Document, created_at: &str) -> String {
    let mut out = String::from("---\n");

    field(&mut out, "source", &document.source.path.to_string_lossy());
    field(&mut out, "sha256", &document.source.content_hash);
    field(&mut out, "title", &title(document));
    field(&mut out, "mime", &document.source.mime);
    field(&mut out, "created_at", created_at);

    let engines = engines(document);
    out.push_str("engines: [");
    for (index, engine) in engines.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&yaml_scalar(engine));
    }
    out.push_str("]\n");

    field(&mut out, "verified", &verification(document));

    field(
        &mut out,
        "conflict_count",
        &document.conflicts.len().to_string(),
    );

    out.push_str("---\n");
    out
}

fn field(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(": ");
    out.push_str(&yaml_scalar(value));
    out.push('\n');
}

/// A document's title, from metadata if it has any.
///
/// The document's own first heading is preferred over the source file name,
/// because "2024 Annual Report" is what the document calls itself while
/// `report.pdf` is merely what somebody saved it as — and the title is also what
/// names the output file.
pub fn title(document: &Document) -> String {
    if let Some(given) = document.meta.title.as_ref().filter(|t| !t.trim().is_empty()) {
        return given.clone();
    }

    for block in document.blocks() {
        if matches!(block.kind, BlockKind::Heading { .. }) && !block.text.trim().is_empty() {
            return block.text.trim().to_owned();
        }
    }

    if let Some(stem) = document.source.path.file_stem() {
        return stem.to_string_lossy().into_owned();
    }

    String::new()
}

fn engines(document: &Document) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for page in &document.pages {
        for engine in &page.provenance.engines {
            let name = engine.to_string();
            if !found.contains(&name) {
                found.push(name);
            }
        }
    }
    found
}

fn block_markdown(out: &mut String, block: &Block) {
    match block.kind {
        BlockKind::Heading { level } => {
            for _ in 0..level.clamp(1, 6) {
                out.push('#');
            }
            out.push(' ');
            // One line, however many it ran to on the page. Markdown ends a
            // heading at the first newline, so a heading whose words ran onto a
            // second line is written as a heading followed by a paragraph: the
            // second half is not part of the heading any more and matches
            // nothing. Measured on a three-column Security and Exchange
            // Commission notice — seven headings, every one of them two lines,
            // and not one of them found. The break is where the column ran out,
            // not where the heading ends.
            out.push_str(&escape(&one_line(&block.text)));
            out.push('\n');
        }
        BlockKind::Paragraph => {
            out.push_str(&escape(&unbroken(&block.text)));
            out.push('\n');
        }
        BlockKind::ListItem => list_item_markdown(out, block, 0),
        BlockKind::Quote => {
            for (index, line) in unbroken(&block.text).lines().enumerate() {
                if index > 0 {
                    out.push('\n');
                }
                out.push_str("> ");
                out.push_str(&escape_line(line));
            }
            out.push('\n');
            for child in &block.children {
                block_markdown(out, child);
            }
        }
        BlockKind::Table => table_markdown(out, block),
        BlockKind::Code => {
            out.push_str("```\n");
            out.push_str(&block.text);
            out.push_str("\n```\n");
        }
        BlockKind::Formula => {
            out.push_str("$$\n");
            out.push_str(&block.text);
            out.push_str("\n$$\n");
        }
        // Figures carry a caption until asset extraction lands; an image with
        // no path would be broken Markdown.
        BlockKind::Figure => {
            if !block.text.trim().is_empty() {
                out.push_str(&escape(&block.text));
                out.push('\n');
            }
        }
        // Rendered by the table that owns them.
        BlockKind::TableRow | BlockKind::TableCell => {}
    }
}

/// Renders a list item and, indented beneath it, its sub-items.
///
/// Nesting is carried by the IR rather than by a depth field, so indentation is
/// simply how deep the recursion is.
fn list_item_markdown(out: &mut String, item: &Block, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str("- ");
    out.push_str(&escape(&item.text));
    out.push('\n');

    for child in &item.children {
        list_item_markdown(out, child, depth + 1);
    }
}

fn table_markdown(out: &mut String, table: &Block) {
    let rows: Vec<&Block> = table
        .children
        .iter()
        .filter(|child| child.kind == BlockKind::TableRow)
        .collect();

    if rows.is_empty() {
        return;
    }

    // GFM requires every row to have the same number of columns, so the grid is
    // as wide as its widest row and short rows are padded.
    let columns = rows
        .iter()
        .map(|row| row.children.len())
        .max()
        .unwrap_or(0);
    if columns == 0 {
        return;
    }

    table_row(out, rows[0], columns);
    out.push('|');
    for _ in 0..columns {
        out.push_str(" --- |");
    }
    out.push('\n');

    for row in &rows[1..] {
        table_row(out, row, columns);
    }
}

fn table_row(out: &mut String, row: &Block, columns: usize) {
    out.push('|');
    for index in 0..columns {
        let text = row
            .children
            .get(index)
            .map(|cell| cell.text.as_str())
            .unwrap_or_default();
        out.push(' ');
        out.push_str(&escape_cell(text));
        out.push_str(" |");
    }
    out.push('\n');
}

/// Escapes text so it renders as written rather than as Markdown structure.
///
/// Only characters that would change the meaning of a line are escaped, and
/// only at the start of one. Escaping inline punctuation too would turn
/// ordinary text such as `5 * 3` into `5 \* 3` — noise that changes nothing,
/// because an unpaired `*` already renders literally.
/// The words of a block as one line, with the break between each pair of them
/// closed up.
///
/// Closed with a space between two Latin words and with nothing between two
/// Chinese ones: a break in a Chinese line is where the column ran out, and a
/// space written there is a space the page does not have. Measured on a
/// newspaper page, five of its ten headings were missed for that and nothing
/// else — `洛阳市抗旱从种子抓起` written as `洛阳市抗旱 从种子抓起`.
fn one_line(text: &str) -> String {
    let mut out = String::new();

    for line in text.split('\n').map(str::trim).filter(|line| !line.is_empty()) {
        if needs_space(&out, line) {
            out.push(' ');
        }
        out.push_str(line);
    }

    out
}

/// A block of prose with the line breaks a Chinese line should not show.
///
/// The same fault `one_line` fixes for a heading, and it cannot be fixed the
/// same way here: a paragraph's line breaks are worth keeping, because between
/// two Latin words the break **is** a space and where they are is where the page
/// put them. So only the break between two Chinese characters goes, and the rest
/// stay where the page had them.
///
/// What it is for: Markdown turns a newline inside a paragraph into a space when
/// it is rendered, so a Chinese paragraph arrives in the window with a space at
/// every place the column ran out — spaces the document does not have.
fn unbroken(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());

    for (at, character) in characters.iter().enumerate() {
        if character == &'\n' {
            let left = at.checked_sub(1).and_then(|before| characters.get(before));
            let right = characters.get(at + 1);

            if let (Some(left), Some(right)) = (left, right)
                && is_cjk(*left)
                && is_cjk(*right)
            {
                continue;
            }
        }

        out.push(*character);
    }

    out
}

fn escape(text: &str) -> String {
    text.split('\n')
        .map(escape_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn escape_line(line: &str) -> String {
    let Some(first) = line.chars().next() else {
        return String::new();
    };

    if BLOCK_MARKERS.contains(&first) {
        return format!("\\{line}");
    }

    // A leading number followed by `.` or `)` opens an ordered list, which is
    // almost always a false positive for prose such as "2024. Revenue grew".
    // The period is escaped rather than the digit, because a backslash only
    // escapes punctuation.
    if first.is_ascii_digit() {
        let digits = line.chars().take_while(char::is_ascii_digit).count();
        let rest: Vec<char> = line.chars().skip(digits).collect();

        if matches!(rest.first(), Some('.') | Some(')'))
            && rest.get(1).is_none_or(|next| next.is_whitespace())
        {
            return format!("{}\\{}", &line[..digits], &line[digits..]);
        }
    }

    line.to_owned()
}

/// A pipe ends a cell and a newline ends a row, so both must go. GFM has no way
/// to express a line break inside a cell, so it becomes a `<br>`.
fn escape_cell(text: &str) -> String {
    escape(text).replace('|', "\\|").replace('\n', "<br>")
}

/// Quotes a YAML scalar when leaving it bare could change its meaning.
///
/// Deliberately conservative. YAML's plain-scalar rules are subtle — a colon
/// only introduces a mapping when followed by a space, a `#` only starts a
/// comment after one — and quoting something that did not need it costs a pair
/// of quote marks, while missing one costs a field that a parser reads as
/// something else. A quoted value also keeps a stable type: a bare timestamp
/// may be read back as a date rather than the string it was written as.
fn yaml_scalar(value: &str) -> String {
    let needs_quotes = value.is_empty()
        || value
            .chars()
            .next()
            .is_some_and(|first| first.is_whitespace())
        || value.chars().last().is_some_and(|last| last.is_whitespace())
        || value.contains([
            ':', '#', '"', '\'', '\n', '[', ']', '{', '}', ',', '&', '*', '!', '|', '>', '%', '@',
            '`',
        ]);

    if needs_quotes {
        format!(
            "\"{}\"",
            value.replace('\\', "\\\\").replace('"', "\\\"")
        )
    } else {
        value.to_owned()
    }
}

/// Builds a filename-safe slug from a title.
///
/// Only alphanumeric characters survive; everything else — spaces, punctuation,
/// path separators — collapses into a single `-`. Deriving safety from an
/// allow-list rather than by enumerating the characters Windows happens to
/// forbid today also fixes a subtler problem: a dash is legal in a filename but
/// is a word separator here, so treating it as content would leave `Q1 - 2024`
/// as `q1---2024`.
///
/// CJK characters are kept rather than transliterated. They are alphanumeric
/// and legal in filenames, and transliterating would make a Chinese title
/// unrecognisable to the person who wrote it.
///
/// Collisions are not resolved here. Two documents with the same title produce
/// the same slug, and it is the pipeline that decides to disambiguate, because
/// only it knows whether the existing file is the same document.
pub fn slug(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut separator_wanted = false;

    for character in title.chars() {
        if character.is_alphanumeric() {
            if separator_wanted && !slug.is_empty() {
                slug.push('-');
            }
            separator_wanted = false;
            slug.push(character.to_ascii_lowercase());
        } else {
            // Runs of separators collapse, and leading or trailing ones produce
            // nothing at all.
            separator_wanted = true;
        }
    }

    let slug: String = slug.chars().take(MAX_SLUG_LENGTH).collect();
    let slug = slug.trim_end_matches('-').to_owned();

    if slug.is_empty() {
        return "untitled".to_owned();
    }

    // A reserved name is reserved whatever the extension, so it is prefixed
    // rather than rejected.
    if RESERVED_NAMES.contains(&slug.as_str()) {
        return format!("_{slug}");
    }

    slug
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Conflict, DocMeta, EngineId, Page, PageProvenance, Shapes, SourceRef, Variant};
    use comrak::nodes::NodeValue;
    use comrak::{Arena, Options, parse_document};
    use std::path::PathBuf;

    const CREATED_AT: &str = "2026-10-08T12:00:00Z";

    fn block(kind: BlockKind, text: &str) -> Block {
        Block::text(kind, text, EngineId::from("test"))
    }

    fn document(blocks: Vec<Block>) -> Document {
        Document {
            source: SourceRef {
                path: PathBuf::from("report.pdf"),
                content_hash: "a".repeat(64),
                mime: "application/pdf".into(),
                size: 10,
                mtime: None,
            },
            pages: vec![Page {
                index: 0,
                size: None,
                blocks,
                provenance: PageProvenance {
                    primary: EngineId::from("pdfium"),
                    engines: vec![EngineId::from("pdfium")],
                    verified: false,
                },
            }],
            meta: DocMeta::default(),
            conflicts: Vec::new(),
        }
    }

    fn disagreement(kind: ConflictKind, readings: &[(&str, &str)]) -> Conflict {
        Conflict {
            page: 0,
            bbox: None,
            kind,
            variants: readings
                .iter()
                .map(|(engine, text)| Variant {
                    engine: EngineId::from(*engine),
                    text: (*text).to_owned(),
                    // One paragraph, which is what a reading of a few words by
                    // an engine that has no other kind of block is.
                    shapes: Shapes::of(&[Block::text(
                        BlockKind::Paragraph,
                        *text,
                        EngineId::from(*engine),
                    )]),
                })
                .collect(),
            resolved: None,
        }
    }

    fn rendered(blocks: Vec<Block>) -> String {
        markdown(&document(blocks), CREATED_AT)
    }

    fn body(markdown: &str) -> &str {
        markdown
            .split_once("\n---\n")
            .expect("front matter")
            .1
            .trim_start_matches('\n')
    }

    #[test]
    fn renders_headings_at_their_level() {
        let markdown = rendered(vec![
            block(BlockKind::Heading { level: 1 }, "Title"),
            block(BlockKind::Heading { level: 3 }, "Section"),
        ]);

        assert!(body(&markdown).contains("# Title"));
        assert!(body(&markdown).contains("### Section"));
    }

    #[test]
    fn heading_levels_above_six_are_clamped() {
        let markdown = rendered(vec![block(BlockKind::Heading { level: 9 }, "Deep")]);
        assert!(body(&markdown).contains("###### Deep"));
        assert!(!body(&markdown).contains("#######"));
    }

    /// Extracted text is not Markdown. A paragraph that happens to start with
    /// `#` must not become a heading.
    #[test]
    fn a_paragraph_beginning_with_a_hash_is_escaped() {
        let markdown = rendered(vec![block(BlockKind::Paragraph, "# not a heading")]);
        assert!(body(&markdown).contains("\\# not a heading"));
    }

    #[test]
    fn a_paragraph_beginning_with_a_dash_is_escaped() {
        let markdown = rendered(vec![block(BlockKind::Paragraph, "- not a list")]);
        assert!(body(&markdown).contains("\\- not a list"));
    }

    /// "2024. Revenue grew" would otherwise open an ordered list.
    #[test]
    fn a_paragraph_beginning_with_a_number_is_escaped() {
        let markdown = rendered(vec![block(BlockKind::Paragraph, "2024. Revenue grew")]);
        assert!(body(&markdown).contains("2024\\. Revenue grew"));
    }

    #[test]
    fn a_number_without_a_separator_is_left_alone() {
        assert_eq!(escape_line("2024 was a good year"), "2024 was a good year");
        assert_eq!(escape_line("version 2.5 released"), "version 2.5 released");
    }

    /// Only line starts are escaped: an unpaired `*` already renders literally,
    /// so escaping it would only add noise.
    #[test]
    fn inline_punctuation_is_left_alone() {
        assert_eq!(escape_line("5 * 3 = 15"), "5 * 3 = 15");
    }

    #[test]
    fn list_items_are_rendered_as_bullets() {
        let markdown = rendered(vec![block(BlockKind::ListItem, "First")]);
        assert!(body(&markdown).contains("- First"));
    }

    #[test]
    fn a_table_becomes_a_grid_with_a_delimiter_row() {
        let mut table = block(BlockKind::Table, "");
        for cells in [["A1", "B1"], ["A2", "B2"]] {
            table.children.push(Block {
                kind: BlockKind::TableRow,
                text: String::new(),
                bbox: None,
                confidence: None,
                engine: EngineId::from("test"),
                children: cells
                    .iter()
                    .map(|cell| block(BlockKind::TableCell, cell))
                    .collect(),
            });
        }

        let markdown = rendered(vec![table]);
        let table_markdown: Vec<&str> = body(&markdown)
            .lines()
            .filter(|line| line.starts_with('|'))
            .collect();

        assert_eq!(table_markdown.len(), 3, "header, delimiter and one body row");
        assert_eq!(table_markdown[0], "| A1 | B1 |");
        assert_eq!(table_markdown[1], "| --- | --- |");
        assert_eq!(table_markdown[2], "| A2 | B2 |");
    }

    /// A short row would otherwise shift every later cell one column left.
    #[test]
    fn short_rows_are_padded_to_the_widest_row() {
        let mut table = block(BlockKind::Table, "");
        for cells in [&["A", "B", "C"][..], &["D"][..]] {
            table.children.push(Block {
                kind: BlockKind::TableRow,
                text: String::new(),
                bbox: None,
                confidence: None,
                engine: EngineId::from("test"),
                children: cells
                    .iter()
                    .map(|cell| block(BlockKind::TableCell, cell))
                    .collect(),
            });
        }

        let markdown = rendered(vec![table]);
        assert!(
            body(&markdown).lines().any(|line| line == "| D |  |  |"),
            "{}",
            body(&markdown)
        );
    }

    #[test]
    fn a_pipe_inside_a_cell_is_escaped() {
        let mut table = block(BlockKind::Table, "");
        table.children.push(Block {
            kind: BlockKind::TableRow,
            text: String::new(),
            bbox: None,
            confidence: None,
            engine: EngineId::from("test"),
            children: vec![block(BlockKind::TableCell, "a|b")],
        });

        let markdown = rendered(vec![table]);
        assert!(body(&markdown).contains("| a\\|b |"), "{}", body(&markdown));
    }

    #[test]
    fn frontmatter_records_provenance() {
        let markdown = rendered(vec![block(BlockKind::Paragraph, "x")]);

        assert!(markdown.starts_with("---\n"));
        assert!(markdown.contains("source: report.pdf"));
        assert!(markdown.contains(&format!("sha256: {}", "a".repeat(64))));
        assert!(markdown.contains("mime: application/pdf"));
        // Quoted deliberately: see `yaml_scalar`.
        assert!(markdown.contains(&format!("created_at: \"{CREATED_AT}\"")));
        assert!(markdown.contains("engines: [pdfium]"));
        assert!(markdown.contains("conflict_count: 0"));
    }

    // ── Conflicts ────────────────────────────────────────────────────────────

    fn with_conflicts(conflicts: Vec<Conflict>) -> Document {
        let mut doc = document(vec![block(BlockKind::Paragraph, "Body")]);
        doc.conflicts = conflicts;
        doc
    }

    #[test]
    fn nothing_is_written_when_the_engines_agreed() {
        let markdown = rendered(vec![block(BlockKind::Paragraph, "Body")]);

        assert!(!markdown.contains("## Conflicts"));
    }

    /// A disagreement that is not in the artifact is a disagreement nobody
    /// sees, and the artifact is what gets indexed and trusted.
    #[test]
    fn a_disagreement_is_written_into_the_document() {
        let doc = with_conflicts(vec![disagreement(
            ConflictKind::TextMismatch,
            &[("vision", "Revenue increased by 13%"), ("ocr", "Revenue increased by 12%")],
        )]);

        let markdown = markdown(&doc, CREATED_AT);

        assert!(markdown.contains("conflict_count: 1"));
        assert!(markdown.contains("## Conflicts"), "{markdown}");
        assert!(markdown.contains("Page 1: the engines read it differently."));
        // Both readings, verbatim.
        assert!(markdown.contains("Revenue increased by 13%"));
        assert!(markdown.contains("Revenue increased by 12%"));
        assert!(markdown.contains("vision"));
        assert!(markdown.contains("ocr"));

        // And what each of them read it as, which the text cannot say: the same
        // words are a row of a table from one engine and a line of prose from
        // the other, and there is nothing else on the page to tell them apart.
        assert!(
            markdown.contains("vision — headings 0, lists 0, tables 0, text 1"),
            "{markdown}"
        );
    }

    #[test]
    fn a_dropped_region_is_described_as_one() {
        let doc = with_conflicts(vec![disagreement(
            ConflictKind::StructuralMismatch,
            &[("ocr", "Signature")],
        )]);

        let markdown = markdown(&doc, CREATED_AT);
        assert!(
            markdown.contains("Page 1: one engine found text the other did not."),
            "{markdown}"
        );
    }

    #[test]
    fn conflicts_are_numbered_from_one() {
        let mut second = disagreement(ConflictKind::TextMismatch, &[("a", "x"), ("b", "y")]);
        second.page = 2;
        let doc = with_conflicts(vec![
            disagreement(ConflictKind::TextMismatch, &[("a", "x"), ("b", "y")]),
            second,
        ]);

        let markdown = markdown(&doc, CREATED_AT);

        assert!(markdown.contains("Page 1:"), "{markdown}");
        assert!(markdown.contains("Page 3:"), "{markdown}");
    }

    /// The conflict report carries extracted text, which was never meant to be
    /// Markdown. Text that escaped the report would become structure in a
    /// document whose whole purpose is that it does not.
    #[test]
    fn extracted_text_cannot_break_out_of_the_conflict_report() {
        let doc = with_conflicts(vec![disagreement(
            ConflictKind::TextMismatch,
            &[
                ("vision", "# not a heading\n```\nstill inside"),
                ("ocr", "ordinary"),
            ],
        )]);

        let parsed = reparse(&markdown(&doc, CREATED_AT));

        // The report's own heading and nothing else: the extracted `#` did not
        // become one.
        assert_eq!(parsed.headings, vec![(2, "Conflicts".to_owned())]);
        assert!(parsed.paragraphs.contains(&"Body".to_owned()));
        // And the injected text stayed inside the block that holds it.
        assert!(
            !parsed
                .paragraphs
                .iter()
                .any(|paragraph| paragraph.contains("not a heading")),
            "paragraphs: {:?}",
            parsed.paragraphs
        );
    }

    // ── Verification ─────────────────────────────────────────────────────────

    /// A page nobody checked and a page that passed are different things, and
    /// the ceiling means a long scan can be partly one and partly the other.
    #[test]
    fn the_front_matter_says_how_much_was_checked() {
        assert!(rendered(vec![block(BlockKind::Paragraph, "x")]).contains("verified: 0/1"));

        let mut doc = document(vec![block(BlockKind::Paragraph, "x")]);
        doc.pages[0].provenance.verified = true;
        assert!(markdown(&doc, CREATED_AT).contains("verified: 1/1"));
    }

    #[test]
    fn a_partly_checked_document_says_so() {
        let mut doc = document(vec![block(BlockKind::Paragraph, "x")]);
        doc.pages[0].provenance.verified = true;
        doc.pages.push(Page {
            index: 1,
            size: None,
            blocks: vec![block(BlockKind::Paragraph, "y")],
            provenance: PageProvenance {
                primary: EngineId::from("pdfium"),
                engines: vec![EngineId::from("pdfium")],
                verified: false,
            },
        });

        assert!(markdown(&doc, CREATED_AT).contains("verified: 1/2"));
    }

    #[test]
    fn the_title_falls_back_to_the_file_stem() {
        let markdown = rendered(vec![block(BlockKind::Paragraph, "x")]);
        assert!(markdown.contains("title: report\n"), "{markdown}");
    }

    /// The document's own heading beats the name somebody saved it under, and
    /// the title is also what names the output file.
    #[test]
    fn the_title_prefers_the_documents_own_heading() {
        let markdown = rendered(vec![
            block(BlockKind::Heading { level: 1 }, "2024 Annual Report"),
            block(BlockKind::Paragraph, "x"),
        ]);

        assert!(markdown.contains("title: 2024 Annual Report"), "{markdown}");
    }

    #[test]
    fn metadata_title_beats_the_heading() {
        let mut doc = document(vec![block(BlockKind::Heading { level: 1 }, "Heading")]);
        doc.meta.title = Some("Given".into());

        assert!(markdown(&doc, CREATED_AT).contains("title: Given"));
    }

    /// A title containing a colon would otherwise turn the rest of the line
    /// into a nested mapping.
    #[test]
    fn a_title_with_a_colon_is_quoted() {
        let mut doc = document(vec![block(BlockKind::Paragraph, "x")]);
        doc.meta.title = Some("Report: 2024".into());

        let markdown = markdown(&doc, CREATED_AT);
        assert!(markdown.contains("title: \"Report: 2024\""), "{markdown}");
    }

    #[test]
    fn a_title_with_a_quote_is_escaped() {
        let mut doc = document(vec![block(BlockKind::Paragraph, "x")]);
        doc.meta.title = Some("The \"big\" report".into());

        let markdown = markdown(&doc, CREATED_AT);
        assert!(
            markdown.contains(r#"title: "The \"big\" report""#),
            "{markdown}"
        );
    }

    /// A blank line between list items makes a loose list, which a reader
    /// renders with each item wrapped in a paragraph — the same words,
    /// presented as something else.
    #[test]
    fn adjacent_list_items_are_not_separated_by_a_blank_line() {
        let markdown = rendered(vec![
            block(BlockKind::ListItem, "first"),
            block(BlockKind::ListItem, "second"),
        ]);

        assert!(body(&markdown).contains("- first\n- second"), "{}", body(&markdown));
        assert!(!body(&markdown).contains("- first\n\n- second"));
    }

    #[test]
    fn a_list_is_separated_from_what_follows_it() {
        let markdown = rendered(vec![
            block(BlockKind::ListItem, "item"),
            block(BlockKind::Paragraph, "after"),
        ]);

        assert!(body(&markdown).contains("- item\n\nafter"), "{}", body(&markdown));
    }

    #[test]
    fn blocks_are_separated_by_a_blank_line() {
        let markdown = rendered(vec![
            block(BlockKind::Paragraph, "one"),
            block(BlockKind::Paragraph, "two"),
        ]);
        assert!(body(&markdown).contains("one\n\ntwo"));
    }

    #[test]
    fn slug_is_lowercase_kebab() {
        assert_eq!(slug("2024 Annual Report"), "2024-annual-report");
    }

    #[test]
    fn slug_keeps_cjk_rather_than_transliterating() {
        assert_eq!(slug("2024 年度报告"), "2024-年度报告");
    }

    #[test]
    fn slug_removes_characters_windows_forbids() {
        assert_eq!(slug("a:b/c\\d?e*f\"g<h>i|j"), "a-b-c-d-e-f-g-h-i-j");
    }

    #[test]
    fn slug_collapses_runs_of_separators() {
        assert_eq!(slug("a   ---   b"), "a-b");
    }

    /// A dash is legal in a filename, but it is a word separator here. Treating
    /// it as content would keep it and then add more around it.
    #[test]
    fn slug_does_not_multiply_dashes() {
        assert_eq!(slug("Q1 - 2024"), "q1-2024");
        assert_eq!(slug("well-known"), "well-known");
    }

    #[test]
    fn slug_drops_leading_and_trailing_separators() {
        assert_eq!(slug("  spaced  "), "spaced");
        assert_eq!(slug("::leading"), "leading");
    }

    #[test]
    fn an_empty_slug_falls_back_to_untitled() {
        assert_eq!(slug(""), "untitled");
        assert_eq!(slug("///"), "untitled");
    }

    /// Windows refuses these names whatever the extension, so the failure
    /// would appear when the file is written rather than when it is named.
    #[test]
    fn a_reserved_windows_name_is_prefixed() {
        assert_eq!(slug("CON"), "_con");
        assert_eq!(slug("LPT1"), "_lpt1");
        assert_eq!(slug("console"), "console");
    }

    #[test]
    fn a_long_slug_is_truncated_on_a_character_boundary() {
        let slug = slug(&"年".repeat(300));
        assert_eq!(slug.chars().count(), MAX_SLUG_LENGTH);
    }

    // ── Round trip ───────────────────────────────────────────────────────────
    //
    // Rendering is only correct if a Markdown reader sees the structure the IR
    // held. These tests read the output back with a real parser rather than
    // asserting on substrings, because the failures that matter are the ones
    // where the text looks right and means something else.

    #[derive(Debug, Default, PartialEq)]
    struct Structure {
        headings: Vec<(u8, String)>,
        paragraphs: Vec<String>,
        list_items: usize,
        table_columns: Vec<usize>,
        table_rows: usize,
    }

    fn reparse(markdown: &str) -> Structure {
        let arena = Arena::new();
        let mut options = Options::default();
        options.extension.table = true;
        // Without this the front matter is read as a thematic break followed by
        // paragraphs of YAML.
        options.extension.front_matter_delimiter = Some("---".to_owned());

        let root = parse_document(&arena, markdown, &options);
        let mut structure = Structure::default();

        for node in root.descendants() {
            match node.data.borrow().value {
                NodeValue::Heading(ref heading) => {
                    structure.headings.push((heading.level, node_text(&node)));
                }
                NodeValue::Paragraph => structure.paragraphs.push(node_text(&node)),
                NodeValue::Item(..) => structure.list_items += 1,
                NodeValue::Table(ref table) => {
                    structure.table_columns.push(table.alignments.len());
                }
                NodeValue::TableRow(..) => structure.table_rows += 1,
                _ => {}
            }
        }

        structure
    }

    fn node_text(node: &comrak::Node<'_>) -> String {
        let mut text = String::new();
        for descendant in node.descendants() {
            if let NodeValue::Text(ref value) = descendant.data.borrow().value {
                text.push_str(value);
            }
        }
        text
    }

    fn two_by_two_table() -> Block {
        let mut table = block(BlockKind::Table, "");
        for cells in [["A1", "B1"], ["A2", "B2"]] {
            table.children.push(Block {
                kind: BlockKind::TableRow,
                text: String::new(),
                bbox: None,
                confidence: None,
                engine: EngineId::from("test"),
                children: cells
                    .iter()
                    .map(|cell| block(BlockKind::TableCell, cell))
                    .collect(),
            });
        }
        table
    }

    /// A heading whose words ran onto a second line on the page is one heading,
    /// and the parser has to see it as one. Written with the break left in, the
    /// heading ends at the newline and its second half becomes a paragraph —
    /// measured on a three-column notice, seven headings and none found.
    #[test]
    fn a_heading_that_ran_to_two_lines_is_still_one_heading() {
        let source = document(vec![block(
            BlockKind::Heading { level: 1 },
            "SECURITIES AND EXCHANGE\nCOMMISSION",
        )]);

        let rendered = markdown(&source, CREATED_AT);
        let parsed = reparse(&rendered);

        assert_eq!(
            parsed.headings,
            vec![(1, "SECURITIES AND EXCHANGE COMMISSION".to_owned())],
            "{rendered}"
        );
        assert!(parsed.paragraphs.is_empty(), "{rendered}");
    }

    /// And a Chinese heading is closed up, because a break in a Chinese line is
    /// where the column ran out and not a space the page has.
    #[test]
    fn a_chinese_heading_is_not_given_a_space_at_the_break() {
        let source = document(vec![block(
            BlockKind::Heading { level: 1 },
            "洛阳市抗旱
从种子抓起",
        )]);

        let parsed = reparse(&markdown(&source, CREATED_AT));

        assert_eq!(parsed.headings, vec![(1, "洛阳市抗旱从种子抓起".to_owned())]);
    }

    /// A paragraph keeps its line breaks, except where one would be rendered as
    /// a space the document does not have.
    ///
    /// Markdown turns a newline inside a paragraph into a space, so a Chinese
    /// paragraph arrives in the window with a space at every place the column
    /// ran out. The break between two Latin words is left alone: there it **is**
    /// a space, and where it is is where the page put it.
    #[test]
    fn a_chinese_paragraph_is_not_given_spaces_at_its_line_breaks() {
        let source = document(vec![
            block(BlockKind::Paragraph, "我国大豆基因组研究
取得重大突破"),
            block(BlockKind::Paragraph, "one two
three four"),
        ]);

        let rendered = markdown(&source, CREATED_AT);
        let page = html(&rendered);

        assert!(page.contains("我国大豆基因组研究取得重大突破"), "{page}");
        assert!(page.contains("one two
three four"), "{page}");
    }

    #[test]
    fn structure_survives_a_round_trip_through_a_markdown_parser() {
        let source = document(vec![
            block(BlockKind::Heading { level: 1 }, "Title"),
            block(BlockKind::Paragraph, "Body text"),
            block(BlockKind::ListItem, "First"),
            block(BlockKind::ListItem, "Second"),
            two_by_two_table(),
            block(BlockKind::Heading { level: 3 }, "Section"),
        ]);

        let parsed = reparse(&markdown(&source, CREATED_AT));

        assert_eq!(
            parsed.headings,
            vec![(1, "Title".to_owned()), (3, "Section".to_owned())]
        );
        assert_eq!(parsed.list_items, 2);
        assert!(parsed.paragraphs.contains(&"Body text".to_owned()));
    }

    #[test]
    fn a_table_survives_a_round_trip_with_its_dimensions() {
        let parsed = reparse(&markdown(&document(vec![two_by_two_table()]), CREATED_AT));

        assert_eq!(parsed.table_columns, vec![2]);
        assert_eq!(parsed.table_rows, 2);
    }

    /// The whole point of escaping: text that looks like structure must not
    /// become structure when a reader looks at it.
    #[test]
    fn escaped_text_does_not_become_structure_when_reparsed() {
        let source = document(vec![
            block(BlockKind::Paragraph, "# not a heading"),
            block(BlockKind::Paragraph, "- not a list"),
            block(BlockKind::Paragraph, "2024. not a list either"),
        ]);

        let parsed = reparse(&markdown(&source, CREATED_AT));

        assert!(parsed.headings.is_empty(), "headings: {:?}", parsed.headings);
        assert_eq!(parsed.list_items, 0, "a bulleted line became a list item");
        assert!(
            parsed.paragraphs.contains(&"# not a heading".to_owned()),
            "paragraphs: {:?}",
            parsed.paragraphs
        );
        assert!(
            parsed
                .paragraphs
                .contains(&"2024. not a list either".to_owned()),
            "paragraphs: {:?}",
            parsed.paragraphs
        );
    }

    // ── HTML ─────────────────────────────────────────────────────────────────

    #[test]
    fn html_renders_the_structure_a_reader_would_see() {
        let html = html(&markdown(
            &document(vec![
                block(BlockKind::Heading { level: 1 }, "Title"),
                block(BlockKind::Paragraph, "Body"),
                two_by_two_table(),
            ]),
            CREATED_AT,
        ));

        assert!(html.contains("<h1>Title</h1>"), "{html}");
        assert!(html.contains("<p>Body</p>"), "{html}");
        assert!(html.contains("<table>"), "{html}");
    }

    /// The front matter is the tool's record of where a document came from. A
    /// reader opening the document should not be shown the plumbing.
    #[test]
    fn html_leaves_the_front_matter_out() {
        let html = html(&markdown(&document(vec![block(BlockKind::Paragraph, "Body")]), CREATED_AT));

        assert!(!html.contains("sha256"), "{html}");
        assert!(!html.contains("showing"), "{html}");
    }

    /// Extracted text is not trusted markup. This output goes into a webview,
    /// where markup that passes through is script that runs.
    #[test]
    fn html_escapes_raw_markup_in_the_document() {
        let html = html("<script>alert(1)</script>");

        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
    }

    /// Front matter is metadata, not content. If a reader saw it as body text
    /// it would be indexed and searched as if it were part of the document.
    #[test]
    fn front_matter_is_not_part_of_the_body() {
        let parsed = reparse(&markdown(
            &document(vec![block(BlockKind::Paragraph, "Body")]),
            CREATED_AT,
        ));

        assert_eq!(parsed.paragraphs, vec!["Body".to_owned()]);
    }
}
