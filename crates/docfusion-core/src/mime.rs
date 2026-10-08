//! Content-based format detection.
//!
//! File extensions lie. A `.pdf` that is really a renamed JPEG is one of the
//! most common forms of input pollution, and routing on the extension sends it
//! to the wrong engine — which then either fails confusingly or, worse,
//! produces plausible garbage. Everything here reads bytes.

/// A coarse format family, decided from the bytes at the start of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Pdf,
    /// Any ZIP container. Covers docx, xlsx and pptx, which are told apart by
    /// looking inside the archive rather than by their name.
    Zip,
    Png,
    Jpeg,
    Tiff,
    Webp,
    /// An OLE2 compound document: the container used by legacy .doc/.xls/.ppt.
    Ole2,
    /// No magic number matched, but the bytes look like readable text.
    Text,
    Unknown,
}

impl Container {
    /// A coarse MIME type. Engines that can determine something more precise —
    /// docx versus xlsx, say — refine this in the document they return.
    pub fn mime_type(self) -> &'static str {
        match self {
            Self::Pdf => "application/pdf",
            Self::Zip => "application/zip",
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Tiff => "image/tiff",
            Self::Webp => "image/webp",
            Self::Ole2 => "application/x-ole-storage",
            Self::Text => "text/plain",
            Self::Unknown => "application/octet-stream",
        }
    }

    /// Whether this container holds an image, which needs an OCR or vision
    /// backend rather than a structural parser.
    pub fn is_image(self) -> bool {
        matches!(self, Self::Png | Self::Jpeg | Self::Tiff | Self::Webp)
    }
}

/// How many leading bytes are needed to classify every format we recognise.
pub const SNIFF_LENGTH: usize = 16;

/// Classifies the leading bytes of a file.
pub fn sniff(head: &[u8]) -> Container {
    if head.is_empty() {
        return Container::Unknown;
    }

    if has_bom(head) {
        return Container::Text;
    }
    if head.starts_with(b"%PDF-") {
        return Container::Pdf;
    }
    if head.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Container::Png;
    }
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Container::Jpeg;
    }
    // Little-endian and big-endian TIFF share the version number at offset 2.
    if head.starts_with(&[0x49, 0x49, 0x2A, 0x00]) || head.starts_with(&[0x4D, 0x4D, 0x00, 0x2A]) {
        return Container::Tiff;
    }
    if head.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return Container::Ole2;
    }
    if is_zip(head) {
        return Container::Zip;
    }
    if is_webp(head) {
        return Container::Webp;
    }
    if looks_like_text(head) {
        return Container::Text;
    }

    Container::Unknown
}

/// Every ZIP variant: a local file header, an empty archive, or a spanned one.
fn is_zip(head: &[u8]) -> bool {
    head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") || head.starts_with(b"PK\x07\x08")
}

fn is_webp(head: &[u8]) -> bool {
    head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP"
}

fn has_bom(head: &[u8]) -> bool {
    const UTF8: &[u8] = &[0xEF, 0xBB, 0xBF];
    const UTF16_LE: &[u8] = &[0xFF, 0xFE];
    const UTF16_BE: &[u8] = &[0xFE, 0xFF];

    // UTF-32LE also begins FF FE 00 00; both are text, so the order is harmless.
    head.starts_with(UTF8) || head.starts_with(UTF16_LE) || head.starts_with(UTF16_BE)
}

/// Conservative text detection: no control bytes, and either valid UTF-8 or
/// valid UTF-8 with a multi-byte character cut off by the end of the sample.
///
/// The truncation case matters because only a fixed-length prefix is examined,
/// and a CJK character is three bytes — a sample can easily split one.
fn looks_like_text(head: &[u8]) -> bool {
    if head.contains(&0) {
        return false;
    }
    if head.iter().any(|byte| *byte < 0x09 || (0x0E..0x20).contains(byte)) {
        return false;
    }
    match std::str::from_utf8(head) {
        Ok(_) => true,
        // `error_len` is None only when the input ended mid-character.
        Err(err) => err.error_len().is_none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_a_pdf() {
        assert_eq!(sniff(b"%PDF-1.7\n..."), Container::Pdf);
    }

    #[test]
    fn recognises_image_formats() {
        assert_eq!(
            sniff(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13]),
            Container::Png
        );
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Container::Jpeg);
        assert_eq!(sniff(&[0x49, 0x49, 0x2A, 0x00, 8, 0, 0, 0]), Container::Tiff);
        assert_eq!(sniff(b"RIFF\x24\x00\x00\x00WEBPVP8 "), Container::Webp);
    }

    /// Every Office document, and any zip, starts the same way.
    #[test]
    fn recognises_a_zip_container() {
        assert_eq!(sniff(b"PK\x03\x04\x14\x00\x00\x00"), Container::Zip);
        assert_eq!(sniff(b"PK\x05\x06\x00\x00"), Container::Zip);
    }

    /// Legacy Office files must be told apart from their modern counterparts,
    /// because the right response is a clear error rather than a failed parse.
    #[test]
    fn recognises_a_legacy_office_container() {
        let ole2 = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0];
        assert_eq!(sniff(&ole2), Container::Ole2);
    }

    #[test]
    fn recognises_plain_utf8_text() {
        assert_eq!(sniff(b"hello world\n"), Container::Text);
    }

    #[test]
    fn recognises_text_with_a_byte_order_mark() {
        assert_eq!(sniff(&[0xEF, 0xBB, 0xBF, b'h', b'i']), Container::Text);
        assert_eq!(sniff(&[0xFF, 0xFE, b'h', 0]), Container::Text);
    }

    /// A CJK character is three bytes, so a truncated sample is the normal
    /// case rather than an edge case, and must not be mistaken for binary.
    #[test]
    fn a_truncated_multibyte_character_is_still_text() {
        let mut head = "年度报告的".as_bytes().to_vec();
        head.truncate(head.len() - 1);
        assert_eq!(sniff(&head), Container::Text);
    }

    #[test]
    fn binary_data_is_not_text() {
        assert_eq!(sniff(&[0x00, 0x01, 0x02, 0x03]), Container::Unknown);
        assert_eq!(sniff(&[0xFF, 0x00, 0xFE, 0x00]), Container::Unknown);
    }

    #[test]
    fn an_empty_input_is_unknown() {
        assert_eq!(sniff(b""), Container::Unknown);
    }

    /// The whole point: the extension is not consulted, so a renamed file is
    /// classified by what it actually is.
    #[test]
    fn a_renamed_image_is_not_a_pdf() {
        let jpeg_named_pdf = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        assert_eq!(sniff(&jpeg_named_pdf), Container::Jpeg);
    }
}
