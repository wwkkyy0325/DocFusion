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

// ── finding a document again ─────────────────────────────────────────────────

/// The text as the index stores it: one token per Chinese character, and Latin
/// words left whole.
///
/// **Why the characters are pulled apart.** FTS5 has no Chinese tokenizer. Its
/// default splits on anything that is not alphanumeric, and a Chinese character
/// *is* alphanumeric to it, so a whole clause becomes a single token and
/// searching for 合同 inside 产品供销合同 finds nothing. Splitting them apart
/// before storing makes every character a token, and then a two-character word
/// is two adjacent tokens, which a phrase query finds.
///
/// **The obvious alternative was measured and rejected.** `tokenize='trigram'`
/// looks like the answer — it is built in, it handles any script, and it makes
/// a substring match an index lookup. It also **matches nothing shorter than
/// three characters**: on a test corpus, 合同, 供销, 学生 and 报到 all returned
/// zero while 产品供销合同 and 24年 returned one. Most Chinese words are two
/// characters, so the tokenizer that handles "any script" handles this
/// knowledge base's script not at all.
///
/// This is also smaller than trigram, not larger: one token per character
/// against roughly three.
///
/// The cost is that it is not reversible — see [`excerpt`], which is why the
/// text it marks up is never the text that is stored here.
pub fn indexed(text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    for character in text.chars() {
        if is_cjk(character) {
            out.push(' ');
            out.push(character);
            out.push(' ');
        } else {
            out.push(character);
        }
    }

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a person typed, as FTS5 should be asked for it.
///
/// Every term becomes a quoted phrase, in the same shape [`indexed`] wrote it,
/// so 合同 asks for the two adjacent tokens `合` and `同`. Quoting is also what
/// keeps the input safe: FTS5's query language has operators in it — `*`, `-`,
/// `(`, `"` — and a person typing `C++` into a search box is not writing a
/// query. A double quote inside a term is doubled, which is how the language
/// escapes one.
///
/// Terms are separated by spaces, which FTS5 reads as `AND`, so `合同 甲方`
/// finds documents holding both and not documents holding either.
pub fn as_query(input: &str) -> String {
    input
        .split_whitespace()
        .map(|term| format!("\"{}\"", indexed(term).replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The words a person typed, as they typed them.
///
/// What [`as_query`] turns into a query, kept separately because the two are
/// used against different strings: the query goes to the index, which holds the
/// pulled-apart text, and these go to the document, which holds the original.
pub fn terms(input: &str) -> Vec<String> {
    input
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>()
}

/// How much of the document to show around the first thing that matched.
///
/// Generous enough to read the sentence a word sits in, short enough that a
/// list of them is a list rather than a wall. Counted in characters, because
/// counting in bytes would give a Chinese excerpt a third of the room.
const EXCERPT: usize = 90;

/// A window of `body` around the first place one of `terms` appears, with the
/// match between [`HIT_START`] and [`HIT_END`].
///
/// **Why the excerpt is found here rather than by SQL.** FTS5 can do it, and it
/// would be reading the text the index holds — which is the text [`indexed`]
/// pulled apart, so the passage would come back with a space between every pair
/// of Chinese characters. There is no way back from that: the markers that say
/// where the match is get in the way of putting the characters together again,
/// and the result is a sentence with a space in the wrong places, which is
/// exactly the fault this project has already chased once.
///
/// `None` when no term is in the body, which is a thing that happens: the index
/// is a cache, and a document edited since it was written can match on words it
/// no longer holds.
pub fn excerpt(body: &str, terms: &[String]) -> Option<String> {
    let (at, length) = terms
        .iter()
        .filter_map(|term| find(body, term).map(|at| (at, term.chars().count())))
        .min_by_key(|(at, _)| *at)?;

    // Whole characters, so that a window never cuts one in half.
    let characters: Vec<char> = body.chars().collect();
    let start = body[..at].chars().count();
    let end = start + length;

    // Whether anything was cut is asked of the window before its edges are
    // tidied, not after: a body that begins with the blank lines of a paragraph
    // break had nothing cut from the front, and an ellipsis there would be
    // saying something was left out when nothing was.
    let from = start.saturating_sub(EXCERPT / 2);
    let to = (end + EXCERPT / 2).min(characters.len());

    // The edges themselves are whitespace only if the window happened to land
    // on some: a paragraph break inside the window shows up as a blank line in
    // a list of results, which reads as a gap somebody left rather than as text.
    let mut shown_from = from;
    while shown_from < start && characters[shown_from].is_whitespace() {
        shown_from += 1;
    }

    let mut shown_to = to;
    while shown_to > end && characters[shown_to - 1].is_whitespace() {
        shown_to -= 1;
    }

    let mut out = String::new();
    if from > 0 {
        out.push('…');
    }
    out.extend(characters[shown_from..start].iter());
    out.push(HIT_START);
    out.extend(characters[start..end.min(characters.len())].iter());
    out.push(HIT_END);
    out.extend(characters[end.min(characters.len())..shown_to].iter());
    if to < characters.len() {
        out.push('…');
    }

    Some(out)
}

/// Wrapped around the matching words in an excerpt.
///
/// Control characters, which is the point: they cannot occur in a document,
/// because a document is text off a page. A pair of brackets or asterisks could
/// be typed by whoever wrote it, and then the window would be marking up a
/// match nobody made.
pub const HIT_START: char = '\u{2}';
pub const HIT_END: char = '\u{3}';

/// Where `needle` first appears in `hay`, ignoring case for Latin and not for
/// anything else.
///
/// Chinese has no case, and neither do most of the scripts this reads; folding
/// everything would be a Unicode case mapping applied to text that then has to
/// be indexed back into the original, which is where offsets start disagreeing
/// with the string. An ASCII-only fold cannot do that: an ASCII byte is never
/// part of a multi-byte character, so a match found in bytes is a match found
/// at a character boundary.
fn find(hay: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }

    if needle.is_ascii() {
        return hay
            .as_bytes()
            .windows(needle.len())
            .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()));
    }

    hay.find(needle)
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

    // ── the index ────────────────────────────────────────────────────────────

    /// Every Chinese character is a token of its own, because that is the only
    /// way a two-character word can be found.
    #[test]
    fn chinese_is_pulled_apart_and_latin_is_not() {
        assert_eq!(indexed("产品供销合同"), "产 品 供 销 合 同");
        assert_eq!(indexed("openEuler 教程"), "openEuler 教 程");
        assert_eq!(indexed("合同 A4 纸"), "合 同 A4 纸");
        assert_eq!(indexed(""), "");
    }

    /// Punctuation and full-width forms are handled like any other character
    /// without a space around it, which means they become tokens too — and that
    /// is right: 第1条 and 第一条 should both be findable.
    #[test]
    fn full_width_and_punctuation_are_characters_like_any_other() {
        assert_eq!(indexed("（一）"), "（ 一 ）");
        assert_eq!(indexed("a,b"), "a,b");
    }

    /// Each term is a phrase, so a two-character word means two characters next
    /// to each other — not two characters somewhere in the document.
    #[test]
    fn every_term_becomes_a_phrase() {
        assert_eq!(as_query("合同"), "\"合 同\"");
        assert_eq!(as_query("openEuler"), "\"openEuler\"");
        assert_eq!(as_query("合同 甲方"), "\"合 同\" \"甲 方\"");
        assert_eq!(as_query("   "), "");
    }

    /// Nobody typing into a search box is writing a query in FTS5's language,
    /// and its language has operators in it.
    #[test]
    fn what_a_person_types_is_not_run_as_a_query() {
        assert_eq!(as_query("C++"), "\"C++\"");
        assert_eq!(as_query("a*"), "\"a*\"");
        assert_eq!(as_query("say \"hello\""), "\"say\" \"\"\"hello\"\"\"");
    }

    #[test]
    fn a_term_is_what_was_typed() {
        assert_eq!(terms("合同 甲方"), vec!["合同", "甲方"]);
        assert!(terms("  ").is_empty());
    }

    // ── the excerpt ──────────────────────────────────────────────────────────

    /// The window marks the match and leaves the text around it as it was —
    /// which is the whole reason it is found here and not by the index.
    #[test]
    fn an_excerpt_marks_the_match_in_the_original_text() {
        let body = "本合同由甲方与乙方于二〇二四年签订，双方约定如下条款。";
        let marked = excerpt(body, &terms("甲方")).expect("a match");

        assert!(marked.contains(&format!("{HIT_START}甲方{HIT_END}")), "{marked}");
        // Once the markers are taken out, what is left reads as the document
        // does — no space invented between two Chinese characters.
        let shown: String = marked.chars().filter(|c| *c != HIT_START && *c != HIT_END).collect();
        assert!(body.contains(shown.trim_matches('…')), "{shown}");
    }

    #[test]
    fn latin_matches_without_regard_to_case() {
        let marked = excerpt("a report by OPENeuler", &terms("openEuler")).expect("a match");

        assert!(marked.contains(&format!("{HIT_START}OPENeuler{HIT_END}")), "{marked}");
    }

    /// The first match in the document wins, so that the same search shows the
    /// same passage every time rather than whichever term was looked for first.
    #[test]
    fn the_earliest_match_wins() {
        let body = "甲方在第一条约定，乙方在第二条约定。";
        let marked = excerpt(body, &terms("乙方 甲方")).expect("a match");

        assert!(marked.starts_with(&HIT_START.to_string()), "should begin at 甲方: {marked}");
    }

    /// A long document is cut down, and the cut is said rather than implied.
    #[test]
    fn a_long_document_is_trimmed_and_says_so() {
        let body = format!("{}合同{}", "前".repeat(300), "后".repeat(300));
        let marked = excerpt(&body, &terms("合同")).expect("a match");

        assert!(marked.starts_with('…'), "{marked}");
        assert!(marked.ends_with('…'), "{marked}");
        assert!(marked.chars().count() < 200, "{}", marked.chars().count());
    }

    /// A short document is not padded out to the window, and does not gain an
    /// ellipsis at an end nothing was cut from.
    #[test]
    fn a_short_document_is_shown_whole() {
        let marked = excerpt("甲方", &terms("甲方")).expect("a match");

        assert_eq!(marked, format!("{HIT_START}甲方{HIT_END}"));
    }

    /// The index is a cache over the tree, so a document edited since it was
    /// written can match on a word it no longer holds. That is a miss, not a
    /// panic and not an empty excerpt with nothing in it.
    #[test]
    fn a_term_that_is_not_there_has_no_excerpt() {
        assert!(excerpt("甲方乙方", &terms("丙方")).is_none());
        assert!(excerpt("甲方乙方", &[]).is_none());
    }

    /// A window is cut on character boundaries, so an excerpt of a Chinese
    /// document is not a third of the length of one of an English document.
    #[test]
    fn the_window_is_measured_in_characters() {
        let body = format!("{}合同{}", "字".repeat(200), "字".repeat(200));
        let marked = excerpt(&body, &terms("合同")).expect("a match");

        // The window either side of the two characters that matched, plus the
        // markers around it and the two ellipses saying it was cut.
        assert_eq!(
            marked.chars().count(),
            (EXCERPT / 2) * 2 + 2 + 2 + 2,
            "{marked}"
        );
    }

    /// A window that lands on a paragraph break does not show the break.
    ///
    /// In a list of results a blank line reads as a gap somebody left rather
    /// than as text, and the window is over a line break, not over a paragraph.
    #[test]
    fn a_window_does_not_begin_or_end_on_whitespace() {
        let body = "\n\n\n合同\n\n";
        let marked = excerpt(body, &terms("合同")).expect("a match");

        assert_eq!(marked, format!("{HIT_START}合同{HIT_END}"), "{marked:?}");
    }
}
