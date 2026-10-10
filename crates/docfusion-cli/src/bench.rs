//! Scoring engines against a corpus of reference documents.
//!
//! Selecting a model by reading a leaderboard would mean trusting numbers
//! produced on somebody else's documents under a benchmark version that may not
//! even be the one being compared against. This runs candidates over a corpus of
//! the user's own documents instead, so the choice is a measurement.
//!
//! Nothing here writes: the corpus is read, engines are run in memory, and the
//! knowledge base is not involved.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use docfusion_core::bench::{self, Score};
use docfusion_core::extract::{InputProbe, Router};
use docfusion_core::ir::SourceRef;
use docfusion_core::mime::SNIFF_LENGTH;
use docfusion_core::render;
use serde_json::json;

use crate::CliError;
use crate::output::Reporter;

/// A reference document and the source it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub name: String,
    pub source: PathBuf,
    pub reference: PathBuf,
}

/// The rendered output is compared against the reference, and the front matter
/// carries a timestamp. Fixing it keeps two runs of the same engine byte-identical,
/// so a difference in the report always means a difference in the output.
const BENCH_TIMESTAMP: &str = "1970-01-01T00:00:00Z";

/// The reference file every case directory must hold.
const REFERENCE: &str = "expected.md";

/// A corpus is one directory per case, each holding `expected.md` beside the
/// document it should come from.
///
/// A directory per case rather than a flat pile of same-stemmed files, because
/// the pairing has to survive a source that is itself Markdown: `notes.md`
/// beside `notes.md` is not a pairing, it is one file. Keeping a case
/// self-contained also leaves room for assets next to it later.
pub fn load(directory: &Path) -> Result<Vec<Case>, CliError> {
    let entries = fs::read_dir(directory).map_err(|error| {
        CliError::NotFound(format!(
            "could not read the corpus at {}: {error}",
            directory.display()
        ))
    })?;

    let mut cases = Vec::new();
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            cases.push(load_case(&entry.path())?);
        }
    }

    cases.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(cases)
}

fn load_case(directory: &Path) -> Result<Case, CliError> {
    let reference = directory.join(REFERENCE);
    if !reference.is_file() {
        return Err(CliError::NotFound(format!(
            "{} holds no {REFERENCE}",
            directory.display()
        )));
    }

    let sources = fs::read_dir(directory)
        .map_err(|error| CliError::NotFound(format!("{}: {error}", directory.display())))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .is_none_or(|name| name.to_string_lossy() != REFERENCE)
        });

    let mut sources = sources;
    let source = sources.next().ok_or_else(|| {
        CliError::NotFound(format!(
            "{} holds only {REFERENCE}, so there is nothing to read",
            directory.display()
        ))
    })?;

    if let Some(extra) = sources.next() {
        return Err(CliError::NotFound(format!(
            "{} holds more than one source, so which to read is ambiguous: {} and {}",
            directory.display(),
            source.display(),
            extra.display()
        )));
    }

    Ok(Case {
        name: directory
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        source,
        reference,
    })
}

#[derive(Debug, Clone)]
pub struct CaseResult {
    pub name: String,
    pub score: Option<Score>,
    pub duration: Duration,
    pub engine: Option<String>,
    pub error: Option<String>,
}

impl CaseResult {
    fn loss(&self) -> Option<f64> {
        self.score.as_ref().map(Score::loss)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub engine: String,
    pub cases: Vec<CaseResult>,
}

impl Report {
    /// Mean loss over the cases that ran.
    pub fn mean_loss(&self) -> Option<f64> {
        mean(self.cases.iter().filter_map(CaseResult::loss))
    }

    pub fn mean_character_error_rate(&self) -> Option<f64> {
        mean(
            self.cases
                .iter()
                .filter_map(|case| case.score.as_ref().map(|score| score.character_error_rate)),
        )
    }

    /// The same, with reading order taken out. The gap is what ordering cost.
    pub fn mean_content_error_rate(&self) -> Option<f64> {
        mean(
            self.cases
                .iter()
                .filter_map(|case| case.score.as_ref().map(|score| score.content_error_rate)),
        )
    }

    pub fn headings_matched(&self) -> usize {
        self.cases
            .iter()
            .filter_map(|case| case.score.as_ref())
            .map(|score| score.headings_matched)
            .sum()
    }

    pub fn headings_total(&self) -> usize {
        self.cases
            .iter()
            .filter_map(|case| case.score.as_ref())
            .map(|score| score.headings_total)
            .sum()
    }

    pub fn tables_matched(&self) -> usize {
        self.cases
            .iter()
            .filter_map(|case| case.score.as_ref())
            .map(|score| score.tables_matched)
            .sum()
    }

    /// The mean of how alike the tables are, over the cases that had any.
    ///
    /// **Over the cases that had a table in the reference, and not over all of
    /// them.** A document with no tables scores one — there is nothing to be
    /// wrong about — and seventeen of the corpus's twenty-four are documents
    /// with no tables. Averaging those in reported 0.953 for a set whose seven
    /// table cases average 0.84, which is the kind of number that is true of
    /// nothing.
    pub fn mean_table_similarity(&self) -> Option<f64> {
        let scores: Vec<f64> = self
            .cases
            .iter()
            .filter_map(|case| case.score.as_ref())
            .filter(|score| score.tables_total > 0)
            .map(|score| score.table_similarity)
            .collect();

        if scores.is_empty() {
            None
        } else {
            Some(scores.iter().sum::<f64>() / scores.len() as f64)
        }
    }

    pub fn tables_total(&self) -> usize {
        self.cases
            .iter()
            .filter_map(|case| case.score.as_ref())
            .map(|score| score.tables_total)
            .sum()
    }

    pub fn total_duration(&self) -> Duration {
        self.cases.iter().map(|case| case.duration).sum()
    }

    pub fn failures(&self) -> usize {
        self.cases.iter().filter(|case| case.error.is_some()).count()
    }
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut total = 0.0;
    let mut count = 0usize;
    for value in values {
        total += value;
        count += 1;
    }
    (count > 0).then(|| total / count as f64)
}

pub fn run(corpus: &[Case], router: &Router, engine: &str) -> Report {
    let mut report = Report {
        engine: engine.to_owned(),
        cases: Vec::with_capacity(corpus.len()),
    };

    for case in corpus {
        report.cases.push(run_case(case, router));
    }

    report
}

fn run_case(case: &Case, router: &Router) -> CaseResult {
    let started = Instant::now();

    let mut result = CaseResult {
        name: case.name.clone(),
        score: None,
        duration: Duration::ZERO,
        engine: None,
        error: None,
    };

    match evaluate(case, router) {
        Ok((score, engine)) => {
            result.score = Some(score);
            result.engine = Some(engine);
        }
        Err(message) => result.error = Some(message),
    }

    result.duration = started.elapsed();
    result
}

fn evaluate(case: &Case, router: &Router) -> Result<(Score, String), String> {
    let bytes = fs::read(&case.source)
        .map_err(|error| format!("{}: {error}", case.source.display()))?;

    let reference = fs::read_to_string(&case.reference)
        .map_err(|error| format!("{}: {error}", case.reference.display()))?;

    let probe = InputProbe::new(
        &bytes[..bytes.len().min(SNIFF_LENGTH)],
        case.source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
        bytes.len() as u64,
    );

    let source = SourceRef {
        path: case.source.clone(),
        content_hash: String::new(),
        mime: probe.container.mime_type().to_owned(),
        size: bytes.len() as u64,
        mtime: None,
    };

    let document = router
        .extract(&probe, &source, &bytes)
        .map_err(|error| error.to_string())?;

    let engine = document
        .pages
        .first()
        .map(|page| page.provenance.primary.to_string())
        .unwrap_or_else(|| "none".to_owned());

    let rendered = render::markdown(&document, BENCH_TIMESTAMP);
    Ok((bench::score(&reference, &rendered), engine))
}

pub fn report(reporter: &Reporter, report: &Report) {
    for case in &report.cases {
        let value = json!({
            "command": "bench",
            "case": case.name,
            "engine": case.engine,
            "loss": case.loss(),
            "character_error_rate": case.score.as_ref().map(|score| score.character_error_rate),
            "content_error_rate": case.score.as_ref().map(|score| score.content_error_rate),
            "headings": case.score.as_ref().map(|score| (score.headings_matched, score.headings_total)),
            "tables": case.score.as_ref().map(|score| (score.tables_matched, score.tables_total)),
            "table_similarity": case.score.as_ref().map(|score| score.table_similarity),
            "duration_ms": case.duration.as_secs_f64() * 1000.0,
            "error": case.error,
        });

        let human = match (&case.score, &case.error) {
            (Some(score), _) => format!(
                "{:<24} loss {:.3}  order {:.3}  content {:.3}  \
                 headings {}/{}  tables {}/{} alike {:.2}  {:>6.0} ms",
                case.name,
                score.loss(),
                score.character_error_rate,
                score.content_error_rate,
                score.headings_matched,
                score.headings_total,
                score.tables_matched,
                score.tables_total,
                score.table_similarity,
                case.duration.as_secs_f64() * 1000.0,
            ),
            (None, Some(error)) => format!("{:<24} FAILED: {error}", case.name),
            (None, None) => format!("{:<24} no result", case.name),
        };

        reporter.record(value, human);
    }

    let summary = json!({
        "command": "bench",
        "summary": true,
        "engine": report.engine,
        "cases": report.cases.len(),
        "failures": report.failures(),
        "mean_loss": report.mean_loss(),
        "mean_character_error_rate": report.mean_character_error_rate(),
        "mean_content_error_rate": report.mean_content_error_rate(),
        "headings": (report.headings_matched(), report.headings_total()),
        "tables": (report.tables_matched(), report.tables_total()),
        "mean_table_similarity": report.mean_table_similarity(),
        "total_duration_ms": report.total_duration().as_secs_f64() * 1000.0,
    });

    let human = format!(
        "---\n{} cases, {} failed\nmean loss {:.3}   mean error {:.3} in reading order, \
         {:.3} ignoring it\nheadings {}/{}   tables {}/{} alike {:.3}   {:.0} ms total",
        report.cases.len(),
        report.failures(),
        report.mean_loss().unwrap_or(f64::NAN),
        report.mean_character_error_rate().unwrap_or(f64::NAN),
        report.mean_content_error_rate().unwrap_or(f64::NAN),
        report.headings_matched(),
        report.headings_total(),
        report.tables_matched(),
        report.tables_total(),
        report.mean_table_similarity().unwrap_or(f64::NAN),
        report.total_duration().as_secs_f64() * 1000.0,
    );

    reporter.record(summary, human);
}

#[cfg(test)]
mod tests {
    use super::*;
    use docfusion_core::extract::{Router, text::TextPassthrough};

    fn router() -> Router {
        Router::new(vec![Box::new(TextPassthrough)])
    }

    /// Writes one case: a directory holding a source and its reference.
    fn case(directory: &Path, name: &str, source_name: &str, source: &str, reference: &str) {
        let case = directory.join(name);
        fs::create_dir_all(&case).unwrap();
        fs::write(case.join(source_name), source).unwrap();
        fs::write(case.join(REFERENCE), reference).unwrap();
    }

    #[test]
    fn loads_a_corpus_of_case_directories() {
        let dir = tempfile::tempdir().unwrap();
        case(dir.path(), "notes", "notes.txt", "hello", "hello\n");

        let cases = load(dir.path()).unwrap();

        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].name, "notes");
        assert!(cases[0].source.ends_with("notes.txt"));
        assert!(cases[0].reference.ends_with(REFERENCE));
    }

    /// A Markdown source is the case a flat layout cannot express, because the
    /// source and the reference would share a name.
    #[test]
    fn a_markdown_source_can_be_paired_with_its_reference() {
        let dir = tempfile::tempdir().unwrap();
        case(dir.path(), "notes", "notes.md", "# Title\n", "# Title\n");

        let cases = load(dir.path()).unwrap();

        assert!(cases[0].source.ends_with("notes.md"));
        assert!(cases[0].reference.ends_with(REFERENCE));
    }

    #[test]
    fn a_case_without_a_reference_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let orphan = dir.path().join("orphan");
        fs::create_dir_all(&orphan).unwrap();
        fs::write(orphan.join("notes.txt"), "hello").unwrap();

        let error = load(dir.path()).unwrap_err();

        assert!(error.to_string().contains(REFERENCE), "{error}");
    }

    #[test]
    fn a_case_without_a_source_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let orphan = dir.path().join("orphan");
        fs::create_dir_all(&orphan).unwrap();
        fs::write(orphan.join(REFERENCE), "hello").unwrap();

        let error = load(dir.path()).unwrap_err();

        assert!(error.to_string().contains("nothing to read"), "{error}");
    }

    #[test]
    fn a_case_with_two_sources_is_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        case(dir.path(), "notes", "notes.txt", "hello", "hello\n");
        fs::write(dir.path().join("notes").join("other.txt"), "hello").unwrap();

        let error = load(dir.path()).unwrap_err();

        assert!(error.to_string().contains("ambiguous"), "{error}");
    }

    #[test]
    fn cases_are_ordered_by_name() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["zebra", "alpha", "middle"] {
            case(dir.path(), name, "source.txt", name, name);
        }

        let names: Vec<String> = load(dir.path())
            .unwrap()
            .into_iter()
            .map(|case| case.name)
            .collect();

        assert_eq!(names, vec!["alpha", "middle", "zebra"]);
    }

    /// The harness has to be able to say "this is correct" before it is trusted
    /// to say which model is better.
    #[test]
    fn an_exact_reference_scores_perfectly() {
        let dir = tempfile::tempdir().unwrap();
        case(
            dir.path(),
            "notes",
            "notes.md",
            "# Title\n\nBody.\n",
            "# Title\n\nBody.\n",
        );

        let report = run(&load(dir.path()).unwrap(), &router(), "test");

        assert_eq!(report.failures(), 0);
        assert_eq!(report.mean_loss(), Some(0.0));
        assert_eq!(report.headings_matched(), 1);
        assert_eq!(report.headings_total(), 1);
    }

    #[test]
    fn a_wrong_reference_scores_worse() {
        let dir = tempfile::tempdir().unwrap();
        case(
            dir.path(),
            "notes",
            "notes.md",
            "# Title\n\nBody.\n",
            "# Something else entirely\n",
        );

        let report = run(&load(dir.path()).unwrap(), &router(), "test");

        assert!(report.mean_loss().unwrap() > 0.0);
        assert_eq!(report.headings_matched(), 0);
    }

    /// The whole point of the harness: a document that survives a round trip
    /// unchanged scores perfectly, and one that is mangled does not.
    #[test]
    fn a_markdown_document_round_trips_without_loss() {
        let dir = tempfile::tempdir().unwrap();
        let document = "# Title\n\nBody text.\n\n- one\n- two\n  - nested\n\n| a | b |\n| --- | --- |\n| c | d |\n\n> quoted\n";
        case(dir.path(), "notes", "notes.md", document, document);

        let report = run(&load(dir.path()).unwrap(), &router(), "test");

        assert_eq!(report.failures(), 0);
        assert_eq!(
            report.mean_loss(),
            Some(0.0),
            "{:?}",
            report.cases[0].score
        );
    }

    #[test]
    fn an_unreadable_source_is_reported_as_a_case_failure() {
        let dir = tempfile::tempdir().unwrap();
        case(dir.path(), "notes", "notes.txt", "hello", "hello\n");

        // The source is found at load time, so removal afterwards is what a
        // bench run has to survive.
        let cases = vec![Case {
            name: "notes".into(),
            source: dir.path().join("notes").join("missing.txt"),
            reference: dir.path().join("notes").join(REFERENCE),
        }];

        let report = run(&cases, &router(), "test");

        assert_eq!(report.failures(), 1);
        assert!(report.mean_loss().is_none());
    }

    #[test]
    fn the_engine_that_handled_a_case_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        case(dir.path(), "notes", "notes.txt", "hello", "hello\n");

        let report = run(&load(dir.path()).unwrap(), &router(), "test");

        assert_eq!(report.cases[0].engine.as_deref(), Some("text-passthrough"));
    }
}
