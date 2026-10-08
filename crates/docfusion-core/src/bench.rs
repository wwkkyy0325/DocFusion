//! Scoring an extraction against a reference.
//!
//! Both sides are Markdown, so both are read with a Markdown parser rather than
//! compared as text: what matters is the structure a reader would see, not
//! whether two renderings happen to use the same characters.
//!
//! The components are reported separately as well as combined, because a model
//! that reads every character and loses the section headings is not the same as
//! one that reads most characters and keeps them.

use comrak::nodes::NodeValue;
use comrak::{Arena, Options, parse_document};

/// How one engine did on one document.
#[derive(Debug, Clone, PartialEq)]
pub struct Score {
    /// Characters that would have to change to turn the output into the
    /// reference, as a fraction of the reference. `0.0` is exact.
    pub character_error_rate: f64,
    /// Reference headings found in the output, in the reference's order.
    pub headings_matched: usize,
    pub headings_total: usize,
    /// Reference tables found with the same shape, in order.
    pub tables_matched: usize,
    pub tables_total: usize,
}

impl Score {
    pub fn heading_accuracy(&self) -> f64 {
        ratio(self.headings_matched, self.headings_total)
    }

    pub fn table_accuracy(&self) -> f64 {
        ratio(self.tables_matched, self.tables_total)
    }

    /// A single number for ranking engines, lower being better.
    ///
    /// Weighted toward the failures that corrupt downstream use. A missing
    /// heading or a table of the wrong shape is worse than a scattering of
    /// character noise, because the text that survives still looks correct —
    /// which is exactly how a silent failure gets believed.
    pub fn loss(&self) -> f64 {
        0.5 * self.character_error_rate
            + 0.3 * (1.0 - self.heading_accuracy())
            + 0.2 * (1.0 - self.table_accuracy())
    }
}

/// A count that is vacuously perfect when there was nothing to find.
fn ratio(matched: usize, total: usize) -> f64 {
    if total == 0 {
        1.0
    } else {
        matched as f64 / total as f64
    }
}

pub fn score(expected: &str, actual: &str) -> Score {
    let expected = outline(expected);
    let actual = outline(actual);

    Score {
        character_error_rate: error_rate(&expected.text, &actual.text),
        headings_matched: ordered_matches(&expected.headings, &actual.headings),
        headings_total: expected.headings.len(),
        tables_matched: ordered_matches(&expected.tables, &actual.tables),
        tables_total: expected.tables.len(),
    }
}

/// The comparable content of a Markdown document.
#[derive(Debug, Default, PartialEq)]
struct Outline {
    text: String,
    headings: Vec<(u8, String)>,
    tables: Vec<(usize, usize)>,
}

fn outline(markdown: &str) -> Outline {
    let arena = Arena::new();
    let mut options = Options::default();
    options.extension.table = true;
    // The extracted output carries front matter. It is metadata rather than
    // content, so it must not count against a reference that has none.
    options.extension.front_matter_delimiter = Some("---".to_owned());

    let root = parse_document(&arena, markdown, &options);
    let mut outline = Outline::default();

    for node in root.descendants() {
        let heading_level = match node.data.borrow().value {
            NodeValue::Heading(ref heading) => Some(heading.level),
            _ => None,
        };
        if let Some(level) = heading_level {
            outline
                .headings
                .push((level, normalize(&descendant_text(&node))));
            continue;
        }

        let columns = match node.data.borrow().value {
            NodeValue::Table(ref table) => Some(table.alignments.len()),
            _ => None,
        };
        if let Some(columns) = columns {
            let rows = node
                .descendants()
                .filter(|child| matches!(child.data.borrow().value, NodeValue::TableRow(..)))
                .count();
            outline.tables.push((rows, columns));
            continue;
        }

        match node.data.borrow().value {
            NodeValue::Text(ref text) => outline.text.push_str(text),
            // A space separates blocks, so the end of one does not fuse onto
            // the start of the next. Text nodes within a block are simply
            // concatenated, because a run boundary is a *formatting* boundary,
            // not a word boundary: `**年**度报告` is one word split across two
            // runs, and inserting a space there would invent a gap that the
            // document does not have.
            NodeValue::Paragraph | NodeValue::Item(..) | NodeValue::CodeBlock(..) => {
                outline.text.push(' ');
            }
            _ => {}
        }
    }

    outline.text = normalize(&outline.text);
    outline
}

/// The text of a node's descendants, concatenated.
///
/// Heading text is scored separately from body text rather than folded into it,
/// so a lost heading is reported as a lost heading instead of as characters
/// that went missing somewhere.
fn descendant_text(node: &comrak::Node<'_>) -> String {
    let mut text = String::new();
    for descendant in node.descendants() {
        if let NodeValue::Text(ref value) = descendant.data.borrow().value {
            text.push_str(value);
        }
    }
    text
}

/// Collapses whitespace, so that reflowing a paragraph is not counted as an
/// error. CJK text has no spaces to collapse, so this leaves it alone.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn error_rate(expected: &str, actual: &str) -> f64 {
    let expected: Vec<char> = expected.chars().collect();
    if expected.is_empty() {
        return if actual.is_empty() { 0.0 } else { 1.0 };
    }

    let distance = trimmed_distance(&expected, &actual.chars().collect::<Vec<_>>());
    (distance as f64 / expected.len() as f64).min(1.0)
}

/// Edit distance after removing the common prefix and suffix.
///
/// The comparison is quadratic in the size of the differing region, and for a
/// working engine that region is small — a good extraction usually differs from
/// its reference in a handful of places. Trimming first is what keeps the cost
/// proportional to how wrong the engine was rather than to how long the
/// document is.
fn trimmed_distance(expected: &[char], actual: &[char]) -> usize {
    let prefix = expected
        .iter()
        .zip(actual)
        .take_while(|(a, b)| a == b)
        .count();

    let suffix = expected[prefix..]
        .iter()
        .rev()
        .zip(actual[prefix.min(actual.len())..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();

    levenshtein(
        &expected[prefix..expected.len() - suffix],
        &actual[prefix.min(actual.len())..actual.len().saturating_sub(suffix)],
    )
}

fn levenshtein(left: &[char], right: &[char]) -> usize {
    if left.is_empty() {
        return right.len();
    }
    if right.is_empty() {
        return left.len();
    }

    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0usize; right.len() + 1];

    for (row, left_character) in left.iter().enumerate() {
        current[0] = row + 1;
        for (column, right_character) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_character != right_character);
            current[column + 1] = substitution
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[right.len()]
}

/// How many of `expected` appear in `actual`, in order.
///
/// Order is part of the definition: a document whose sections are all present
/// but shuffled is not the same document, and the text still reads correctly,
/// which is what makes that failure hard to notice.
fn ordered_matches<T: PartialEq>(expected: &[T], actual: &[T]) -> usize {
    let mut cursor = 0;
    let mut matched = 0;

    for want in expected {
        let Some(offset) = actual[cursor..].iter().position(|have| have == want) else {
            continue;
        };
        cursor += offset + 1;
        matched += 1;
    }

    matched
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_documents_score_perfectly() {
        let markdown = "# Title\n\nSome body text.\n";

        let score = score(markdown, markdown);

        assert_eq!(score.character_error_rate, 0.0);
        assert_eq!(score.heading_accuracy(), 1.0);
        assert_eq!(score.loss(), 0.0);
    }

    #[test]
    fn a_few_wrong_characters_cost_little() {
        let score = score("the quick brown fox", "the quik brown fox");
        assert!(score.character_error_rate < 0.1, "{score:?}");
    }

    /// Reflowing a paragraph is not an error.
    #[test]
    fn whitespace_differences_are_ignored() {
        let score = score("one two three", "one\n\ntwo   three");
        assert_eq!(score.character_error_rate, 0.0);
    }

    #[test]
    fn front_matter_is_not_counted_as_content() {
        let expected = "# Title\n\nBody.\n";
        let actual = "---\ntitle: x\nsha256: abc\n---\n# Title\n\nBody.\n";

        assert_eq!(score(expected, actual).character_error_rate, 0.0);
    }

    #[test]
    fn headings_are_compared_with_their_level() {
        let score = score("# Title", "## Title");

        assert_eq!(score.headings_total, 1);
        assert_eq!(score.headings_matched, 0, "a demoted heading is not the same");
    }

    /// Present but shuffled is not the same, and the surviving text still reads
    /// correctly — which is what makes it hard to notice.
    ///
    /// The count is the longest run that is still in order, which is what
    /// matching each expected heading against the next one available gives: two
    /// of these three survive, because `One` and `Two` stay in sequence even
    /// though `Three` moved to the front.
    #[test]
    fn headings_out_of_order_do_not_match() {
        let expected = "# One\n\n# Two\n\n# Three\n";
        let actual = "# Three\n\n# One\n\n# Two\n";

        assert_eq!(score(expected, actual).headings_matched, 2);
    }

    #[test]
    fn reversing_the_reading_order_keeps_only_one_heading() {
        let expected = "# One\n\n# Two\n\n# Three\n";
        let actual = "# Three\n\n# Two\n\n# One\n";

        assert_eq!(score(expected, actual).headings_matched, 1);
    }

    #[test]
    fn a_document_with_no_headings_scores_perfectly_on_headings() {
        let score = score("just a paragraph", "just a paragraph");

        assert_eq!(score.headings_total, 0);
        assert_eq!(score.heading_accuracy(), 1.0);
    }

    #[test]
    fn a_missing_heading_is_counted() {
        let score = score("# One\n\n# Two\n", "# One\n");

        assert_eq!(score.headings_total, 2);
        assert_eq!(score.headings_matched, 1);
    }

    #[test]
    fn table_shape_is_compared() {
        let expected = "| a | b |\n| --- | --- |\n| c | d |\n";
        let same = expected;
        let wider = "| a | b | c |\n| --- | --- | --- |\n| d | e | f |\n";

        assert_eq!(score(expected, same).tables_matched, 1);
        assert_eq!(score(expected, wider).tables_matched, 0);
        assert_eq!(score(expected, wider).tables_total, 1);
    }

    #[test]
    fn a_table_that_became_a_paragraph_does_not_match() {
        let expected = "| a | b |\n| --- | --- |\n";
        let actual = "a b\n";

        assert_eq!(score(expected, actual).tables_matched, 0);
    }

    /// A missing table and a missing heading are worse than character noise,
    /// because what survives still looks right.
    #[test]
    fn structural_loss_outweighs_character_noise() {
        let noisy = score("# Title\n\nbody text here", "# Title\n\nbody taxt here");
        let structural = score("# Title\n\nbody text here", "body text here");

        assert!(
            structural.loss() > noisy.loss(),
            "structural {} should exceed noisy {}",
            structural.loss(),
            noisy.loss()
        );
    }

    #[test]
    fn an_empty_reference_is_satisfied_by_empty_output() {
        assert_eq!(score("", "").character_error_rate, 0.0);
        assert_eq!(score("", "something").character_error_rate, 1.0);
    }

    #[test]
    fn the_error_rate_is_capped_at_one() {
        let score = score("short", "a very much longer output indeed");
        assert!(score.character_error_rate <= 1.0, "{score:?}");
    }

    #[test]
    fn levenshtein_matches_known_values() {
        let distance = |a: &str, b: &str| {
            levenshtein(
                &a.chars().collect::<Vec<_>>(),
                &b.chars().collect::<Vec<_>>(),
            )
        };

        assert_eq!(distance("", ""), 0);
        assert_eq!(distance("abc", "abc"), 0);
        assert_eq!(distance("abc", "abd"), 1);
        assert_eq!(distance("abc", "ab"), 1);
        assert_eq!(distance("abc", ""), 3);
        assert_eq!(distance("kitten", "sitting"), 3);
    }

    /// Trimming must not change the answer, only the cost of reaching it.
    #[test]
    fn trimming_agrees_with_a_full_comparison() {
        let expected: Vec<char> = "the quick brown fox jumps".chars().collect();
        let actual: Vec<char> = "the quiet brown fox jumped".chars().collect();

        assert_eq!(trimmed_distance(&expected, &actual), levenshtein(&expected, &actual));
    }

    #[test]
    fn trimming_handles_completely_different_text() {
        let expected: Vec<char> = "aaa".chars().collect();
        let actual: Vec<char> = "bbb".chars().collect();

        assert_eq!(trimmed_distance(&expected, &actual), 3);
    }

    #[test]
    fn trimming_handles_one_side_being_a_prefix() {
        let expected: Vec<char> = "abcdef".chars().collect();
        let actual: Vec<char> = "abc".chars().collect();

        assert_eq!(trimmed_distance(&expected, &actual), 3);
    }
}
