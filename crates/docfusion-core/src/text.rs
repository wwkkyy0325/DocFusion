//! Putting two pieces of text together.
//!
//! One question, asked in two places for the same reason. A page is read in
//! pieces — runs of a line, lines of a block — and joining them is where the
//! answer matters, because the wrong one **invents a character the document
//! does not have**.
//!
//! A space belongs between two Latin words and not between two Chinese ones: a
//! break between 研究 and 取得 is where the column ran out, and writing 研究 取得
//! is a space the page never had. Measured on a newspaper page, five of its ten
//! headings were missed for exactly that reason and nothing else — the words
//! were right and the space between them was not.

/// Whether two pieces of text need a space between them when joined.
pub fn needs_space(left: &str, right: &str) -> bool {
    match (left.chars().next_back(), right.chars().next()) {
        (Some(a), Some(b)) => !is_cjk(a) && !is_cjk(b),
        _ => false,
    }
}

/// Whether a character is written without spaces around it.
pub fn is_cjk(character: char) -> bool {
    matches!(character as u32,
        0x3000..=0x303F     // CJK punctuation
        | 0x3040..=0x30FF   // kana
        | 0x3400..=0x4DBF   // CJK extension A
        | 0x4E00..=0x9FFF   // CJK unified ideographs
        | 0xF900..=0xFAFF   // compatibility ideographs
        | 0xFF00..=0xFFEF   // full-width forms
        | 0x20000..=0x2FA1F // extensions B and beyond
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin_words_are_separated() {
        assert!(needs_space("AND EXCHANGE", "COMMISSION"));
    }

    /// The whole reason this module exists: a Chinese line break is not a space.
    #[test]
    fn chinese_words_are_not() {
        assert!(!needs_space("我国大豆基因组研究", "取得重大突破"));
        assert!(!needs_space("洛阳市抗旱", "从种子抓起"));
    }

    /// And a break between the two scripts is not one either — which is the
    /// rule this project had already settled on for the runs of a line, kept
    /// rather than re-decided here.
    #[test]
    fn a_break_between_scripts_is_not_a_space() {
        assert!(!needs_space("研究", "abc"));
        assert!(!needs_space("abc", "研究"));
    }

    #[test]
    fn nothing_has_nothing_on_either_side_of_it() {
        assert!(!needs_space("", ""));
        assert!(!needs_space("", "COMMISSION"));
        assert!(!needs_space("AND", ""));
    }
}
