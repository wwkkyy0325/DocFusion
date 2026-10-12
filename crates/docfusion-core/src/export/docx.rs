//! A document as Word holds one.
//!
//! Built from the same parse the pane is drawn from — `comrak`'s — and not from
//! the HTML, because Word's XML is a document model and the HTML is a rendering
//! of one. A heading is a heading here; an `<h2>` that happens to have been
//! styled like one is not the same thing, and this is the format somebody opens
//! in order to *edit* what came out of the scanner.

use std::fmt::Write as _;

use comrak::nodes::{AstNode, ListType, NodeValue};
use comrak::{Arena, parse_document};

use super::xml;
use crate::render;

/// The text column in twips, which is what a table divides up when its columns
/// are given a width. A4 less the margins below: 11906 − 1440 − 1440.
const TEXT_WIDTH: i32 = 9026;

/// Where a list item's text sits, in twips, one standard indent per level and
/// one more for the item itself.
fn list_indent(depth: i32) -> i32 {
    (depth + 1) * 720
}

/// The property a run carries: which of the four things it is.
///
/// A copy rather than a stack of borrows, because it is pushed down the parse
/// and every level may change one of them.
#[derive(Debug, Clone, Copy, Default)]
struct Style {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
}

pub fn of(markdown: &str) -> Vec<u8> {
    let options = render::rendering();
    let arena = Arena::new();
    let document = parse_document(&arena, markdown, &options);

    let mut body = String::new();
    blocks(&mut body, document);

    // **A Word body ends with a paragraph**, and a table may not be the last
    // thing in one — so there is always an empty paragraph at the end, which is
    // what Word writes in its own files.
    body.push_str("<w:p/>");

    let document = format!("{HEAD}{body}{TAIL}");

    // `[Content_Types].xml` first; see `xml::zip`.
    xml::zip(&[
        ("[Content_Types].xml", CONTENT_TYPES.to_owned()),
        ("_rels/.rels", ROOT_RELATIONSHIPS.to_owned()),
        ("word/document.xml", document),
        ("word/styles.xml", STYLES.to_owned()),
        ("word/_rels/document.xml.rels", DOCUMENT_RELATIONSHIPS.to_owned()),
    ])
}

// ── the body ────────────────────────────────────────────────────────────────

/// Emits the block children of `parent`.
///
/// **Two tables in a row get a paragraph between them**, because Word joins two
/// adjacent tables into one — and a document holding two tables with a blank
/// line between them is not a document holding one table of both.
fn blocks<'a>(out: &mut String, parent: &'a AstNode<'a>) {
    let mut last_was_table = false;

    for node in parent.children() {
        let table = matches!(node.data.borrow().value, NodeValue::Table(_));

        if table && last_was_table {
            out.push_str("<w:p/>");
        }

        block(out, node);
        last_was_table = table;
    }
}

fn block<'a>(out: &mut String, node: &'a AstNode<'a>) {
    match &node.data.borrow().value {
        // Where the document came from, not what it says — the same line the
        // pane drops.
        NodeValue::FrontMatter(_) => {}

        NodeValue::Heading(heading) => {
            let style = format!("Heading{}", heading.level.clamp(1, 6));
            paragraph(out, Some(&style), 0, None, node);
        }

        NodeValue::Paragraph => paragraph(out, None, 0, None, node),

        NodeValue::BlockQuote => quote(out, node),

        NodeValue::List(list) => {
            let (kind, start) = (list.list_type, list.start);
            list_items(out, node, kind, start, 0);
        }

        NodeValue::CodeBlock(code) => code_block(out, &code.literal),

        NodeValue::ThematicBreak => rule(out),

        NodeValue::Table(_) => table(out, node),

        // Raw HTML from a document that had some: the words it is, because
        // what goes into an XML package cannot be markup this program did not
        // write.
        NodeValue::HtmlBlock(html) => {
            out.push_str("<w:p>");
            run(out, &html.literal, Style::default());
            out.push_str("</w:p>");
        }

        _ => {}
    }
}

/// A quoted passage: the paragraphs it holds, in the style that marks them.
///
/// A quote holding a list keeps the list. The style is a paragraph's, and a
/// list item is a paragraph like any other.
fn quote<'a>(out: &mut String, node: &'a AstNode<'a>) {
    for child in node.children() {
        match &child.data.borrow().value {
            NodeValue::Paragraph => paragraph(out, Some("Quote"), 0, None, child),
            NodeValue::List(list) => {
                let (kind, start) = (list.list_type, list.start);
                list_items(out, child, kind, start, 0);
            }
            _ => block(out, child),
        }
    }
}

/// A list: each item on a paragraph of its own, with its marker written as text.
///
/// **The marker is written down rather than declared.** A real Word list is a
/// numbering definition in a third part, which ends up inferring the numbers —
/// and what reaches here is Markdown, where whatever number a line carries is
/// the number that line has. A document that numbered its clauses `3.1`, `3.2`
/// would come out `1.`, `2.` if Word were left to count them itself.
///
/// A list read off a page arrives as bullets either way, because the IR has
/// items and not ordered lists — so this rule is about the Markdown the writer
/// is handed, which a passthrough document can be any of.
fn list_items<'a>(out: &mut String, node: &'a AstNode<'a>, kind: ListType, start: usize, depth: i32) {
    for (number, item) in (start..).zip(node.children()) {
        let marker = match kind {
            ListType::Bullet => "• ".to_owned(),
            ListType::Ordered => format!("{number}. "),
        };

        item_out(out, item, &marker, depth);
    }
}

fn item_out<'a>(out: &mut String, item: &'a AstNode<'a>, marker: &str, depth: i32) {
    let mut carried = false;

    for child in item.children() {
        match &child.data.borrow().value {
            // A list under an item is the next level down.
            NodeValue::List(nested) => {
                let (kind, start) = (nested.list_type, nested.start);
                list_items(out, child, kind, start, depth + 1);
            }
            // The marker goes on the item's first paragraph, which is where the
            // words it labels are.
            NodeValue::Paragraph if !carried => {
                carried = true;
                paragraph(out, None, list_indent(depth), Some(marker), child);
            }
            _ => block(out, child),
        }
    }
}

/// One paragraph: how it is styled, how far in it sits, what is written before
/// its words, and the words.
fn paragraph<'a>(
    out: &mut String,
    style: Option<&str>,
    indent: i32,
    marker: Option<&str>,
    node: &'a AstNode<'a>,
) {
    out.push_str("<w:p>");
    out.push_str(&paragraph_properties(style, indent));

    if let Some(marker) = marker {
        run(out, marker, Style::default());
    }

    inlines(out, node, Style::default());
    out.push_str("</w:p>");
}

/// What a paragraph carries of its own. Empty for a plain one, so that the
/// commonest paragraph in a document writes nothing at all.
fn paragraph_properties(style: Option<&str>, indent: i32) -> String {
    if style.is_none() && indent == 0 {
        return String::new();
    }

    let mut out = String::from("<w:pPr>");

    if let Some(style) = style {
        write!(out, "<w:pStyle w:val=\"{style}\"/>").expect("a String is always writable");
    }
    if indent > 0 {
        write!(out, "<w:ind w:left=\"{indent}\"/>").expect("a String is always writable");
    }

    out.push_str("</w:pPr>");
    out
}

// ── the runs ────────────────────────────────────────────────────────────────

/// The runs of one paragraph, or of one table cell.
fn inlines<'a>(out: &mut String, parent: &'a AstNode<'a>, style: Style) {
    for node in parent.children() {
        match &node.data.borrow().value {
            NodeValue::Text(text) => run(out, text, style),
            NodeValue::Code(code) => run(out, &code.literal, Style { code: true, ..style }),
            NodeValue::Strong => inlines(out, node, Style { bold: true, ..style }),
            NodeValue::Emph => inlines(out, node, Style { italic: true, ..style }),
            NodeValue::Strikethrough => inlines(out, node, Style { strike: true, ..style }),
            // A line the author broke is a space by the time it is a paragraph.
            NodeValue::SoftBreak => run(out, " ", style),
            // …and one they meant as a break is a break.
            NodeValue::LineBreak => out.push_str("<w:r><w:br/></w:r>"),
            NodeValue::Link(link) => link_run(out, node, &link.url, style),
            NodeValue::Image(image) => {
                let alt = text_of(node);
                let words = if alt.is_empty() { image.url.clone() } else { alt };
                run(out, &format!("[{words}]"), style);
            }
            NodeValue::HtmlInline(html) => run(out, html, style),
            _ => inlines(out, node, style),
        }
    }
}

/// A link, as its words and — when they are not the address — the address after
/// them.
///
/// A real Word hyperlink is a relationship pointing out of the package, and what
/// it would point at is a page on a network this program knows nothing about.
/// The address is written down instead, where it survives being printed.
fn link_run<'a>(out: &mut String, node: &'a AstNode<'a>, url: &str, style: Style) {
    let text = text_of(node);

    if text.is_empty() || text == url {
        run(out, url, style);
        return;
    }

    run(out, &format!("{text} ({url})"), style);
}

fn run(out: &mut String, text: &str, style: Style) {
    out.push_str("<w:r>");
    out.push_str(&run_properties(style));
    out.push_str("<w:t xml:space=\"preserve\">");
    out.push_str(&xml::escape(text));
    out.push_str("</w:t></w:r>");
}

/// What a run carries of its own, in the order the schema wants it in — which is
/// why the font goes before the bold and not after it.
fn run_properties(style: Style) -> String {
    let mut out = String::new();

    if style.bold {
        out.push_str("<w:b/>");
    }
    if style.italic {
        out.push_str("<w:i/>");
    }
    if style.strike {
        out.push_str("<w:strike/>");
    }
    if style.code {
        out.push_str("<w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\"/>");
    }

    if out.is_empty() {
        return out;
    }

    format!("<w:rPr>{out}</w:rPr>")
}

/// A fenced block: one paragraph in the code style, its lines kept apart by
/// breaks rather than by paragraphs. A code block is one thing, and Word would
/// otherwise put a paragraph's own spacing between every line of it.
fn code_block(out: &mut String, literal: &str) {
    out.push_str("<w:p><w:pPr><w:pStyle w:val=\"Code\"/></w:pPr>");
    out.push_str("<w:r><w:rPr><w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\"/></w:rPr>");

    for (at, line) in literal.trim_end_matches('\n').lines().enumerate() {
        if at > 0 {
            out.push_str("<w:br/>");
        }
        out.push_str("<w:t xml:space=\"preserve\">");
        out.push_str(&xml::escape(line));
        out.push_str("</w:t>");
    }

    out.push_str("</w:r></w:p>");
}

/// A thematic break, which Word has no element for: a paragraph whose only mark
/// is a rule under it.
fn rule(out: &mut String) {
    out.push_str(
        "<w:p><w:pPr><w:pBdr>\
         <w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"999999\"/>\
         </w:pBdr></w:pPr></w:p>",
    );
}

// ── tables ──────────────────────────────────────────────────────────────────

fn table<'a>(out: &mut String, node: &'a AstNode<'a>) {
    let columns = node
        .first_child()
        .map(|row| row.children().count())
        .unwrap_or(0);

    if columns == 0 {
        return;
    }

    // Every column the same width. Sizing them by what is in them is the job of
    // whatever the document is opened in, and a table read off a page has no
    // column widths left to recover anyway.
    let width = TEXT_WIDTH / columns as i32;

    out.push_str(
        "<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>\
         <w:top w:val=\"single\" w:sz=\"4\" w:color=\"999999\"/>\
         <w:left w:val=\"single\" w:sz=\"4\" w:color=\"999999\"/>\
         <w:bottom w:val=\"single\" w:sz=\"4\" w:color=\"999999\"/>\
         <w:right w:val=\"single\" w:sz=\"4\" w:color=\"999999\"/>\
         <w:insideH w:val=\"single\" w:sz=\"4\" w:color=\"999999\"/>\
         <w:insideV w:val=\"single\" w:sz=\"4\" w:color=\"999999\"/>\
         </w:tblBorders></w:tblPr><w:tblGrid>",
    );

    for _ in 0..columns {
        write!(out, "<w:gridCol w:w=\"{width}\"/>").expect("a String is always writable");
    }

    out.push_str("</w:tblGrid>");

    for row in node.children() {
        let header = matches!(row.data.borrow().value, NodeValue::TableRow(true));
        out.push_str("<w:tr>");

        // Said rather than inferred from the formatting, so that the row stays
        // the header when the table runs over a page.
        if header {
            out.push_str("<w:trPr><w:tblHeader/></w:trPr>");
        }

        for cell in row.children() {
            write!(
                out,
                "<w:tc><w:tcPr><w:tcW w:w=\"{width}\" w:type=\"dxa\"/></w:tcPr>"
            )
            .expect("a String is always writable");

            out.push_str("<w:p>");

            let style = Style {
                bold: header,
                ..Style::default()
            };

            // On the paragraph mark as well, so a header cell that happens to be
            // empty is still a header.
            if header {
                out.push_str("<w:pPr><w:rPr><w:b/></w:rPr></w:pPr>");
            }

            inlines(out, cell, style);
            out.push_str("</w:p></w:tc>");
        }

        out.push_str("</w:tr>");
    }

    out.push_str("</w:tbl>");
}

// ── the places a document needs one string rather than a run of them ────────

fn text_of<'a>(node: &'a AstNode<'a>) -> String {
    let mut out = String::new();
    collect_text(node, &mut out);
    out
}

fn collect_text<'a>(parent: &'a AstNode<'a>, out: &mut String) {
    for node in parent.children() {
        match &node.data.borrow().value {
            NodeValue::Text(text) => out.push_str(text),
            NodeValue::Code(code) => out.push_str(&code.literal),
            _ => collect_text(node, out),
        }
    }
}

// ── the package ─────────────────────────────────────────────────────────────

const HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
                    <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
                    <w:body>";

/// A4 with two centimetres of margin, which is what a Word document is unless
/// somebody says otherwise.
const TAIL: &str = "<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/>\
                    <w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" \
                    w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr>\
                    </w:body></w:document>";

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
</Types>";

const ROOT_RELATIONSHIPS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
</Relationships>";

const DOCUMENT_RELATIONSHIPS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
</Relationships>";

/// The styles the body above refers to.
///
/// A document that names a style it does not define still opens — it opens in
/// Times New Roman with no headings in the navigation pane, which is the
/// difference between a document somebody can work in and one they can only
/// read. `w:name` is lower case and spaced because that is the name Word knows
/// the built-in styles by; `w:styleId` is this program's.
const STYLES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:docDefaults>\
<w:rPrDefault><w:rPr>\
<w:rFonts w:ascii=\"Calibri\" w:hAnsi=\"Calibri\" w:eastAsia=\"SimSun\"/>\
<w:sz w:val=\"21\"/><w:szCs w:val=\"21\"/></w:rPr></w:rPrDefault>\
<w:pPrDefault><w:pPr><w:spacing w:after=\"140\" w:line=\"264\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault>\
</w:docDefaults>\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/>\
<w:pPr><w:outlineLvl w:val=\"0\"/><w:spacing w:before=\"240\" w:after=\"80\"/></w:pPr>\
<w:rPr><w:b/><w:sz w:val=\"36\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading2\"><w:name w:val=\"heading 2\"/><w:basedOn w:val=\"Normal\"/>\
<w:pPr><w:outlineLvl w:val=\"1\"/><w:spacing w:before=\"200\" w:after=\"70\"/></w:pPr>\
<w:rPr><w:b/><w:sz w:val=\"30\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading3\"><w:name w:val=\"heading 3\"/><w:basedOn w:val=\"Normal\"/>\
<w:pPr><w:outlineLvl w:val=\"2\"/><w:spacing w:before=\"180\" w:after=\"60\"/></w:pPr>\
<w:rPr><w:b/><w:sz w:val=\"26\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading4\"><w:name w:val=\"heading 4\"/><w:basedOn w:val=\"Normal\"/>\
<w:pPr><w:outlineLvl w:val=\"3\"/><w:spacing w:before=\"160\" w:after=\"50\"/></w:pPr>\
<w:rPr><w:b/><w:sz w:val=\"24\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading5\"><w:name w:val=\"heading 5\"/><w:basedOn w:val=\"Normal\"/>\
<w:pPr><w:outlineLvl w:val=\"4\"/><w:spacing w:before=\"160\" w:after=\"50\"/></w:pPr>\
<w:rPr><w:b/><w:sz w:val=\"22\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Heading6\"><w:name w:val=\"heading 6\"/><w:basedOn w:val=\"Normal\"/>\
<w:pPr><w:outlineLvl w:val=\"5\"/><w:spacing w:before=\"160\" w:after=\"50\"/></w:pPr>\
<w:rPr><w:b/><w:sz w:val=\"21\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Quote\"><w:name w:val=\"Quote\"/><w:basedOn w:val=\"Normal\"/>\
<w:pPr><w:pBdr><w:left w:val=\"single\" w:sz=\"18\" w:space=\"8\" w:color=\"AAAAAA\"/></w:pBdr>\
<w:ind w:left=\"360\"/></w:pPr>\
<w:rPr><w:i/><w:color w:val=\"595959\"/></w:rPr></w:style>\
<w:style w:type=\"paragraph\" w:styleId=\"Code\"><w:name w:val=\"Code\"/><w:basedOn w:val=\"Normal\"/>\
<w:pPr><w:shd w:val=\"clear\" w:fill=\"F5F6F8\"/><w:spacing w:before=\"0\" w:after=\"0\"/></w:pPr>\
<w:rPr><w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\"/><w:sz w:val=\"19\"/></w:rPr></w:style>\
</w:styles>";

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    const DOCUMENT: &str = "# 标题\n\n一段话。\n\n| 名称 | 数量 |\n|---|---|\n| 甲方 | 12 |\n";

    fn part(bytes: &[u8], name: &str) -> String {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("a zip");
        let mut body = String::new();
        archive
            .by_name(name)
            .unwrap_or_else(|_| panic!("no {name} in the package"))
            .read_to_string(&mut body)
            .expect("utf-8");
        body
    }

    fn body(markdown: &str) -> String {
        part(&of(markdown), "word/document.xml")
    }

    #[test]
    fn a_heading_is_a_heading_rather_than_a_styled_line() {
        let body = body("# One\n\n## Two\n");

        assert!(body.contains("<w:pStyle w:val=\"Heading1\"/>"), "{body}");
        assert!(body.contains("<w:pStyle w:val=\"Heading2\"/>"), "{body}");
    }

    /// The style has to be defined, and defined under the name Word knows it by,
    /// or the paragraph renders as body text and the navigation pane is empty.
    #[test]
    fn every_style_the_body_names_is_written_down() {
        let body = body("# One\n\n> quoted\n\n```\ncode\n```\n");
        let styles = part(&of("x"), "word/styles.xml");

        for style in ["Heading1", "Quote", "Code"] {
            assert!(body.contains(&format!("w:val=\"{style}\"")), "the body never asks for {style}");
            assert!(styles.contains(&format!("w:styleId=\"{style}\"")), "{style} is not defined");
        }

        assert!(styles.contains("w:val=\"heading 1\""), "Word will not link the style by this name");
    }

    /// A table is a table: cells in columns, and the first row marked as the
    /// header so that it repeats when the table runs over a page.
    #[test]
    fn a_table_is_a_table_with_a_header_row() {
        let body = body(DOCUMENT);

        assert!(body.contains("<w:tbl>"), "{body}");
        assert_eq!(body.matches("<w:gridCol").count(), 2, "two columns: {body}");
        assert_eq!(body.matches("<w:tr>").count(), 2, "a header and one row: {body}");
        assert!(body.contains("<w:tblHeader/>"), "{body}");
        assert!(body.contains(">名称<") && body.contains(">甲方<"), "{body}");
    }

    /// The numbers were on the page and were read as numbers. A list Word
    /// renumbers from one would lose a page that numbered its clauses 3.1, 3.2.
    #[test]
    fn a_list_keeps_the_numbers_the_page_had() {
        let body = body("3. three\n4. four\n\n- bullet\n");

        assert!(body.contains(">3. <"), "{body}");
        assert!(body.contains(">4. <"), "{body}");
        assert!(body.contains(">• <"), "{body}");
    }

    #[test]
    fn emphasis_becomes_the_property_it_means() {
        let body = body("a **bold** and *italic* and ~~struck~~ word\n");

        assert!(body.contains("<w:b/>"), "{body}");
        assert!(body.contains("<w:i/>"), "{body}");
        assert!(body.contains("<w:strike/>"), "{body}");
    }

    /// A line the author broke is a space by the time it is a paragraph, and one
    /// they meant as a break is a break. Getting the first one wrong puts a line
    /// break in the middle of every paragraph of every document that was written
    /// with its lines hard-wrapped.
    #[test]
    fn a_soft_break_is_a_space_and_a_hard_one_is_a_break() {
        let soft = body("one\ntwo\n");

        assert!(!soft.contains("<w:br/>"), "a soft break became a line break: {soft}");
        assert!(soft.contains("<w:t xml:space=\"preserve\"> </w:t>"), "no space: {soft}");

        assert!(body("one  \ntwo\n").contains("<w:br/>"), "a hard break is not one");
    }

    /// The document is somebody else's text. An ampersand written raw into the
    /// XML is a file Word refuses to open.
    ///
    /// Asserted piece by piece rather than as one string: `comrak` ends a text
    /// node where the markup begins, so `a & b <c>` is three runs and there is
    /// no one place to look for it.
    #[test]
    fn a_document_holding_markup_is_escaped() {
        let body = body("a & b <c> \"d\"\n");

        for escaped in ["a &amp; b ", "&lt;c&gt;", "&quot;d&quot;"] {
            assert!(body.contains(escaped), "{escaped} is not in {body}");
        }

        // And nothing of it went in raw, which is the thing that matters: a bare
        // `<` in the middle of the XML is a file Word will not open at all.
        assert!(!body.contains(" <c>"), "{body}");
        assert!(!body.contains(" & b"), "{body}");
    }

    /// Two tables with a blank line between them are two tables, and Word joins
    /// adjacent ones into a single table of both.
    #[test]
    fn two_tables_in_a_row_are_kept_apart() {
        let body = body("| a |\n|---|\n| 1 |\n\n| b |\n|---|\n| 2 |\n");

        assert_eq!(body.matches("<w:tbl>").count(), 2, "{body}");
        assert_eq!(body.matches("</w:tbl>").count(), 2, "{body}");
        assert!(body.contains("</w:tbl><w:p/><w:tbl>"), "nothing between them: {body}");
    }

    #[test]
    fn the_front_matter_is_not_part_of_the_document() {
        let body = body("---\ntitle: Report\n---\n\n# Report\n");

        assert!(!body.contains("title: Report"), "{body}");
        assert!(body.contains(">Report<"), "{body}");
    }

    /// **Every part of the package has to be announced in `[Content_Types].xml`**,
    /// and the two main parts have to be announced as what they are rather than
    /// by their extension. A part the file does not account for is a document
    /// Word reports as corrupt and refuses to open — which is the loudest way
    /// this can be wrong and the one worth a test.
    #[test]
    fn every_part_is_announced_in_the_content_types() {
        let bytes = of(DOCUMENT);
        let types = part(&bytes, "[Content_Types].xml");

        // Named, not inferred: without these Word has no way to know this is a
        // document rather than a zip with XML in it.
        for named in [
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml",
        ] {
            assert!(types.contains(named), "{named} is not in {types}");
        }

        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("a zip");

        for at in 0..archive.len() {
            let name = archive.by_index(at).unwrap().name().to_owned();

            // The content types are the file that says what the others are, so
            // it is not one of the others.
            if name == "[Content_Types].xml" {
                continue;
            }

            let covered = types.contains(&format!("/{name}"))
                || (name.ends_with(".rels") && types.contains("Extension=\"rels\""))
                || (name.ends_with(".xml") && types.contains("Extension=\"xml\""));

            assert!(covered, "{name} is in the package and not in the content types");
        }
    }

    /// An empty document is still a document: a reader that opens it should get
    /// a page, not an error.
    #[test]
    fn a_document_with_nothing_in_it_is_still_one() {
        let bytes = of("");
        assert!(part(&bytes, "word/document.xml").contains("<w:sectPr>"));
    }
}
