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
    /// The same, with reading order taken out of the comparison.
    ///
    /// The ordered rate conflates two failures that need different answers. An
    /// engine that reads every character of a multi-column page correctly but
    /// lays it out across the columns instead of down them scores as badly as
    /// one that read nothing — and those are not the same problem, or the same
    /// size of problem. The gap between the two rates is what ordering cost,
    /// and it is the number to look at when a page reads well but scores badly.
    pub content_error_rate: f64,
    /// Reference headings found in the output, in the reference's order.
    pub headings_matched: usize,
    pub headings_total: usize,
    /// Reference tables found with the same shape, in order.
    pub tables_matched: usize,
    pub tables_total: usize,
    /// How alike the tables are in what they hold, from none to identical.
    ///
    /// Beside the count above rather than instead of it, because they answer
    /// two questions and a table can pass one and fail the other. Two tables of
    /// fifteen rows by three columns with not one word in common are found by
    /// the count and worth almost nothing here.
    pub table_similarity: f64,
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

pub fn score(expected_raw: &str, actual_raw: &str) -> Score {
    let expected = outline(expected_raw);
    let actual = outline(actual_raw);

    // Compared by shape and not by what is in them, which is what this count
    // has always meant. What is in them is the similarity beside it.
    let wanted: Vec<(usize, usize)> = expected.tables.iter().map(Table::shape).collect();
    let produced: Vec<(usize, usize)> = actual.tables.iter().map(Table::shape).collect();

    Score {
        character_error_rate: error_rate(&expected.text, &actual.text),
        content_error_rate: error_rate(&sorted(&expected.text), &sorted(&actual.text)),
        headings_matched: ordered_matches(&expected.headings, &actual.headings),
        headings_total: expected.headings.len(),
        tables_matched: ordered_matches(&wanted, &produced),
        tables_total: expected.tables.len(),
        table_similarity: table_similarity(expected_raw, actual_raw),
    }
}

/// A document's characters in a canonical order, so that comparing two of them
/// ignores which came first.
fn sorted(text: &str) -> String {
    let mut characters: Vec<char> = text.chars().collect();
    characters.sort_unstable();
    characters.into_iter().collect()
}

/// The comparable content of a Markdown document.
#[derive(Debug, Default, PartialEq)]
struct Outline {
    text: String,
    /// The headings, in the reference's order, **without their depth**.
    ///
    /// Depth is deliberately absent. Every one of the corpus's sixty-four
    /// references is a level-one heading — not one of them uses `##` — because
    /// OmniDocBench's ground truth records that a block is a title, not how deep
    /// a title it is, and the conversion to Markdown had to pick a number. A
    /// reference that cannot express a depth cannot measure one, and comparing
    /// two of them scored a correct section heading as a miss: measured, 8 of 64
    /// with the level compared and 21 of 64 without it.
    headings: Vec<String>,
    tables: Vec<Table>,
}

/// A table, as a comparison needs one: its cells, row by row.
///
/// The shape was all this kept, and a shape is a poor description of a table —
/// two tables of fifteen rows by eleven columns with nothing in common share
/// one, and both count as found. The cells are kept so that the distance below
/// has something to be a distance between.
#[derive(Debug, Default, PartialEq)]
struct Table {
    rows: Vec<Vec<String>>,
}

impl Table {
    fn shape(&self) -> (usize, usize) {
        (
            self.rows.len(),
            self.rows.iter().map(|row| row.len()).max().unwrap_or(0),
        )
    }

    /// The table as an ordered labelled tree: a table, its rows, their cells.
    ///
    /// Three levels, because that is what a Markdown table is. A cell's label
    /// is its own text, so a cell that says the wrong thing is one node away
    /// from the cell that says the right one.
    fn tree(&self) -> Tree {
        let mut tree = Tree::default();
        let root = tree.add("table".to_owned());

        for row in &self.rows {
            let at = tree.add("tr".to_owned());
            let mut children = Vec::with_capacity(row.len());

            for cell in row {
                children.push(tree.add(format!("td:{}", cell.trim())));
            }

            tree.children[at] = children;
            tree.children[root].push(at);
        }

        tree
    }
}

/// An ordered, labelled tree, which is what a table edit distance is a distance
/// between.
#[derive(Debug, Default)]
struct Tree {
    labels: Vec<String>,
    children: Vec<Vec<usize>>,
}

impl Tree {
    fn add(&mut self, label: String) -> usize {
        self.labels.push(label);
        self.children.push(Vec::new());
        self.labels.len() - 1
    }

    /// The nodes in post-order: children before their parent, left before right.
    fn postorder(&self) -> Vec<usize> {
        let mut order = Vec::with_capacity(self.labels.len());
        if self.labels.is_empty() {
            return order;
        }

        let mut stack = vec![(0usize, false)];
        while let Some((node, seen)) = stack.pop() {
            if seen {
                order.push(node);
                continue;
            }

            stack.push((node, true));
            for child in self.children[node].iter().rev() {
                stack.push((*child, false));
            }
        }

        order
    }

    /// For each node, the post-order position of its own leftmost leaf.
    ///
    /// A leaf is its own; anything else is its first child's. That is what
    /// makes a subtree a contiguous run of post-order positions, which is the
    /// property the algorithm below is built on.
    fn leftmost(&self, positions: &[usize]) -> Vec<usize> {
        fn walk(tree: &Tree, node: usize, positions: &[usize], out: &mut [usize]) -> usize {
            // Every child has to be walked and not only the first: a node's
            // leftmost leaf comes from its first child, but its second child is
            // a subtree whose own positions need filling in. Following only the
            // first left every node that is not on the leftmost path with a
            // zero, which the distance below then subtracted from.
            let mut leftmost = positions[node];

            for (index, child) in tree.children[node].iter().enumerate() {
                let below = walk(tree, *child, positions, out);
                if index == 0 {
                    leftmost = below;
                }
            }

            out[positions[node]] = leftmost;
            leftmost
        }

        let mut out = vec![0usize; self.labels.len() + 1];
        if !self.labels.is_empty() {
            walk(self, 0, positions, &mut out);
        }

        out
    }
}

/// The positions that have to be visited for every subtree to be covered.
///
/// Every node whose leftmost leaf is a given one ends a prefix of the same run,
/// so only the largest of them has to be computed and the rest fall out of it.
///
/// **Sorted, and not merely collected.** The order matters: a subtree's
/// distance is read out of the table while a larger one that contains it is
/// being computed, so the smaller has to have been done first. Collected in the
/// order the leftmost leaves happen to run in, the root came out before its own
/// rows and every distance measured against an empty table — two documents with
/// no table in common scored 0.98.
fn keyroots(leftmost: &[usize]) -> Vec<usize> {
    let mut largest = std::collections::BTreeMap::new();

    for (position, leaf) in leftmost.iter().enumerate().skip(1) {
        largest.insert(*leaf, position);
    }

    let mut roots: Vec<usize> = largest.into_values().collect();
    roots.sort_unstable();
    roots
}

/// The tree edit distance between two ordered labelled trees.
///
/// Zhang and Shasha's algorithm: a node may be deleted, inserted, or renamed,
/// each at a cost of one, and renaming two nodes that say the same thing costs
/// nothing. Every pair of keyroots is visited and the forests between them are
/// compared, which is what keeps this off the square-of-the-square path.
fn tree_distance(left: &Tree, right: &Tree) -> f64 {
    let lorder = left.postorder();
    let rorder = right.postorder();
    let (n, m) = (lorder.len(), rorder.len());

    if n == 0 || m == 0 {
        return n.max(m) as f64;
    }

    let mut lpos = vec![0usize; left.labels.len()];
    for (index, node) in lorder.iter().enumerate() {
        lpos[*node] = index + 1;
    }

    let mut rpos = vec![0usize; right.labels.len()];
    for (index, node) in rorder.iter().enumerate() {
        rpos[*node] = index + 1;
    }

    let lleaf = left.leftmost(&lpos);
    let rleaf = right.leftmost(&rpos);

    let mut distance = vec![vec![0.0f64; m + 1]; n + 1];

    for i in keyroots(&lleaf) {
        for j in keyroots(&rleaf) {
            let across = i - lleaf[i] + 1;
            let down = j - rleaf[j] + 1;
            let mut forest = vec![vec![0.0f64; down + 1]; across + 1];

            for x in 1..=across {
                forest[x][0] = forest[x - 1][0] + 1.0;
            }
            for y in 1..=down {
                forest[0][y] = forest[0][y - 1] + 1.0;
            }

            for x in 1..=across {
                let left_at = lleaf[i] + x - 1;

                for y in 1..=down {
                    let right_at = rleaf[j] + y - 1;
                    let same = left.labels[lorder[left_at - 1]]
                        == right.labels[rorder[right_at - 1]];

                    forest[x][y] = (forest[x - 1][y] + 1.0)
                        .min(forest[x][y - 1] + 1.0)
                        .min(forest[x - 1][y - 1] + if same { 0.0 } else { 1.0 });

                    if lleaf[left_at] == lleaf[i] && rleaf[right_at] == rleaf[j] {
                        distance[left_at][right_at] = forest[x][y];
                    } else {
                        forest[x][y] = forest[x][y].min(
                            forest[lleaf[left_at] - lleaf[i]][rleaf[right_at] - rleaf[j]]
                                + distance[left_at][right_at],
                        );
                    }
                }
            }
        }
    }

    distance[n][m]
}

/// How alike two documents' tables are, from nothing in common to identical.
///
/// One minus the tree edit distance over the larger of the two, which is what
/// TEDS is. Each reference table is matched to the table it is most alike, so
/// that two tables returned in the wrong order are not both scored as nothing.
///
/// **This exists because a shape is not a table.** The count above says whether
/// a table of the right size came back; this says whether what is in it is
/// right, and a table with every cell correct and its rows split differently
/// no longer scores the same as one with the words shuffled.
pub fn table_similarity(expected: &str, actual: &str) -> f64 {
    let wanted = outline(expected).tables;
    if wanted.is_empty() {
        return 1.0;
    }

    let produced = outline(actual).tables;
    let mut taken = vec![false; produced.len()];
    let mut total = 0.0;

    for want in &wanted {
        let mut best = 0.0f64;
        let mut pick = None;

        for (index, got) in produced.iter().enumerate() {
            if taken[index] {
                continue;
            }

            let (a, b) = (want.tree(), got.tree());
            let nodes = a.labels.len().max(b.labels.len());
            if nodes == 0 {
                continue;
            }

            let score = (1.0 - tree_distance(&a, &b) / nodes as f64).max(0.0);
            if score > best {
                best = score;
                pick = Some(index);
            }
        }

        if let Some(index) = pick {
            taken[index] = true;
        }

        total += best;
    }

    total / wanted.len() as f64
}

/// Puts a table row that was written across several lines back into one.
///
/// Markdown has no way to put a line break inside a cell, so a table whose cells
/// held one is written out with the break left in — and a row, and the table
/// with it, then end at the first of those lines. The corpus's references come
/// from OmniDocBench, which keeps the `<br>` the original table had: two of the
/// twenty-four arrive that way, one of them an eleven-column table whose header
/// is spread over eleven lines.
///
/// Read strictly, such a reference says its table has one row and is followed by
/// a paragraph. An engine that returns the six rows that are actually on the
/// page is then scored against a table that never existed, which is how a
/// correct reading of a five-column table came out at 0.39 and was written down
/// as the worst table in the set.
///
/// **Only a row that is not closed is joined.** A row that ends in `|` is
/// complete and has nothing left to continue it, so a rule that joined anyway
/// would swallow the horizontal rule or the paragraph below a table. The
/// unclosed row is the whole of the signal, and it is a reliable one: it is what
/// a break inside a cell produces and nothing else does.
///
/// Applied to both sides. The renderer writes one line per row — a newline in a
/// cell becomes a space before it is written — so for the output this does
/// nothing, and applying it there anyway is what keeps it a rule about Markdown
/// rather than a thumb on the scale.
fn joined_rows(markdown: &str) -> String {
    let mut rows: Vec<String> = Vec::new();
    let mut inside = false;

    for line in markdown.lines() {
        let trimmed = line.trim();
        // Nothing above, or a row that is closed: either way there is nothing
        // for this line to be the rest of.
        let open = rows
            .last()
            .is_some_and(|last| !last.trim_end().ends_with('|'));

        if trimmed.starts_with('|') {
            inside = true;
            rows.push(line.to_owned());
        } else if inside && !trimmed.is_empty() && open {
            let last = rows.last_mut().expect("open means something is above");
            last.push(' ');
            last.push_str(trimmed);
        } else {
            inside = false;
            rows.push(line.to_owned());
        }
    }

    rows.join("\n")
}

fn outline(markdown: &str) -> Outline {
    let joined = joined_rows(markdown);
    let markdown = joined.as_str();

    let arena = Arena::new();
    let mut options = Options::default();
    options.extension.table = true;
    // The extracted output carries front matter. It is metadata rather than
    // content, so it must not count against a reference that has none.
    options.extension.front_matter_delimiter = Some("---".to_owned());

    let root = parse_document(&arena, markdown, &options);
    let mut outline = Outline::default();

    for node in root.descendants() {
        if let NodeValue::Heading(..) = node.data.borrow().value {
            outline.headings.push(normalize(&descendant_text(&node)));
            continue;
        }

        let is_table = matches!(node.data.borrow().value, NodeValue::Table(..));
        if is_table {
            let mut rows: Vec<Vec<String>> = Vec::new();

            for row in node
                .descendants()
                .filter(|child| matches!(child.data.borrow().value, NodeValue::TableRow(..)))
            {
                let mut cells: Vec<String> = Vec::new();

                for cell in row.children() {
                    if matches!(cell.data.borrow().value, NodeValue::TableCell) {
                        cells.push(descendant_text(&cell));
                    }
                }

                rows.push(cells);
            }

            outline.tables.push(Table { rows });
            continue;
        }

        match node.data.borrow().value {
            NodeValue::Text(ref text) => outline.text.push_str(text),
            // A fenced block's content is a literal rather than a text node, so
            // it has to be taken explicitly. Leaving it out scores an engine
            // that returned the whole page inside one fence as though it had
            // returned nothing at all — the characters are on the page and a
            // reader sees them, so they belong in the comparison.
            NodeValue::CodeBlock(ref code) => {
                outline.text.push(' ');
                outline.text.push_str(&code.literal);
                outline.text.push(' ');
            }

            // A space separates blocks, so the end of one does not fuse onto
            // the start of the next. Text nodes within a block are simply
            // concatenated, because a run boundary is a *formatting* boundary,
            // not a word boundary: `**年**度报告` is one word split across two
            // runs, and inserting a space there would invent a gap that the
            // document does not have.
            NodeValue::Paragraph | NodeValue::Item(..) => {
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
        assert_eq!(score.content_error_rate, 0.0);
        assert_eq!(score.heading_accuracy(), 1.0);
        assert_eq!(score.loss(), 0.0);
    }

    /// The failure this metric exists for. An engine that reads every character
    /// of a two-column page and lays it out across the columns rather than down
    /// them is not the same as one that read nothing, and the ordered rate
    /// cannot tell them apart.
    #[test]
    fn misread_and_misordered_are_different_findings() {
        let reference = "# Heading\n\nfirst column second column\n";
        // The same words, taken in the wrong order.
        let transposed = "# Heading\n\nsecond column first column\n";

        let score = score(reference, transposed);

        assert!(
            score.character_error_rate > 0.2,
            "out of order should cost the ordered rate something: {score:?}"
        );
        assert!(
            score.content_error_rate < 0.05,
            "but every character is present: {score:?}"
        );
        assert!(
            score.character_error_rate > score.content_error_rate,
            "the gap is the damage ordering did"
        );
    }

    #[test]
    fn a_missing_character_costs_both_rates() {
        let score = score("abcdef", "abcdf");

        assert!(score.character_error_rate > 0.0);
        assert!(score.content_error_rate > 0.0);
    }

    #[test]
    fn extra_output_is_not_free() {
        let score = score("abcdef", "abcdefghij");

        assert!(score.character_error_rate > 0.0, "{score:?}");
        assert!(score.content_error_rate > 0.0, "{score:?}");
    }

    /// A fenced block's content is a literal rather than a text node. Taken on
    /// the parser's terms, a whole page returned inside one fence scored as
    /// though nothing had been returned — a correctly read page reported as a
    /// total failure, which is the most misleading answer a metric can give.
    ///
    /// The rate is not zero even now, and should not be: inside a fence the
    /// `#` is literal content rather than a heading, so a wrapped page really
    /// does differ from the unwrapped one by its syntax characters. What
    /// changed is the size of the finding, from everything to almost nothing.
    #[test]
    fn text_inside_a_code_block_is_still_text() {
        let reference = "# Title\n\nThe page's own words.\n";
        let wrapped = format!("```markdown\n{reference}```");

        let score = score(reference, &wrapped);

        assert!(
            score.content_error_rate < 0.15,
            "every word is present: {score:?}"
        );
        assert!(
            score.headings_matched < score.headings_total,
            "but a wrapped heading is a heading to nobody: {score:?}"
        );
    }

    /// A sentence introducing the answer is not the page, and this scorer
    /// counts it as output rather than excusing it — which is right, and is why
    /// stripping the envelope belongs to the endpoint that received it rather
    /// than to the measurement. See `endpoint::strip_envelope`.
    #[test]
    fn a_preamble_is_counted_against_the_engine() {
        let reference = "# Title\n\nBody.\n";
        let introduced = format!("Here is the transcription:\n\n```\n{reference}```");

        assert!(score(reference, &introduced).content_error_rate > 0.5);
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

    /// Depth is not compared, because the reference cannot express it. See
    /// `Outline::headings`.
    #[test]
    fn a_heading_is_compared_by_its_text_and_not_its_depth() {
        let score = score("# Title", "## Title");

        assert_eq!(score.headings_total, 1);
        assert_eq!(score.headings_matched, 1);
    }

    /// And the words still have to be the words.
    #[test]
    fn a_heading_with_the_wrong_text_is_still_a_miss() {
        let score = score("# Title", "## Something else");

        assert_eq!(score.headings_total, 1);
        assert_eq!(score.headings_matched, 0);
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

    /// The reference two of the corpus's cases are written in: a cell held a
    /// line break, Markdown has nowhere to put one, and the row is written
    /// across two lines with the first one left open.
    #[test]
    fn a_row_written_across_two_lines_is_one_row() {
        let expected = "| a | b |\n| --- | --- |\n| c | one\n two |\n";
        let actual = "| a | b |\n| --- | --- |\n| c | one two |\n";

        assert_eq!(score(expected, actual).tables_matched, 1);
        assert_eq!(score(expected, actual).table_similarity, 1.0);
    }

    /// The guard on the rule: only a row that is not closed can be continued,
    /// so what follows a finished table is left where it is. Lines rather than
    /// text, so the trailing newline of the input is not in the answer.
    #[test]
    fn what_follows_a_closed_row_is_not_swallowed() {
        assert_eq!(
            joined_rows("| a | b |\n| --- | --- |\n| c | d |\n---\nafter\n"),
            "| a | b |\n| --- | --- |\n| c | d |\n---\nafter"
        );
    }

    /// And a blank line still ends a table, because an open row is not a
    /// licence to read the rest of the document into it.
    #[test]
    fn a_blank_line_ends_a_table_that_is_mid_row() {
        assert_eq!(joined_rows("| a | b\n\nafter\n"), "| a | b\n\nafter");
    }

    /// The worst table in the corpus, reduced: an eleven-column header whose
    /// cells each carried a line or two, so the header arrives as eleven lines.
    #[test]
    fn a_header_written_over_eleven_lines_is_one_header() {
        let mut expected = String::from("| 名称 | 代码 |\n");
        for i in 0..9 {
            expected.push_str(&format!("line {i}\n"));
        }
        expected.push_str("| --- | --- |\n| x | 1 |\n");
        let actual = "| 名称 | 代码 |\n| --- | --- |\n| x | 1 |\n";

        assert_eq!(score(&expected, actual).table_similarity, 1.0);
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

    // ── Table similarity ─────────────────────────────────────────────────────

    const WANTED: &str = "| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |\n";

    #[test]
    fn a_tree_is_no_distance_from_itself() {
        let tree = Table {
            rows: vec![vec!["a".into(), "b".into()], vec!["1".into(), "2".into()]],
        };

        assert_eq!(tree_distance(&tree.tree(), &tree.tree()), 0.0);
    }

    /// One node against another is one edit, whichever way round it is read.
    #[test]
    fn distance_is_symmetric() {
        let left = Table {
            rows: vec![vec!["a".into()], vec!["b".into()], vec!["c".into()]],
        };
        let right = Table {
            rows: vec![vec!["a".into()], vec!["z".into()]],
        };

        let there = tree_distance(&left.tree(), &right.tree());
        let back = tree_distance(&right.tree(), &left.tree());

        assert_eq!(there, back);
        // Two edits: a row renamed to the other's, and one row deleted — and a
        // deleted row takes its cell with it, which is the third.
        assert_eq!(there, 3.0);
    }

    #[test]
    fn a_table_is_identical_to_itself() {
        assert_eq!(table_similarity(WANTED, WANTED), 1.0);
    }

    #[test]
    fn one_wrong_cell_is_not_the_same_table() {
        let actual = "| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 9 |\n";

        let score = table_similarity(WANTED, actual);

        assert!(score > 0.5 && score < 1.0, "{score}");
    }

    /// The reason this exists, and the reason the count beside it stays.
    ///
    /// Fifteen rows by three columns either way, and not one word in common.
    /// **The count calls it a match, and it is not one** — the structure is
    /// there and the table is not. Two numbers because they answer two
    /// questions: was a table of the right shape returned, and is what is in it
    /// right.
    #[test]
    fn the_same_shape_with_different_words_is_not_the_same_table() {
        let mut wanted = String::from("| a | b | c |\n| --- | --- | --- |\n");
        let mut actual = String::from("| x | y | z |\n| --- | --- | --- |\n");
        for row in 0..14 {
            wanted.push_str(&format!("| left{row} | mid{row} | right{row} |\n"));
            actual.push_str(&format!("| one{row} | two{row} | three{row} |\n"));
        }

        let found = score(&wanted, &actual);
        assert_eq!(found.tables_matched, 1, "the shape is the shape");

        // The table, the rows and the root are all the same node; the
        // forty-five cells are all renames.
        let alike = table_similarity(&wanted, &actual);
        assert!((alike - (1.0 - 45.0 / 61.0)).abs() < 1e-9, "{alike}");
    }

    /// And the same table twice is worth one, which is what makes the number
    /// above mean anything.
    #[test]
    fn the_same_words_in_the_same_places_is_the_same_table() {
        let mut wanted = String::from("| a | b | c |\n| --- | --- | --- |\n");
        for row in 0..14 {
            wanted.push_str(&format!("| left{row} | mid{row} | right{row} |\n"));
        }

        assert_eq!(table_similarity(&wanted, &wanted), 1.0);
    }

    /// Paired by likeness and not by position: two tables returned the other
    /// way round are two tables found, which pairing by index got wrong.
    #[test]
    fn two_tables_in_the_wrong_order_are_both_found() {
        let first = "| a |\n| --- |\n| 1 |\n";
        let second = "| z |\n| --- |\n| 9 |\n";

        let wanted = format!("{first}\n\n{second}\n");
        let actual = format!("{second}\n\n{first}\n");

        assert_eq!(table_similarity(&wanted, &actual), 1.0);
    }

    /// A table that is missing is worth nothing, and one that was not asked for
    /// does not make up for it.
    #[test]
    fn a_missing_table_scores_nothing() {
        assert_eq!(table_similarity(WANTED, "no tables here"), 0.0);
    }

    #[test]
    fn a_document_with_no_tables_is_vacuously_perfect() {
        assert_eq!(table_similarity("just prose", "just prose"), 1.0);
    }
}
