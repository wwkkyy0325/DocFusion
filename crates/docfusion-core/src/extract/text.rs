//! Pass-through for inputs that are already text: `.txt` and `.md`.
//!
//! This engine exists mostly so the pipeline has one shape for every input. It
//! does not attempt to be a Markdown parser — it recognises headings and splits
//! on blank lines, which is what carries over into the output.

use comrak::nodes::NodeValue;
use comrak::{Arena, Node, Options, parse_document};

use crate::ir::{Block, BlockKind, DocMeta, Document, EngineId, Page, PageProvenance};
use crate::mime::Container;

use super::{ExtractError, Extractor, Fit, Input, InputProbe};

pub const ENGINE_VERSION: &str = "1.0.0";

pub struct TextPassthrough;

impl Extractor for TextPassthrough {
    fn id(&self) -> EngineId {
        EngineId::from("text-passthrough")
    }

    fn version(&self) -> &str {
        ENGINE_VERSION
    }

    fn accepts(&self, probe: &InputProbe) -> Fit {
        match probe.container {
            Container::Text => Fit::Yes,
            _ => Fit::No,
        }
    }

    fn extract(&self, input: &Input<'_>) -> Result<Document, ExtractError> {
        let engine = self.id();
        let text = decode(input.bytes);

        // Markdown is read as Markdown and plain text as plain text. The file
        // name decides only *how to read text already known to be text*, which
        // is a different question from what the file is — that was settled by
        // sniffing the bytes. Reading prose as Markdown would turn `a *b* c`
        // into emphasis the author never wrote.
        let blocks = if is_markdown(input.probe.file_name.as_deref()) {
            markdown_blocks(&text, &engine)
        } else {
            into_blocks(&text, &engine)
        };

        Ok(Document {
            source: input.source.clone(),
            pages: vec![Page {
                // A text file has no pages. One page with unknown geometry is
                // the honest representation; the IR is page-oriented because
                // every other format is.
                index: 0,
                size: None,
                blocks,
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

/// Decodes text, honouring a byte order mark.
///
/// Anything undecodable falls back to lossy UTF-8 rather than failing: for a
/// pass-through format, showing replacement characters beats refusing the file.
/// Legacy encodings such as GBK are not detected yet.
fn decode(bytes: &[u8]) -> String {
    const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
    const UTF16_LE_BOM: &[u8] = &[0xFF, 0xFE];
    const UTF16_BE_BOM: &[u8] = &[0xFE, 0xFF];

    if let Some(rest) = bytes.strip_prefix(UTF8_BOM) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(UTF16_LE_BOM) {
        return decode_utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(UTF16_BE_BOM) {
        return decode_utf16(rest, u16::from_be_bytes);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn decode_utf16(bytes: &[u8], to_unit: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| to_unit([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// Splits text into heading and paragraph blocks.
///
/// Line breaks inside a paragraph are kept. In CommonMark a single newline
/// renders as a space, so this is safe for wrapped prose while leaving lists
/// and preformatted text intact; joining the lines would corrupt both.
fn into_blocks(text: &str, engine: &EngineId) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph = String::new();
    let mut fence: Option<&str> = None;

    for line in text.lines() {
        if let Some(marker) = fence {
            paragraph.push_str(line);
            paragraph.push('\n');
            if line.trim_start().starts_with(marker) {
                fence = None;
            }
            continue;
        }

        if let Some(marker) = opening_fence(line) {
            flush(&mut paragraph, &mut blocks, engine);
            fence = Some(marker);
            paragraph.push_str(line);
            paragraph.push('\n');
            continue;
        }

        if line.trim().is_empty() {
            flush(&mut paragraph, &mut blocks, engine);
            continue;
        }

        // A heading only opens a block; inside a paragraph a leading `#` is
        // just text, which matches how Markdown itself behaves.
        if paragraph.is_empty()
            && let Some((level, title)) = atx_heading(line.trim())
        {
            blocks.push(Block::text(
                BlockKind::Heading { level },
                title,
                engine.clone(),
            ));
            continue;
        }

        paragraph.push_str(line);
        paragraph.push('\n');
    }

    flush(&mut paragraph, &mut blocks, engine);
    blocks
}

fn flush(paragraph: &mut String, blocks: &mut Vec<Block>, engine: &EngineId) {
    let text = std::mem::take(paragraph);
    let trimmed = text.trim_end();
    if !trimmed.is_empty() {
        blocks.push(Block::text(
            BlockKind::Paragraph,
            trimmed,
            engine.clone(),
        ));
    }
}

fn is_markdown(file_name: Option<&str>) -> bool {
    file_name.is_some_and(|name| {
        let lowered = name.to_ascii_lowercase();
        lowered.ends_with(".md") || lowered.ends_with(".markdown")
    })
}

/// Reads Markdown into blocks.
///
/// Parsing rather than guessing: the line-based reader below recognises
/// headings and paragraphs and nothing else, so a list would flatten into a
/// paragraph whose leading dash is then escaped to stop it becoming a list.
/// The text survives, but a note that goes in as Markdown comes out as escaped
/// prose — a loss the benchmark measured before anyone noticed it.
fn markdown_blocks(markdown: &str, engine: &EngineId) -> Vec<Block> {
    let arena = Arena::new();
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.front_matter_delimiter = Some("---".to_owned());

    let root = parse_document(&arena, markdown, &options);
    let mut blocks = Vec::new();

    for node in root.children() {
        convert(&node, engine, &mut blocks);
    }

    blocks
}

fn convert(node: &Node<'_>, engine: &EngineId, out: &mut Vec<Block>) {
    let kind = match node.data.borrow().value {
        NodeValue::Heading(ref heading) => Some(BlockKind::Heading {
            level: heading.level,
        }),
        NodeValue::Paragraph => Some(BlockKind::Paragraph),
        // Items live inside a list, so the list is what has to be walked; an
        // item is reached through it rather than on its own.
        NodeValue::List(..) => {
            for item in node.children() {
                if matches!(item.data.borrow().value, NodeValue::Item(..)) {
                    out.push(list_item(&item, engine));
                }
            }
            return;
        }
        NodeValue::Item(..) => {
            out.push(list_item(node, engine));
            return;
        }
        NodeValue::CodeBlock(ref code) => {
            // Code is not prose: it is emitted as written, without the
            // whitespace normalisation a paragraph gets.
            out.push(Block::text(
                BlockKind::Code,
                code.literal.trim_end(),
                engine.clone(),
            ));
            return;
        }
        NodeValue::BlockQuote => {
            let inner = convert_all(node, engine);
            let text = inner
                .iter()
                .map(|block| block.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            out.push(Block::text(BlockKind::Quote, text, engine.clone()));
            return;
        }
        NodeValue::Table(..) => {
            out.push(table(node, engine));
            return;
        }
        _ => None,
    };

    if let Some(kind) = kind {
        out.push(Block::text(kind, text_of(node), engine.clone()));
    }
}

fn convert_all(parent: &Node<'_>, engine: &EngineId) -> Vec<Block> {
    let mut blocks = Vec::new();
    for node in parent.children() {
        convert(&node, engine, &mut blocks);
    }
    blocks
}

fn list_item(node: &Node<'_>, engine: &EngineId) -> Block {
    let mut text = String::new();
    let mut children = Vec::new();

    for child in node.children() {
        match child.data.borrow().value {
            NodeValue::Paragraph if text.is_empty() => text = text_of(&child),
            NodeValue::List(..) => {
                for nested in child.children() {
                    children.push(list_item(&nested, engine));
                }
            }
            _ => {}
        }
    }

    Block {
        kind: BlockKind::ListItem,
        text,
        bbox: None,
        confidence: None,
        engine: engine.clone(),
        children,
    }
}

fn table(node: &Node<'_>, engine: &EngineId) -> Block {
    let rows = node
        .children()
        .filter(|child| matches!(child.data.borrow().value, NodeValue::TableRow(..)))
        .map(|row| Block {
            kind: BlockKind::TableRow,
            text: String::new(),
            bbox: None,
            confidence: None,
            engine: engine.clone(),
            children: row
                .children()
                .filter(|cell| matches!(cell.data.borrow().value, NodeValue::TableCell))
                .map(|cell| Block {
                    kind: BlockKind::TableCell,
                    text: text_of(&cell),
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

/// The text of a node's descendants.
///
/// Concatenated as written rather than joined with spaces, because a run
/// boundary is a formatting boundary: `**年**度报告` is one word split across
/// two runs, and a space between them would be a gap the document does not have.
fn text_of(node: &Node<'_>) -> String {
    let mut text = String::new();
    for descendant in node.descendants() {
        if let NodeValue::Text(ref value) = descendant.data.borrow().value {
            text.push_str(value);
        }
    }
    text.trim().to_owned()
}

/// Returns the fence marker if this line opens a fenced code block.
fn opening_fence(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    ["```", "~~~"]
        .into_iter()
        .find(|marker| trimmed.starts_with(marker))
}

/// Parses an ATX heading, returning its level and text.
fn atx_heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }

    let rest = &line[hashes..];
    if rest.is_empty() {
        return Some((hashes as u8, ""));
    }

    // The marker must be separated from its text, so `#tag` is not a heading.
    let content = rest.strip_prefix(' ')?;
    // A closing run of hashes is optional decoration.
    Some((
        hashes as u8,
        content.trim_end().trim_end_matches('#').trim_end(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn run(text: &str) -> Vec<Block> {
        into_blocks(text, &EngineId::from("test"))
    }

    fn kinds(blocks: &[Block]) -> Vec<&BlockKind> {
        blocks.iter().map(|block| &block.kind).collect()
    }

    #[test]
    fn blank_lines_separate_paragraphs() {
        let blocks = run("first\n\nsecond\n\n\nthird");
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].text, "first");
        assert_eq!(blocks[2].text, "third");
    }

    #[test]
    fn line_breaks_inside_a_paragraph_are_kept() {
        let blocks = run("one\ntwo\nthree");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text, "one\ntwo\nthree");
    }

    #[test]
    fn atx_headings_become_heading_blocks() {
        let blocks = run("# Title\n\nbody\n\n## Section");
        assert_eq!(
            kinds(&blocks),
            vec![
                &BlockKind::Heading { level: 1 },
                &BlockKind::Paragraph,
                &BlockKind::Heading { level: 2 },
            ]
        );
        assert_eq!(blocks[0].text, "Title");
    }

    #[test]
    fn a_hash_without_a_space_is_not_a_heading() {
        let blocks = run("#hashtag");
        assert_eq!(kinds(&blocks), vec![&BlockKind::Paragraph]);
    }

    #[test]
    fn a_hash_inside_a_paragraph_is_not_a_heading() {
        let blocks = run("intro\n# not a heading");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Paragraph);
    }

    #[test]
    fn seven_hashes_are_not_a_heading() {
        let blocks = run("####### too deep");
        assert_eq!(kinds(&blocks), vec![&BlockKind::Paragraph]);
    }

    #[test]
    fn closing_hashes_are_stripped_from_headings() {
        let blocks = run("## Title ##");
        assert_eq!(blocks[0].text, "Title");
    }

    #[test]
    fn headings_inside_a_fenced_block_are_left_alone() {
        let blocks = run("```\n# not a heading\n```");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Paragraph);
        assert!(blocks[0].text.contains("# not a heading"));
    }

    #[test]
    fn a_paragraph_after_a_fence_still_starts_cleanly() {
        let blocks = run("```\ncode\n```\n\nafter");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1].text, "after");
    }

    #[test]
    fn text_after_a_fence_is_a_new_block() {
        let blocks = run("intro\n```\ncode\n```");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].text, "intro");
    }

    #[test]
    fn empty_input_produces_no_blocks() {
        assert!(run("").is_empty());
        assert!(run("\n\n   \n").is_empty());
    }

    #[test]
    fn decodes_utf8_with_a_byte_order_mark() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("你好".as_bytes());
        assert_eq!(decode(&bytes), "你好");
    }

    #[test]
    fn decodes_utf16_little_endian() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "你好".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode(&bytes), "你好");
    }

    #[test]
    fn decodes_utf16_big_endian() {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in "你好".encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        assert_eq!(decode(&bytes), "你好");
    }

    /// Undecodable bytes degrade rather than aborting: for a pass-through
    /// format, replacement characters are better than a refusal.
    #[test]
    fn invalid_utf8_becomes_replacement_characters() {
        assert!(decode(&[0xE4, 0xBD, 0x28]).contains('\u{FFFD}'));
    }

    fn extract_named(bytes: &[u8], name: &str) -> Document {
        let probe = crate::extract::InputProbe::new(bytes, Some(name.to_owned()), bytes.len() as u64);
        let source = crate::ir::SourceRef {
            path: PathBuf::from(name),
            content_hash: "0".repeat(64),
            mime: "text/plain".into(),
            size: bytes.len() as u64,
            mtime: None,
        };

        TextPassthrough
            .extract(&Input {
                probe: &probe,
                bytes,
                source: &source,
            })
            .unwrap()
    }

    /// A list that goes in as Markdown must come out as a list. Reading it
    /// line by line flattens it into a paragraph whose leading dash is then
    /// escaped to stop it becoming a list, which loses the structure.
    #[test]
    fn markdown_lists_stay_lists() {
        let document = extract_named(b"- one\n- two\n", "notes.md");
        let kinds: Vec<&BlockKind> = document.blocks().map(|block| &block.kind).collect();

        assert_eq!(kinds, vec![&BlockKind::ListItem, &BlockKind::ListItem]);
    }

    #[test]
    fn nested_markdown_lists_keep_their_structure() {
        let document = extract_named(b"- parent\n  - child\n", "notes.md");
        let item = document.blocks().next().unwrap();

        assert_eq!(item.text, "parent");
        assert_eq!(item.children.len(), 1);
        assert_eq!(item.children[0].text, "child");
    }

    #[test]
    fn markdown_tables_become_table_blocks() {
        let document = extract_named(b"| a | b |\n| --- | --- |\n| c | d |\n", "notes.md");
        let table = document.blocks().next().unwrap();

        assert_eq!(table.kind, BlockKind::Table);
        assert_eq!(table.children.len(), 2);
        assert_eq!(table.children[0].children[1].text, "b");
    }

    /// Code is not prose, so it is kept as written.
    #[test]
    fn markdown_code_blocks_keep_their_text() {
        let document = extract_named(b"```\nfn main() {}\n```\n", "notes.md");
        let block = document.blocks().next().unwrap();

        assert_eq!(block.kind, BlockKind::Code);
        assert_eq!(block.text, "fn main() {}");
    }

    #[test]
    fn markdown_block_quotes_are_kept() {
        let document = extract_named(b"> quoted\n", "notes.md");
        assert_eq!(document.blocks().next().unwrap().kind, BlockKind::Quote);
    }

    /// Plain text is not Markdown. Reading it as Markdown would turn `a *b* c`
    /// into emphasis the author never wrote, and turn a line of dashes into a
    /// list.
    #[test]
    fn plain_text_is_not_parsed_as_markdown() {
        let document = extract_named(b"- not a list\n", "notes.txt");
        assert_eq!(
            document.blocks().next().unwrap().kind,
            BlockKind::Paragraph
        );
    }

    #[test]
    fn extracting_marks_the_page_as_unverified() {
        let probe = InputProbe::new(b"hello", None, 5);
        let source = crate::ir::SourceRef {
            path: PathBuf::from("notes.txt"),
            content_hash: "0".repeat(64),
            mime: "text/plain".into(),
            size: 5,
            mtime: None,
        };

        let document = TextPassthrough
            .extract(&Input {
                probe: &probe,
                bytes: b"hello",
                source: &source,
            })
            .unwrap();

        assert!(!document.pages[0].provenance.verified);
        assert_eq!(document.pages[0].provenance.primary.as_str(), "text-passthrough");
        assert!(document.pages[0].size.is_none());
    }
}
