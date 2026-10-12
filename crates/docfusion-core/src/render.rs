//! Rendering the IR as Markdown.
//!
//! Extracted text is plain text that was never meant to be Markdown. A
//! paragraph that happens to begin with `#` or `-` would be reinterpreted as
//! structure the document never had, so text is escaped before it is written.

use comrak::nodes::NodeValue;
use comrak::{Arena, Options, format_commonmark, format_html, parse_document};

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
    blocks(markdown).into_iter().map(|block| block.html).collect()
}

/// One block of a document, in the two shapes anything can be done with it in.
///
/// A block is a **top-level thing in the rendered document** — a heading, a
/// paragraph, a list, a table, a fenced block — which is what somebody pointing
/// at a page means by "this bit". Not the IR's blocks: those are finer (a list
/// item and a table cell are each one) and they are not what is on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// Whether this block is a table, which is the one thing that changes what
    /// can be done with it: a table can be a spreadsheet and nothing else can.
    /// Every other distinction is one the formats do not care about.
    pub table: bool,
    pub html: String,
    /// The block as Markdown, which is what the document it came from is made
    /// of — so a block taken out of it is a document that stands on its own.
    pub markdown: String,
}

/// Splits a document into the blocks it is displayed as, and renders each one.
///
/// **This is what makes "the nth thing in the pane" and "the nth block" the same
/// block**, rather than two counts that agree until they do not: [`html`] is
/// defined as these blocks joined, so the correspondence is not a convention
/// that has to be maintained, it is the definition. The window numbers the
/// elements it drew; the other side numbers the nodes it parsed; they are the
/// same list because there is only one.
///
/// The parse is `comrak`'s, the same one the pane is rendered with — so a block
/// exported here is the block that was on screen, down to how a table's
/// alignment was written.
pub fn blocks(markdown: &str) -> Vec<Rendered> {
    let options = rendering();

    let arena = Arena::new();
    let document = parse_document(&arena, markdown, &options);

    document
        .children()
        .filter_map(|node| {
            let mut html = String::new();
            format_html(node, &options, &mut html).expect("a String is always writable");

            // **A block is a node that renders to something**, and the front
            // matter is the one that does not: `comrak` keeps it as a top-level
            // node and draws nothing for it, so counting it would put every
            // block's number one out — the window numbers what it drew, and it
            // never drew this. Written as "renders to nothing" rather than "is
            // not front matter" so that anything else `comrak` draws nothing
            // for is excluded by the same line.
            if html.trim().is_empty() {
                return None;
            }

            let mut commonmark = String::new();
            format_commonmark(node, &options, &mut commonmark).expect("a String is always writable");

            Some(Rendered {
                table: matches!(node.data.borrow().value, NodeValue::Table(_)),
                html,
                markdown: commonmark,
            })
        })
        .collect()
}

/// The plain text of a block or a document: the words, and no structure.
///
/// Taken from the parse rather than from the rendering, because getting text
/// out of HTML means taking tags out of it and that is a guess — `&amp;` and a
/// literal `&amp;` in the source are the same three characters by the time it is
/// markup. The parse still knows which characters were the document's.
///
/// A space between two words, and none between two Chinese ones, which is the
/// same rule the rest of this program joins text with.
pub fn plain(markdown: &str) -> String {
    let options = rendering();

    let arena = Arena::new();
    let document = parse_document(&arena, markdown, &options);

    let mut out = String::new();

    for node in document.descendants() {
        match &node.data.borrow().value {
            NodeValue::Text(text) => {
                // No space where a break has already been written: a tab and a
                // space is a cell that starts with a space, and the space is
                // not in the document.
                if needs_space(&out, text) && !out.ends_with(['\t', '\n']) {
                    out.push(' ');
                }
                out.push_str(text);
            }
            // A line break in the source is a break in the words.
            NodeValue::LineBreak | NodeValue::SoftBreak => out.push('\n'),
            NodeValue::Paragraph | NodeValue::Heading(_) => break_before(&mut out, '\n'),
            // **A cell boundary is not a word boundary.** Cells are separate
            // things, and joining them by the rule for words runs 名称 and 数量
            // together — which reads as one word and is two, with nothing left
            // to tell them apart. A tab, because that is what a table flattened
            // into text is made of.
            NodeValue::TableCell => break_before(&mut out, '\t'),
            NodeValue::TableRow(_) => break_before(&mut out, '\n'),
            _ => {}
        }
    }

    // The separator is written before the thing that follows it, so the last
    // one is a tab or a newline that nothing follows.
    out.trim_end_matches(['\t', '\n']).to_owned()
}

/// Ends the current line, unless there is nothing on it.
fn break_before(out: &mut String, separator: char) {
    if !out.is_empty() && !out.ends_with('\n') && !out.ends_with('\t') {
        out.push(separator);
    }
}

/// The options a document is parsed with, wherever it is parsed.
///
/// One function because the callers have to agree: the pane, the blocks it is
/// made of and the exports written from the same text are one parse, and two
/// sets of options would be two different documents.
pub(crate) fn rendering() -> Options<'static> {
    let mut options = Options::default();
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

    options
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
/// The document without the two things this program wrapped around it: the
/// front matter, and the conflict report at the end.
///
/// What is left is what a person reading the file would call the document, and
/// it is what goes into the search index. Both of the things removed are *about*
/// the document rather than in it — a title, a hash, an engine, and a note that
/// two engines read one region differently — and neither is anything the person
/// who wrote the page would recognise as theirs.
///
/// The report especially: it carries the extracted text of the regions the
/// engines disagreed about, so indexing it would make a document findable by a
/// passage that appears twice in its own file and once in the report about it,
/// and would rank the document by the wording of a disagreement.
///
/// Both markers are this program's own, which is what makes this reading back
/// what was written rather than guessing at somebody else's Markdown. A
/// document that is not in this shape — a person's own notes in the tree — is
/// returned whole rather than mangled.
pub fn body_of(markdown: &str) -> &str {
    parts(markdown).body.trim_end()
}

/// A document in the three pieces an editor has to keep apart.
///
/// **The front matter and the report are this program's; the body is the
/// document's.** Somebody editing the words of a document should not be able to
/// damage the record of where it came from or the note about what the engines
/// disagreed on, and [`Parts::joined`] is what makes that impossible rather than
/// merely discouraged: the file that comes out has the front matter it went in
/// with, whatever was typed in between.
///
/// The three always reassemble to exactly the input, which is what a round trip
/// through an editor has to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parts<'a> {
    /// The front matter, its closing `---` and all, or empty.
    pub front: &'a str,
    /// The words.
    pub body: &'a str,
    /// The conflict report, its heading first, or empty.
    pub report: &'a str,
}

impl<'a> Parts<'a> {
    /// The three back as one file.
    pub fn joined(&self) -> String {
        format!("{}{}{}", self.front, self.body, self.report)
    }

    /// The same document with somebody else's words in it, and the program's
    /// two halves untouched.
    pub fn with_body(&self, body: &'a str) -> Self {
        Self {
            front: self.front,
            body,
            report: self.report,
        }
    }
}

/// Splits a document the way an editor needs it split.
///
/// **Line by line rather than by searching for the heading's text.** The report
/// begins at a line of its own at the left margin, and a document that *says*
/// `## Conflicts` inside a paragraph is a document about conflicts — cutting it
/// in half there would take the rest of that paragraph with it.
pub fn parts(markdown: &str) -> Parts<'_> {
    let (front, rest) = split_front(markdown);

    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == REPORT_HEADING {
            return Parts {
                front,
                body: &rest[..offset],
                report: &rest[offset..],
            };
        }
        offset += line.len();
    }

    Parts {
        front,
        body: rest,
        report: "",
    }
}

/// The front matter, its opening `---` and its closing one, or empty.
fn split_front(markdown: &str) -> (&str, &str) {
    let Some(rest) = markdown.strip_prefix("---\n") else {
        return ("", markdown);
    };

    let Some(end) = rest.find("\n---\n") else {
        return ("", markdown);
    };

    markdown.split_at("---\n".len() + end + "\n---\n".len())
}

/// The heading the conflict report begins at, written by [`conflicts_markdown`].
const REPORT_HEADING: &str = "## Conflicts";

/// The front matter's note that somebody has edited this document by hand.
pub const EDITED_AT: &str = "edited_at";

/// The file with a note in its front matter saying when it was last edited.
///
/// When is the last one, not every one: a field that was already there is
/// dropped before the new one is written, so editing a document five times
/// says when the fifth was rather than carrying five lines about it.
///
/// A file with no front matter is returned untouched. Writing one would be this
/// program inventing a record for a document it did not make — which is what
/// the manifest is for, and it knows either way.
pub fn edited(markdown: &str, at: &str) -> String {
    let split = parts(markdown);

    let Some(fields) = split
        .front
        .strip_prefix("---\n")
        .and_then(|rest| rest.strip_suffix("---\n"))
    else {
        return markdown.to_owned();
    };

    let mut out = String::from("---\n");

    for line in fields.lines() {
        if !line.starts_with(&format!("{EDITED_AT}:")) {
            out.push_str(line);
            out.push('\n');
        }
    }

    field(&mut out, EDITED_AT, at);
    out.push_str("---\n");

    out.push_str(split.body);
    out.push_str(split.report);
    out
}

/// When the document was last edited by hand, if it ever was.
pub fn edited_at_of(markdown: &str) -> Option<String> {
    parts(markdown)
        .front
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{EDITED_AT}:")))
        .map(|value| value.trim().trim_matches('"').to_owned())
        .filter(|value| !value.is_empty())
}

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
        // Written as display maths, and **the text inside is not LaTeX**. It is
        // what a recogniser made of the glyphs of a formula, and the difference
        // matters enough to say out loud: nothing here reads a formula, and
        // `docfusion doctor` cannot fetch a model that would.
        //
        // The delimiters stay anyway. The region **is** display maths — the
        // layout model found a formula — and `$$` is the only way Markdown says
        // so, so this is not a claim that the text is LaTeX but a claim about
        // what the region is, which is true. Two things would be worse. Written
        // as prose, `M3 = 34.8°√3` reads as a sentence the document does not
        // contain, and a reader has no way to know it is a bad transcription
        // rather than a bad sentence. Written as nothing, the page stops saying
        // that a formula was here at all, which is the one thing about it that
        // is known and worth passing on — to a person, or to whatever can read
        // it when a formula model arrives.
        //
        // The cost is on the record: 14 of the corpus's 24 cases carry LaTeX in
        // their references, and on those the error rates measure notation
        // rather than reading. See the design notes.
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

    /// **The three pieces reassemble to exactly what went in**, which is what a
    /// round trip through an editor has to be: anything else and opening a
    /// document and saving it unchanged would rewrite the file.
    #[test]
    fn a_document_splits_into_three_and_joins_back_into_one() {
        let markdown = "---\ntitle: Report\n---\n# Heading\n\ntext\n\n## Conflicts\n\npage 1\n";

        let split = parts(markdown);

        assert_eq!(split.front, "---\ntitle: Report\n---\n");
        assert_eq!(split.body, "# Heading\n\ntext\n\n");
        assert_eq!(split.report, "## Conflicts\n\npage 1\n");
        assert_eq!(split.joined(), markdown);
    }

    #[test]
    fn a_document_with_neither_wrapper_is_all_body() {
        let markdown = "# Just notes\n\nsomebody's own\n";

        let split = parts(markdown);

        assert_eq!(split.front, "");
        assert_eq!(split.report, "");
        assert_eq!(split.body, markdown);
    }

    /// **A paragraph that mentions the heading is not the heading.** The report
    /// begins at a line of its own at the left margin, and cutting at the text
    /// instead would take the rest of that paragraph with it.
    #[test]
    fn a_paragraph_naming_the_heading_is_not_cut_in_half() {
        let markdown = "---\ntitle: x\n---\nthe report is under ## Conflicts, at the end\n";

        let split = parts(markdown);

        assert_eq!(split.report, "", "a paragraph was cut in half");
        assert_eq!(split.body, "the report is under ## Conflicts, at the end\n");
    }

    /// **A document with no body still has its report kept apart.** The heading
    /// can be the first thing after the front matter, and a search for the
    /// heading *with a newline in front of it* finds nothing there — so the
    /// report stays in the body, and editing the words would put the report
    /// through the editor as though it were something the document says.
    #[test]
    fn a_document_with_nothing_but_a_report_keeps_it_apart() {
        let markdown = "---\ntitle: x\n---\n## Conflicts\n\npage 1\n";

        let split = parts(markdown);

        assert_eq!(split.body, "");
        assert_eq!(split.report, "## Conflicts\n\npage 1\n");
        assert_eq!(split.joined(), markdown);
    }

    /// The note is when the *last* edit was, not every one: a document edited
    /// five times says so once.
    #[test]
    fn editing_notes_when_and_carries_only_the_last_time() {
        let markdown = "---\ntitle: Report\n---\n# A\n\ntext\n\n## Conflicts\n\npage 1\n";

        let once = edited(markdown, "2026-10-12T10:00:00Z");
        assert_eq!(edited_at_of(&once).as_deref(), Some("2026-10-12T10:00:00Z"));

        let twice = edited(&once, "2026-10-13T09:00:00Z");
        assert_eq!(edited_at_of(&twice).as_deref(), Some("2026-10-13T09:00:00Z"));
        assert_eq!(twice.matches("edited_at:").count(), 1, "{twice}");

        // And nothing else moved: the front matter it went in with, the words,
        // and the report are all where they were.
        assert!(twice.contains("title: Report"), "{twice}");
        assert_eq!(parts(&twice).body, parts(markdown).body);
        assert_eq!(parts(&twice).report, parts(markdown).report);
    }

    #[test]
    fn a_document_nobody_has_edited_says_so() {
        assert_eq!(edited_at_of("---\ntitle: Report\n---\n# A\n"), None);
        assert_eq!(edited_at_of("# no front matter to write in\n"), None);
    }

    /// A file this program did not write has nowhere to keep the note, and
    /// giving it a front matter would be inventing a provenance for it.
    #[test]
    fn a_file_with_no_front_matter_is_not_given_one() {
        let markdown = "# somebody's own notes\n";

        assert_eq!(edited(markdown, "2026-10-12T10:00:00Z"), markdown);
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

    // ── The blocks a document is displayed as ────────────────────────────────

    /// The pane and the blocks it is made of are **one parse, not two that
    /// agree** — which is what lets the window number what it drew and the other
    /// side number what it parsed without either having to be told the other's
    /// convention.
    #[test]
    fn the_html_is_the_blocks_joined() {
        let markdown = "# Title\n\nA paragraph.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- one\n- two\n";

        let joined: String = blocks(markdown).into_iter().map(|block| block.html).collect();

        assert_eq!(joined, html(markdown));
    }

    /// **The front matter is not a block.** It is a top-level node to `comrak`
    /// and it draws nothing, and counting it would put every block's number one
    /// out — so a window numbering what it drew and a backend numbering what it
    /// parsed would disagree by one, on every document, silently.
    #[test]
    fn the_front_matter_is_not_one_of_the_blocks() {
        let markdown = "---\ntitle: Report\nengines: [pdfium]\n---\n\n# Title\n\nA paragraph.\n";

        let found = blocks(markdown);

        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].markdown.trim(), "# Title");
        assert_eq!(found[1].markdown.trim(), "A paragraph.");
        assert!(!found[0].html.contains("title"), "{found:?}");
    }

    #[test]
    fn a_block_knows_when_it_is_a_table() {
        let markdown = "# Title\n\nA paragraph.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- one\n";

        let tables: Vec<bool> = blocks(markdown).iter().map(|block| block.table).collect();

        assert_eq!(tables, vec![false, false, true, false]);
    }

    /// A block taken out of a document is Markdown that stands on its own —
    /// which is the whole of what "export this bit" means.
    #[test]
    fn a_block_is_markdown_of_its_own() {
        let markdown = "# Title\n\nThe words.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";

        let found = blocks(markdown);

        assert_eq!(found.len(), 3, "{found:?}");
        assert_eq!(found[0].markdown.trim(), "# Title");
        assert!(found[1].markdown.contains("The words."));
        assert!(found[2].markdown.contains("| a | b |"), "{:?}", found[2]);
    }

    /// The words, with the punctuation of the markup taken out and nothing else
    /// disturbed — including the rule about spaces, which for Chinese is that
    /// there are none.
    #[test]
    fn the_plain_text_is_the_words() {
        let text = plain("# 标题\n\n我国大豆研究取得突破\n");

        assert_eq!(text, "标题\n我国大豆研究取得突破");
        assert!(!text.contains('#'));
    }

    /// Taken from the parse and not from the markup, because getting text out
    /// of markup means taking tags out of it and that cannot be done: the two
    /// are the same characters by then.
    #[test]
    fn an_ampersand_comes_back_as_an_ampersand() {
        assert_eq!(plain("Ben & Jerry"), "Ben & Jerry");
        assert_eq!(plain("a < b"), "a < b");
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
