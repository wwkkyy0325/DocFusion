//! What the two Office formats need in common: escaping, and the container both
//! of them are carried in.

use std::io::{Cursor, Write};

/// Text as XML character data, or as the value of an attribute.
///
/// Five characters, and every one of them is a way for a document's own words to
/// stop being words: `&` and `<` are how an element is written, `>` closes one
/// inside a comment, and a quote ends an attribute early. A cell holding `a & b`
/// written raw is a file Word refuses to open — and the text here is somebody
/// else's, read off a page by an engine, so it holds whatever was on that page.
///
/// **A control character is dropped rather than escaped**, because XML 1.0 has
/// no escape for one: a page that read a form feed puts a form feed in its text
/// and there is no way to write it. What is dropped is a character nobody could
/// have seen, and keeping it would cost the whole file.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // The three a document may hold.
            '\t' | '\n' | '\r' => out.push(character),
            control if control < ' ' => {}
            // Not characters in XML 1.0 either, and the only two that a valid
            // Rust string can hold.
            '\u{fffe}' | '\u{ffff}' => {}
            other => out.push(other),
        }
    }

    out
}

/// Files as a zip, which is what a `.docx` and an `.xlsx` both are.
///
/// **`[Content_Types].xml` first**, because it is the one part of an OPC package
/// whose position a reader is allowed to care about: Word and Excel both look
/// for it before anything else. The caller puts it first in the slice; this only
/// writes them in the order it is given.
pub fn zip(files: &[(&str, String)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));

    for (name, body) in files {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .expect("a name with no directory in it");
        writer
            .write_all(body.as_bytes())
            .expect("a String is always writable");
    }

    writer
        .finish()
        .expect("a Vec is always writable")
        .into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_five_characters_that_end_an_element_or_an_attribute_are_escaped() {
        assert_eq!(escape("a & b < c > d \" e ' f"), "a &amp; b &lt; c &gt; d &quot; e &apos; f");
    }

    /// A form feed is what a scanner reads off a page. There is no way to write
    /// one in XML 1.0, so it goes, and the file stays openable.
    #[test]
    fn a_character_xml_cannot_hold_is_dropped_rather_than_written() {
        assert_eq!(escape("a\u{0c}b\u{0}"), "ab");
        assert_eq!(escape("a\tb\nc\rd"), "a\tb\nc\rd", "the three that are legal stay");
    }

    #[test]
    fn the_files_are_a_zip_that_holds_what_went_in() {
        let bytes = zip(&[("[Content_Types].xml", "<Types/>".to_owned()), ("a/b.xml", "x".to_owned())]);

        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("a zip");
        assert_eq!(archive.len(), 2);
        assert_eq!(archive.by_name("a/b.xml").unwrap().size(), 1);
    }
}
