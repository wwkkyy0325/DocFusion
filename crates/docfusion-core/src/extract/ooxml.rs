//! Shared machinery for OOXML formats.
//!
//! docx, xlsx and pptx are all ZIP archives of XML parts. Opening the archive
//! and turning a part into a tree is the same work every time, so it lives here
//! rather than being repeated per format with its own subtly different state
//! machine.
//!
//! Helpers return plain strings as errors and let each extractor attach its own
//! engine identity, since the same malformed XML means something different
//! depending on which format was being read.

use std::io::{Cursor, Read};

use quick_xml::Reader;
use quick_xml::events::{BytesRef, BytesStart, Event};
use zip::ZipArchive;

pub type Archive<'a> = ZipArchive<Cursor<&'a [u8]>>;

/// Opens an OOXML container.
pub fn open(bytes: &[u8]) -> Result<Archive<'_>, String> {
    ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("could not read the archive: {error}"))
}

/// Reads a part, or `None` when the archive has no such part.
///
/// `None` is how a docx parser discovers it is actually looking at an xlsx:
/// the parts present are what identifies the format, not the file name.
pub fn entry(archive: &mut Archive<'_>, name: &str) -> Result<Option<String>, String> {
    if !archive.file_names().any(|entry| entry == name) {
        return Ok(None);
    }

    let mut file = archive
        .by_name(name)
        .map_err(|error| format!("could not read {name}: {error}"))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| format!("{name} is not readable text: {error}"))?;

    Ok(Some(text))
}

/// Part names in the archive, sorted so that processing order is stable.
pub fn part_names(archive: &Archive<'_>, prefix: &str) -> Vec<String> {
    let mut names: Vec<String> = archive
        .file_names()
        .filter(|name| name.starts_with(prefix))
        .map(str::to_owned)
        .collect();
    names.sort();
    names
}

/// An element of a parsed XML document.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    /// The local name, with any namespace prefix removed.
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub children: Vec<Element>,
    /// Character data directly inside this element.
    pub text: String,
}

impl Element {
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|child| child.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |child| child.name == name)
    }

    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The first matching element in document order, at any depth.
    pub fn descendant(&self, name: &str) -> Option<&Element> {
        for child in &self.children {
            if child.name == name {
                return Some(child);
            }
            if let Some(found) = child.descendant(name) {
                return Some(found);
            }
        }
        None
    }

    /// Every matching element in document order, at any depth, excluding self.
    pub fn descendants_named<'a>(&'a self, name: &str, found: &mut Vec<&'a Element>) {
        for child in &self.children {
            if child.name == name {
                found.push(child);
            }
            child.descendants_named(name, found);
        }
    }
}

/// Parses a part into a tree.
///
/// A tree rather than a streaming visitor because both formats interleave
/// unrelated elements — property blocks sit alongside content — and the
/// extraction logic reads far more clearly when it can ask for a child by name.
pub fn parse(xml: &str) -> Result<Element, String> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => stack.push(element_from(&start)),
            Ok(Event::Empty(empty)) => {
                let node = element_from(&empty);
                attach(&mut stack, &mut root, node);
            }
            Ok(Event::Text(text)) => {
                if let Some(open) = stack.last_mut() {
                    open.text.push_str(&text.into_inner());
                }
            }
            // Entity references arrive as their own event rather than being
            // folded into the surrounding text. Ignoring them silently deletes
            // every `&` and `<` from the document, which is how "A & B" turns
            // into "A  B" — so they are resolved here.
            Ok(Event::GeneralRef(reference)) => {
                if let Some(open) = stack.last_mut() {
                    open.text.push_str(&resolve_reference(&reference));
                }
            }
            Ok(Event::CData(data)) => {
                if let Some(open) = stack.last_mut() {
                    open.text.push_str(&data.into_inner());
                }
            }
            Ok(Event::End(_)) => {
                if let Some(node) = stack.pop() {
                    attach(&mut stack, &mut root, node);
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("malformed XML: {error}")),
            _ => {}
        }
    }

    root.ok_or_else(|| "the XML part is empty".to_owned())
}

fn attach(stack: &mut [Element], root: &mut Option<Element>, node: Element) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(node),
        None => *root = Some(node),
    }
}

fn element_from(start: &BytesStart<'_>) -> Element {
    let attributes = start
        .attributes()
        .filter_map(Result::ok)
        .map(|attribute| {
            (
                local_name(attribute.key.as_ref()).to_owned(),
                unescape_attribute(&attribute.value),
            )
        })
        .collect();

    Element {
        name: start.local_name().as_ref().to_owned(),
        attributes,
        children: Vec::new(),
        text: String::new(),
    }
}

/// Resolves entity references in an attribute value.
///
/// Attribute values arrive with their entity references intact, unlike text,
/// which the reader splits into separate events.
fn unescape_attribute(raw: &str) -> String {
    quick_xml::escape::unescape(raw)
        .map(|value| value.into_owned())
        .unwrap_or_else(|_| raw.to_owned())
}

/// Turns an entity reference into the text it stands for.
///
/// A fixed table is complete rather than merely convenient: XML predefines
/// exactly five entities, numeric character references cover everything else,
/// and OOXML documents carry no DTD that could define more. An unrecognised
/// reference is kept verbatim, because dropping it would silently delete text.
fn resolve_reference(reference: &BytesRef<'_>) -> String {
    let name = reference.xml10_content();

    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hexadecimal) => u32::from_str_radix(hexadecimal, 16).ok(),
            None => number.parse::<u32>().ok(),
        };
        return code
            .and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_else(|| format!("&{name};"));
    }

    match name.as_ref() {
        "amp" => "&".to_owned(),
        "lt" => "<".to_owned(),
        "gt" => ">".to_owned(),
        "quot" => "\"".to_owned(),
        "apos" => "'".to_owned(),
        _ => format!("&{name};"),
    }
}

/// Drops the namespace prefix from a qualified name.
///
/// OOXML uses a stable prefix per namespace across producers, and every lookup
/// in this module is by local name, so resolving prefixes properly would add
/// machinery without changing any answer.
fn local_name(raw: &str) -> &str {
    match raw.rsplit_once(':') {
        Some((_, local)) => local,
        None => raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_elements_and_attributes() {
        let root = parse(r#"<w:body><w:p w:val="x">hello</w:p></w:body>"#).unwrap();

        assert_eq!(root.name, "body");
        let paragraph = root.child("p").expect("paragraph");
        assert_eq!(paragraph.text, "hello");
        assert_eq!(paragraph.attribute("val"), Some("x"));
    }

    #[test]
    fn keeps_text_that_spans_escapes_and_entities() {
        let root = parse("<p>a &amp; b &#65;</p>").unwrap();
        assert_eq!(root.text, "a & b A");
    }

    #[test]
    fn character_data_is_treated_as_text() {
        let root = parse("<p><![CDATA[a < b & c]]></p>").unwrap();
        assert_eq!(root.text, "a < b & c");
    }

    #[test]
    fn resolves_both_numeric_reference_forms() {
        let root = parse("<p>&#65;&#x42;&#x4e2d;</p>").unwrap();
        assert_eq!(root.text, "AB中");
    }

    /// Dropping an unrecognised reference would silently delete text, so it is
    /// kept as written.
    #[test]
    fn an_unknown_entity_is_preserved_verbatim() {
        let root = parse("<p>a &nbsp; b</p>").unwrap();
        assert_eq!(root.text, "a &nbsp; b");
    }

    #[test]
    fn attribute_values_have_their_entities_resolved() {
        let root = parse(r#"<p w:val="a &amp; b"/>"#).unwrap();
        assert_eq!(root.attribute("val"), Some("a & b"));
    }

    #[test]
    fn empty_elements_are_recorded() {
        let root = parse(r#"<p><br/><t>x</t></p>"#).unwrap();
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.child("br").unwrap().name, "br");
    }

    #[test]
    fn finds_descendants_at_any_depth_in_document_order() {
        let root = parse("<a><b><c>1</c></b><c>2</c></a>").unwrap();
        let mut found = Vec::new();
        root.descendants_named("c", &mut found);

        let texts: Vec<&str> = found.iter().map(|node| node.text.as_str()).collect();
        assert_eq!(texts, vec!["1", "2"]);
    }

    #[test]
    fn child_lookup_is_not_recursive() {
        let root = parse("<a><b><c/></b></a>").unwrap();
        assert!(root.child("c").is_none());
        assert!(root.descendant("c").is_some());
    }

    #[test]
    fn malformed_xml_is_reported_rather_than_guessed_at() {
        assert!(parse("<a><b></a>").is_err());
    }

    #[test]
    fn an_empty_document_is_an_error() {
        assert!(parse("").is_err());
    }
}
