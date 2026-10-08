//! Pass-through for inputs that are already text: `.txt` and `.md`.
//!
//! This engine exists mostly so the pipeline has one shape for every input. It
//! does not attempt to be a Markdown parser — it recognises headings and splits
//! on blank lines, which is what carries over into the output.

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

        Ok(Document {
            source: input.source.clone(),
            pages: vec![Page {
                // A text file has no pages. One page with unknown geometry is
                // the honest representation; the IR is page-oriented because
                // every other format is.
                index: 0,
                size: None,
                blocks: into_blocks(&text, &engine),
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
