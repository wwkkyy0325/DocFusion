//! Checking one engine against another.
//!
//! The verifier is whichever engine did *not* produce the primary result, and
//! it has to be a different lineage: a model asked to check itself has
//! correlated errors and cannot find its own mistakes. Two engines reading the
//! same page independently is what turns a silent failure — the failure mode of
//! every general vision model — into a recorded one.
//!
//! What a disagreement *means* is not decided here. Comparing two readings is a
//! pure function of two block lists, so it is testable without a model, a
//! network, or a set of weights on disk.

use regex::RegexSet;

use crate::ir::{Block, Conflict, ConflictKind, Rect, Variant};

/// Below this, a block is one the engine was unsure of itself.
const DEFAULT_CONFIDENCE: f32 = 0.85;

/// How many pages a second engine may be asked to read.
///
/// Verification is the expensive half of the work — another recognition pass,
/// or another billed request — so a thousand-page scan must not be able to run
/// up an unbounded bill.
pub const DEFAULT_MAX_VERIFY_PAGES: u32 = 50;

/// How much two rectangles must overlap before they are called one region.
///
/// Loose enough to survive the two engines drawing their boxes differently,
/// tight enough that two stacked paragraphs are not both matched to one.
const MIN_IOU: f32 = 0.5;

/// The text patterns that make a page worth a second opinion.
///
/// A recognition error in prose costs a reader a moment; the same error in an
/// amount, a date or an identifier costs money. These are shapes that carry a
/// *value* rather than a meaning, and a value has to be exactly right.
///
/// Deliberately broad, because the two mistakes are not symmetric. A pattern
/// that fires when it need not costs one more pass of the local recognition
/// engine, which is free and takes seconds; a pattern that stays quiet when it
/// should have fired ships a wrong number in a document somebody will trust.
/// Only bare integers are left out, since almost every document contains one
/// and a trigger that always fires is not a decision.
///
/// Changing these changes what the tool verifies, so it is a change to the
/// engine's output — the version in [`crate::extract::scan`] has to move with
/// it, or a cached result will be served for a policy that never produced it.
const CRITICAL_FIELDS: &[&str] = &[
    // Amounts, in the currencies this is likely to meet.
    r"[¥$€£]\s?\d[\d,]*(?:\.\d+)?",
    // Dates, in the orders and separators that appear in practice.
    r"\d{4}[-/年]\d{1,2}(?:[-/月]\d{1,2}日?)?",
    r"\d{1,2}/\d{1,2}/\d{2,4}",
    // Identifiers: a prefix and a long run of digits, as in INV-2024-0001.
    r"[A-Z]{2,}-\d{3,}",
    // Clause and article numbers.
    r"(?:§|第)\s?\d+(?:\.\d+)*(?:条|款|项|章)?",
    // Dotted numbers: version and section numbers.
    r"\b\d+\.\d+(?:\.\d+)*\b",
    // Percentages, and numbers written with a thousands separator. Both are
    // values a reader would act on, which is the test for this list.
    r"\d+(?:\.\d+)?%",
    r"\d{1,3}(?:,\d{3})+(?:\.\d+)?",
];

/// When the second engine is asked to read a page.
///
/// Dual-running every page doubles both cost and latency, and buys nothing on a
/// clean document. The decision is what keeps the check affordable enough to
/// leave on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMode {
    /// Never. The second engine is not run at all.
    Off,

    /// Where the primary was unsure of itself, or read a field whose exact
    /// value matters. The default: the second engine is engaged by the text,
    /// not by the page count.
    OnDoubt,

    /// Only where the primary read a field whose exact value matters.
    OnCriticalFields,

    /// Every page, whatever it says.
    Always,
}

/// What the second engine is asked to check, and how much it may spend.
#[derive(Debug, Clone)]
pub struct VerifyPolicy {
    pub mode: VerifyMode,
    /// Confidence below which a block counts as one the engine was unsure of.
    /// Consulted by [`VerifyMode::OnDoubt`].
    pub below: f32,
    /// Consulted by [`VerifyMode::OnDoubt`] and
    /// [`VerifyMode::OnCriticalFields`].
    pub trigger: FieldTrigger,
    /// Hard ceiling on how many pages may be sent to the second engine.
    ///
    /// Counted per document rather than per run, so one enormous scan cannot
    /// exhaust a budget the rest of the batch needed.
    pub max_verify_pages: u32,
}

impl Default for VerifyPolicy {
    /// Verification on demand, which is the design's rule 1: a second opinion
    /// where it is likely to be worth one, and not otherwise.
    fn default() -> Self {
        Self {
            mode: VerifyMode::OnDoubt,
            below: DEFAULT_CONFIDENCE,
            trigger: FieldTrigger::critical_fields(),
            max_verify_pages: DEFAULT_MAX_VERIFY_PAGES,
        }
    }
}

impl VerifyPolicy {
    /// Never run a second engine.
    pub fn off() -> Self {
        Self {
            mode: VerifyMode::Off,
            ..Self::default()
        }
    }

    /// Run a second engine on every page.
    pub fn always() -> Self {
        Self {
            mode: VerifyMode::Always,
            ..Self::default()
        }
    }

    /// Whether this page's reading is worth checking.
    pub fn should_verify(&self, blocks: &[Block]) -> bool {
        match self.mode {
            VerifyMode::Off => false,
            VerifyMode::Always => true,
            VerifyMode::OnDoubt => {
                lowest_confidence(blocks).is_some_and(|confidence| confidence < self.below)
                    || self.trigger.hits(&page_text(blocks))
            }
            VerifyMode::OnCriticalFields => self.trigger.hits(&page_text(blocks)),
        }
    }

    /// The policy as it reaches the cache key.
    ///
    /// The policy decides what the output *says* — which pages carry conflicts
    /// and which are marked verified — so leaving it out would serve a cached
    /// single-engine result to a run that asked for dual verification.
    pub fn describe(&self) -> String {
        let mode = match self.mode {
            VerifyMode::Off => "off".to_owned(),
            VerifyMode::Always => "always".to_owned(),
            VerifyMode::OnDoubt => format!("doubt@{}", self.below),
            VerifyMode::OnCriticalFields => "fields".to_owned(),
        };

        format!("verify:{mode};max:{}", self.max_verify_pages)
    }
}

/// Text worth a second opinion because its exact value matters.
#[derive(Debug, Clone)]
pub struct FieldTrigger {
    set: RegexSet,
}

impl FieldTrigger {
    pub fn critical_fields() -> Self {
        Self {
            // The patterns are a constant, so a failure here is a mistake in
            // the source rather than anything the caller can cause.
            set: RegexSet::new(CRITICAL_FIELDS).expect("the built-in patterns compile"),
        }
    }

    pub fn hits(&self, text: &str) -> bool {
        self.set.is_match(text)
    }
}

/// Compares what two engines said about one page.
///
/// Alignment is by position when both engines reported geometry, and by whole
/// page text when either did not. The second case is today's common one, since
/// a vision model answers with Markdown and no coordinates.
pub fn compare(page: u32, primary: &[Block], secondary: &[Block]) -> Vec<Conflict> {
    let mut conflicts = match aligned_by_position(page, primary, secondary) {
        Some(conflicts) => conflicts,
        None => aligned_by_text(page, primary, secondary),
    };

    // A dropped region is the dangerous failure: the text that *is* present
    // looks right, so nothing draws attention to what is missing. Reported
    // first, for the same reason.
    conflicts.sort_by_key(|conflict| match conflict.kind {
        ConflictKind::StructuralMismatch => 0,
        ConflictKind::TextMismatch => 1,
    });

    conflicts
}

/// Compares region by region, matching on bounding-box overlap.
///
/// Returns `None` when either side has a block with no bounding box. There is
/// then nothing to align on, and guessing an alignment would attribute a
/// disagreement to the wrong region — worse than admitting the report is
/// coarser.
fn aligned_by_position(page: u32, primary: &[Block], secondary: &[Block]) -> Option<Vec<Conflict>> {
    if primary
        .iter()
        .chain(secondary)
        .any(|block| block.bbox.is_none())
    {
        return None;
    }

    let mut conflicts = Vec::new();
    let mut matched = vec![false; secondary.len()];

    for left in primary {
        let Some(index) = best_match(left, secondary, &matched) else {
            conflicts.push(conflict(page, ConflictKind::StructuralMismatch, &[left]));
            continue;
        };

        matched[index] = true;
        let right = &secondary[index];

        if normalize(&block_text(left)) != normalize(&block_text(right)) {
            conflicts.push(conflict(page, ConflictKind::TextMismatch, &[left, right]));
        }
    }

    // Whatever the second engine saw and the first did not. A block is only
    // unmatched if nothing overlapped it, so this is a region one engine
    // dropped, not a reordering.
    for (index, right) in secondary.iter().enumerate() {
        if !matched[index] {
            conflicts.push(conflict(page, ConflictKind::StructuralMismatch, &[right]));
        }
    }

    Some(conflicts)
}

/// The unmatched block that overlaps `block` most, if any overlaps enough.
///
/// Greedy and one-to-one: each block on the second side can be claimed once, so
/// two paragraphs cannot both be matched to the same one and reported as two
/// agreements that never happened.
fn best_match(block: &Block, candidates: &[Block], matched: &[bool]) -> Option<usize> {
    let bbox = block.bbox?;

    candidates
        .iter()
        .enumerate()
        .filter(|(index, _)| !matched[*index])
        .filter_map(|(index, candidate)| {
            candidate
                .bbox
                .map(|other| (index, bbox.iou(&other)))
                .filter(|(_, overlap)| *overlap >= MIN_IOU)
        })
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(index, _)| index)
}

/// Compares the two readings as whole pages, without geometry.
///
/// Coarser than region-by-region matching but not weaker: two engines that
/// produce the same characters for a page have agreed about it, whatever they
/// called the blocks. What it cannot do is say *where* they disagree, and the
/// conflict is recorded without a region rather than with a guessed one.
fn aligned_by_text(page: u32, primary: &[Block], secondary: &[Block]) -> Vec<Conflict> {
    let left = page_text(primary);
    let right = page_text(secondary);

    if normalize(&left) == normalize(&right) {
        return Vec::new();
    }

    let variants: Vec<Variant> = [(&left, primary), (&right, secondary)]
        .into_iter()
        .filter_map(|(text, blocks)| {
            blocks.first().map(|block| Variant {
                engine: block.engine.clone(),
                text: text.clone(),
            })
        })
        .collect();

    // One side saw text and the other saw none: that is a region to one engine
    // and not to the other, which is a different finding from two readings of
    // the same region that differ.
    let kind = if variants.len() < 2 {
        ConflictKind::StructuralMismatch
    } else {
        ConflictKind::TextMismatch
    };

    vec![Conflict {
        page,
        bbox: None,
        kind,
        variants,
        resolved: None,
    }]
}

/// Builds a conflict from the blocks that disagree.
///
/// The region is the union of theirs, which is where the disagreement is
/// whatever side is right about the edges.
fn conflict(page: u32, kind: ConflictKind, blocks: &[&Block]) -> Conflict {
    let mut bbox: Option<Rect> = None;
    for block in blocks {
        if let Some(rect) = block.bbox {
            match &mut bbox {
                Some(covered) => covered.union(&rect),
                empty => *empty = Some(rect),
            }
        }
    }

    Conflict {
        page,
        bbox,
        kind,
        variants: blocks
            .iter()
            .map(|block| Variant {
                engine: block.engine.clone(),
                text: block_text(block),
            })
            .collect(),
        resolved: None,
    }
}

/// A block's own text and everything nested inside it.
///
/// A table's `text` is empty because its content lives in the cells, so
/// comparing the field alone would report a table against the same table read
/// as prose — a real difference of opinion about structure, but not about what
/// the page says.
fn block_text(block: &Block) -> String {
    let mut text = block.text.clone();
    for child in &block.children {
        text.push_str(&block_text(child));
    }
    text
}

fn page_text(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(block_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The least confident block on the page, when the engine reports confidence.
///
/// The minimum rather than the mean: one line the engine was unsure of is
/// reason enough for a second opinion, and averaging lets a page of confident
/// text bury it.
fn lowest_confidence(blocks: &[Block]) -> Option<f32> {
    blocks
        .iter()
        .flat_map(descendants)
        .filter_map(|block| block.confidence)
        .min_by(f32::total_cmp)
}

fn descendants(block: &Block) -> Box<dyn Iterator<Item = &Block> + '_> {
    Box::new(std::iter::once(block).chain(block.children.iter().flat_map(descendants)))
}

/// Folds away the differences two engines are entitled to have.
///
/// Whitespace goes entirely. An engine that reads `the cat` and one that reads
/// `thecat` have not disagreed about the document, and spacing is much the
/// largest source of noise between a recognition model and a language model —
/// particularly for scripts that do not separate words. Full-width forms fold
/// to half-width for the same reason: the two engines disagree about which to
/// emit far more often than about the character itself.
///
/// Nothing else is folded. Case, punctuation and character identity are the
/// things the comparison is *for*, and a tolerance wide enough to hide a wrong
/// digit is the failure this whole module exists to prevent.
fn normalize(text: &str) -> String {
    text.chars()
        .map(fold_width)
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn fold_width(character: char) -> char {
    match character {
        '\u{3000}' => ' ',
        '\u{FF01}'..='\u{FF5E}' => {
            char::from_u32(character as u32 - 0xFEE0).unwrap_or(character)
        }
        _ => character,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{BlockKind, EngineId};

    fn at(engine: &str, text: &str, bbox: Option<Rect>) -> Block {
        Block {
            kind: BlockKind::Paragraph,
            text: text.to_owned(),
            bbox,
            confidence: None,
            engine: EngineId::from(engine),
            children: Vec::new(),
        }
    }

    /// Two engines looking at the same region and reading it the same way.
    fn overlapping() -> (Block, Block) {
        (
            at("a", "the same words", Some(Rect::new(0.0, 0.0, 0.5, 0.1))),
            at("b", "the same words", Some(Rect::new(0.0, 0.0, 0.5, 0.1))),
        )
    }

    // ── Normalization ────────────────────────────────────────────────────────

    /// Spacing is the largest source of noise between a recognition model and
    /// a language model, and it is not a disagreement about the document.
    #[test]
    fn whitespace_does_not_count_as_a_difference() {
        assert_eq!(normalize("the cat"), normalize("thecat"));
        assert_eq!(normalize("  a \n b  "), normalize("ab"));
        assert_eq!(normalize("Revenue 12%"), normalize("Revenue12%"));
    }

    #[test]
    fn full_width_forms_fold_to_half_width() {
        assert_eq!(normalize("ＡＢＣ１２３"), "abc123");
        assert_eq!(normalize("全角　空格"), normalize("全角空格"));
    }

    /// The whole point is to catch a wrong digit, so the tolerance must not
    /// reach that far.
    #[test]
    fn a_single_wrong_character_is_still_a_difference() {
        assert_ne!(normalize("12%"), normalize("13%"));
        assert_ne!(normalize("Acme Ltd"), normalize("Acme Lld"));
        assert_ne!(normalize("1.0"), normalize("1.O"));
    }

    // ── Policy ───────────────────────────────────────────────────────────────

    fn unsure(text: &str, confidence: f32) -> Block {
        let mut block = at("a", text, None);
        block.confidence = Some(confidence);
        block
    }

    #[test]
    fn a_policy_of_off_never_verifies() {
        let policy = VerifyPolicy::off();

        assert!(!policy.should_verify(&[unsure("$1,234.00", 0.10)]));
        assert!(!policy.should_verify(&[unsure("plain prose", 0.99)]));
    }

    #[test]
    fn always_verifies_whatever_the_page_says() {
        assert!(VerifyPolicy::always().should_verify(&[unsure("plain prose", 0.99)]));
    }

    /// Rule 1: the second opinion is bought where it is worth buying.
    #[test]
    fn doubt_verifies_an_unsure_page_and_leaves_a_confident_one_alone() {
        let policy = VerifyPolicy::default();

        assert!(policy.should_verify(&[unsure("plain prose", 0.60)]));
        assert!(!policy.should_verify(&[unsure("plain prose", 0.99)]));
    }

    #[test]
    fn doubt_verifies_a_page_holding_a_value_that_matters() {
        let policy = VerifyPolicy::default();

        assert!(policy.should_verify(&[unsure("Total: $1,234.00", 0.99)]));
        assert!(policy.should_verify(&[unsure("Signed 2024-01-05", 0.99)]));
        assert!(!policy.should_verify(&[unsure("Revenue roughly doubled", 0.99)]));
    }

    /// A page of confident text must not bury the one line the engine doubted.
    #[test]
    fn doubt_uses_the_least_confident_block_not_the_average() {
        let policy = VerifyPolicy::default();
        let page = vec![
            unsure("confident line", 0.99),
            unsure("confident line", 0.99),
            unsure("doubtful line", 0.40),
        ];

        assert!(policy.should_verify(&page));
    }

    #[test]
    fn critical_fields_ignores_confidence() {
        let policy = VerifyPolicy {
            mode: VerifyMode::OnCriticalFields,
            ..VerifyPolicy::default()
        };

        assert!(policy.should_verify(&[unsure("Invoice INV-2024-0001", 0.99)]));
        assert!(!policy.should_verify(&[unsure("plain prose", 0.10)]));
    }

    /// The policy decides what the output says, so it has to reach the cache
    /// key or a cached single-engine result answers a dual-verification run.
    #[test]
    fn the_policy_reaches_the_cache_key() {
        let off = VerifyPolicy::off().describe();
        let always = VerifyPolicy::always().describe();
        let capped = VerifyPolicy {
            max_verify_pages: 3,
            ..VerifyPolicy::always()
        }
        .describe();

        assert_ne!(off, always);
        assert_ne!(always, capped, "the ceiling changes the output too");
    }

    #[test]
    fn the_trigger_recognises_the_shapes_that_carry_a_value() {
        let trigger = FieldTrigger::critical_fields();

        for text in [
            "Total: $1,234.00",
            "金额 ¥1200",
            "Due 2024-01-05",
            "Due 05/01/2024",
            "Order INV-2024-0001",
            "see §3.2",
            "第12条",
            "version 2.10.1",
            // A value a reader would act on, even without a currency.
            "Revenue increased by 13%",
            "1,234 units shipped",
        ] {
            assert!(trigger.hits(text), "should have fired on {text:?}");
        }

        // A bare integer is left out on purpose: almost every document holds
        // one, and a trigger that always fires is not a decision.
        for text in ["plain prose", "a paragraph of ordinary words", "N/A", "in 2024"] {
            assert!(!trigger.hits(text), "should not have fired on {text:?}");
        }
    }

    // ── Alignment ────────────────────────────────────────────────────────────

    #[test]
    fn agreeing_engines_produce_nothing_to_report() {
        let (left, right) = overlapping();

        assert!(compare(0, &[left], &[right]).is_empty());
    }

    #[test]
    fn the_same_region_read_differently_is_a_text_mismatch() {
        let (left, mut right) = overlapping();
        right.text = "thirteen".into();

        let conflicts = compare(0, &[left], &[right]);

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, ConflictKind::TextMismatch);
        assert_eq!(conflicts[0].variants.len(), 2);
        assert_eq!(conflicts[0].variants[0].text, "the same words");
        assert_eq!(conflicts[0].variants[1].text, "thirteen");
        assert_eq!(conflicts[0].variants[0].engine.as_str(), "a");
        assert!(conflicts[0].resolved.is_none(), "nothing was preferred");
    }

    #[test]
    fn a_region_only_one_engine_saw_is_a_structural_mismatch() {
        let (left, _) = overlapping();
        let elsewhere = at("b", "a line of its own", Some(Rect::new(0.0, 0.8, 0.5, 0.1)));

        let conflicts = compare(0, &[left], &[elsewhere]);

        assert_eq!(conflicts.len(), 2, "unmatched on both sides: {conflicts:?}");
        assert!(
            conflicts
                .iter()
                .all(|conflict| conflict.kind == ConflictKind::StructuralMismatch)
        );
        // One variant means one engine saw it and the other did not.
        assert!(conflicts.iter().any(|conflict| conflict.variants.len() == 1));
    }

    /// The dangerous class is reported first: the text that *is* present looks
    /// right, so nothing else draws attention to what is missing.
    #[test]
    fn dropped_regions_are_reported_before_different_readings() {
        let (left, mut right) = overlapping();
        right.text = "different".into();
        let dropped = at("b", "a line of its own", Some(Rect::new(0.0, 0.8, 0.5, 0.1)));

        let conflicts = compare(0, &[left], &[right, dropped]);

        assert_eq!(conflicts[0].kind, ConflictKind::StructuralMismatch);
        assert_eq!(conflicts[1].kind, ConflictKind::TextMismatch);
    }

    /// Each block on the second side can be claimed once, or one paragraph
    /// becomes two agreements that never happened.
    #[test]
    fn a_block_is_matched_at_most_once() {
        let shared = Rect::new(0.0, 0.0, 0.5, 0.1);
        let primary = vec![at("a", "x", Some(shared)), at("a", "x", Some(shared))];
        let secondary = vec![at("b", "x", Some(shared))];

        let conflicts = compare(0, &primary, &secondary);

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, ConflictKind::StructuralMismatch);
    }

    #[test]
    fn regions_that_barely_overlap_are_not_the_same_region() {
        let left = at("a", "one", Some(Rect::new(0.0, 0.0, 0.5, 0.1)));
        // Shares a sliver of the first, far short of MIN_IOU.
        let right = at("b", "two", Some(Rect::new(0.45, 0.0, 0.5, 0.1)));

        let conflicts = compare(0, &[left], &[right]);

        assert_eq!(conflicts.len(), 2, "two regions, neither matched");
    }

    /// Without geometry there is nothing to align on, and a guessed region
    /// would attribute the disagreement to the wrong place.
    #[test]
    fn engines_without_geometry_are_compared_as_whole_pages() {
        let left = at("a", "Revenue increased", None);
        let right = at("b", "Revenue decreased", None);

        let conflicts = compare(4, &[left], &[right]);

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].page, 4);
        assert_eq!(conflicts[0].bbox, None, "a region would be a guess");
        assert_eq!(conflicts[0].kind, ConflictKind::TextMismatch);
        assert_eq!(conflicts[0].variants.len(), 2);
    }

    #[test]
    fn whole_page_comparison_reports_nothing_when_the_engines_agree() {
        let left = at("a", "Revenu e increased", None);
        let right = at("b", "Revenue increased", None);

        assert!(compare(0, &[left], &[right]).is_empty());
    }

    /// A page of text against a blank one is a dropped region, not a different
    /// reading of the same region.
    #[test]
    fn a_page_one_engine_saw_and_the_other_did_not_is_structural() {
        let left = at("a", "Signature", None);

        let conflicts = compare(0, &[left], &[]);

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].kind, ConflictKind::StructuralMismatch);
        assert_eq!(conflicts[0].variants.len(), 1);
    }

    /// A table's text lives in its cells, so comparing the field alone would
    /// report the same table as a disagreement.
    #[test]
    fn a_table_is_compared_by_its_contents_not_its_empty_text() {
        let mut table = at("a", "", None);
        table.kind = BlockKind::Table;
        let mut row = at("a", "", None);
        row.kind = BlockKind::TableRow;
        row.children.push(at("a", "Q1", None));
        row.children.push(at("a", "Q2", None));
        table.children.push(row);

        assert_eq!(block_text(&table), "Q1Q2");
    }

    #[test]
    fn a_page_mixing_geometry_and_blank_boxes_falls_back_to_text() {
        let left = vec![
            at("a", "one", Some(Rect::new(0.0, 0.0, 0.5, 0.1))),
            at("a", "two", None),
        ];
        let right = vec![at("b", "one", Some(Rect::new(0.0, 0.0, 0.5, 0.1)))];

        let conflicts = compare(0, &left, &right);

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].bbox, None);
    }
}
