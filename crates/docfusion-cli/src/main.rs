//! The DocFusion command line.
//!
//! This is the full surface of the tool: the GUI is a deliberate subset of it,
//! so any capability that exists but is not reachable from here is a capability
//! an agent cannot use.

mod bench;
mod output;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use docfusion_core::extract::pdf::PdfTextLayer;
use docfusion_core::extract::{
    ExtractError, Extractor, Router, docx::DocxParser, pptx::PptxParser, text::TextPassthrough,
    xlsx::XlsxParser,
};
use docfusion_store::manifest::StoreError;
use docfusion_store::pipeline::{Mode, Pipeline, PipelineError};
use serde_json::{Value, json};

use output::{FAILED, INDEX_INCONSISTENT, NO_BACKEND, OK, PARTIAL, Reporter};

#[derive(Parser)]
#[command(
    name = "docfusion",
    version,
    about = "Turn documents into structured Markdown",
    long_about = "Reads PDFs, images and Office documents and writes structured \
                  Markdown into a knowledge base, processing each document only once."
)]
struct Cli {
    /// The knowledge base directory. Created if it does not exist.
    #[arg(long, global = true, default_value = "knowledge", env = "DOCFUSION_ROOT")]
    root: PathBuf,

    /// Write results as NDJSON on stdout. Everything else goes to stderr.
    #[arg(long, global = true)]
    json: bool,

    /// Do everything except writing.
    #[arg(long, global = true)]
    dry_run: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Process documents into the knowledge base.
    Add {
        /// Files to process.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },

    /// Print a document's Markdown.
    Show {
        /// A content id, a source path, or the output path.
        document: String,
    },

    /// List the documents in the knowledge base.
    List {
        /// Only documents with this status: done, partial or failed.
        #[arg(long)]
        status: Option<String>,
    },

    /// Repair the index against the file tree.
    Reconcile {
        /// Apply the repairs instead of only reporting them.
        #[arg(long)]
        fix: bool,
    },

    /// Score engines against a corpus of reference documents.
    Bench {
        /// Corpus directory: one subdirectory per case, holding `expected.md`
        /// beside the document it should come from.
        #[arg(long)]
        set: PathBuf,
        /// Run only this engine. Default: the whole router.
        #[arg(long)]
        engine: Option<String>,
    },

    /// Publish the output contract: commands, fields and exit codes.
    Schema,
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    Pipeline(#[from] PipelineError),

    #[error(transparent)]
    Store(#[from] StoreError),

    #[error("{0}")]
    NotFound(String),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let reporter = Reporter::new(cli.json);

    match run(&cli, &reporter) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            reporter.note(format!("error: {error}"));
            ExitCode::from(exit_code(&error))
        }
    }
}

fn run(cli: &Cli, reporter: &Reporter) -> Result<u8, CliError> {
    match &cli.command {
        Command::Add { paths } => add(cli, reporter, paths),
        Command::Show { document } => show(cli, reporter, document),
        Command::List { status } => list(cli, reporter, status.as_deref()),
        Command::Reconcile { fix } => reconcile(cli, reporter, *fix),
        Command::Bench { set, engine } => run_bench(reporter, set, engine.as_deref()),
        Command::Schema => {
            println!("{}", schema());
            Ok(OK)
        }
    }
}

/// Reports, and optionally repairs, the index against the file tree.
///
/// Drift is reported as it is found even when repairing, because what was
/// relinked or forgotten is the part a person wants to check.
fn reconcile(cli: &Cli, reporter: &Reporter, fix: bool) -> Result<u8, CliError> {
    let pipeline = open(&cli.root)?;
    let drift = docfusion_store::reconcile::scan(&pipeline)?;

    for (record, path) in &drift.relinked {
        reporter.note(format!(
            "moved  {} -> {}",
            record.output_path.display(),
            path.display()
        ));
    }
    for record in &drift.missing {
        reporter.note(format!("gone   {}", record.output_path.display()));
    }
    for path in &drift.untracked {
        reporter.note(format!("extra  {} (not tracked)", path.display()));
    }

    // An unclaimed file is reported but is not drift: the index says nothing
    // about it, so it cannot be wrong about it. A person may keep their own
    // notes beside the extracted documents.
    if !drift.needs_repair() {
        reporter.record(
            json!({
                "command": "reconcile",
                "drift": false,
                "relinked": 0,
                "forgotten": 0,
                "untracked": drift.untracked.len(),
            }),
            "the index matches the file tree",
        );
        return Ok(OK);
    }

    // `--dry-run` wins over `--fix`: a global switch that says "change nothing"
    // should not be overridden by a command-specific one that says "change this".
    if !fix || cli.dry_run {
        return Ok(INDEX_INCONSISTENT);
    }

    let repaired = docfusion_store::reconcile::repair(&pipeline, &drift)?;
    reporter.record(
        json!({
            "command": "reconcile",
            "drift": true,
            "relinked": repaired.relinked,
            "forgotten": repaired.forgotten,
            "untracked": drift.untracked.len(),
        }),
        format!(
            "relinked {}, forgot {}",
            repaired.relinked, repaired.forgotten
        ),
    );

    Ok(OK)
}

fn add(cli: &Cli, reporter: &Reporter, paths: &[PathBuf]) -> Result<u8, CliError> {
    let mut pipeline = open(&cli.root)?;
    let mode = if cli.dry_run { Mode::DryRun } else { Mode::Apply };

    let mut failures: Vec<CliError> = Vec::new();

    for path in paths {
        match pipeline.ingest(path, mode) {
            Ok(outcome) => reporter.record(
                json!({
                    "command": "add",
                    "path": path,
                    "id": outcome.id,
                    "output": outcome.output,
                    "reused": outcome.reused,
                    "conflicts": outcome.conflicts,
                    "status": outcome.status.as_str(),
                }),
                format!(
                    "{}  {}",
                    if outcome.reused { "cached " } else { "added  " },
                    outcome.output.display()
                ),
            ),
            Err(error) => {
                // A failure on one file should not abandon the rest: a batch of
                // thirty documents should not be lost to one unreadable scan.
                reporter.note(format!("{}: {error}", path.display()));
                failures.push(error.into());
            }
        }
    }

    if failures.is_empty() {
        return Ok(OK);
    }
    if failures.len() < paths.len() {
        return Ok(PARTIAL);
    }

    // Everything failed, so the run failed; the code says what kind of failure
    // it was when they all agree.
    let first = failures[0].to_string();
    if failures.iter().all(|error| error.to_string() == first) {
        return Ok(exit_code(&failures[0]));
    }
    Ok(FAILED)
}

fn show(cli: &Cli, reporter: &Reporter, document: &str) -> Result<u8, CliError> {
    let pipeline = open(&cli.root)?;
    let record = pipeline
        .manifest()
        .find(document)?
        .ok_or_else(|| CliError::NotFound(format!("no document matches `{document}`")))?;

    let output = pipeline.root().join(&record.output_path);
    let markdown = std::fs::read_to_string(&output).map_err(|error| {
        CliError::NotFound(format!("{} is unreadable: {error}", output.display()))
    })?;

    reporter.record(
        json!({
            "command": "show",
            "id": record.id,
            "output": record.output_path,
            "markdown": markdown,
        }),
        markdown.trim_end(),
    );

    Ok(OK)
}

fn list(cli: &Cli, reporter: &Reporter, status: Option<&str>) -> Result<u8, CliError> {
    let pipeline = open(&cli.root)?;

    for record in pipeline.manifest().all()? {
        if status.is_some_and(|wanted| wanted != record.status.as_str()) {
            continue;
        }

        reporter.record(
            json!({
                "command": "list",
                "id": record.id,
                "source": record.source_path,
                "output": record.output_path,
                "status": record.status.as_str(),
                "conflicts": record.conflict_count,
                "engine": record.engine.id.as_str(),
                "updated_at": record.updated_at,
            }),
            format!(
                "{}  {:<8}  {}",
                &record.id[..8],
                record.status.as_str(),
                record.output_path.display()
            ),
        );
    }

    Ok(OK)
}

fn open(root: &Path) -> Result<Pipeline, CliError> {
    Ok(Pipeline::open(root, router())?)
}

/// Measures engines against a corpus. Reads only: nothing is written and the
/// knowledge base is not involved.
fn run_bench(reporter: &Reporter, set: &Path, engine: Option<&str>) -> Result<u8, CliError> {
    let cases = bench::load(set)?;
    if cases.is_empty() {
        return Err(CliError::NotFound(format!(
            "no cases in {}; a corpus holds one subdirectory per case, \
             each with `expected.md` beside the document to read",
            set.display()
        )));
    }

    let router = router_for(engine)?;
    let report = bench::run(&cases, &router, engine.unwrap_or("router"));
    bench::report(reporter, &report);

    Ok(OK)
}

/// Every engine the tool knows about, best fit first.
fn engines() -> Vec<Box<dyn Extractor>> {
    vec![
        Box::new(PdfTextLayer::new()),
        Box::new(DocxParser),
        Box::new(XlsxParser),
        Box::new(PptxParser),
        Box::new(TextPassthrough),
    ]
}

fn router() -> Router {
    Router::new(engines())
}

/// A router holding one named engine, or all of them.
///
/// Selecting a single engine is what makes a comparison meaningful: with the
/// whole router in place, each case is handled by whichever engine happens to
/// fit, and the run measures the router rather than the engine.
fn router_for(engine: Option<&str>) -> Result<Router, CliError> {
    let Some(wanted) = engine else {
        return Ok(router());
    };

    let selected: Vec<Box<dyn Extractor>> = engines()
        .into_iter()
        .filter(|candidate| candidate.id().as_str() == wanted)
        .collect();

    if selected.is_empty() {
        let known: Vec<String> = engines().iter().map(|c| c.id().to_string()).collect();
        return Err(CliError::NotFound(format!(
            "no engine named `{wanted}`; there is: {}",
            known.join(", ")
        )));
    }

    Ok(Router::new(selected))
}

fn exit_code(error: &CliError) -> u8 {
    match error {
        // Nothing can read this input, which is a different situation from a
        // run that broke: the file is fine, the tool is missing a backend.
        CliError::Pipeline(PipelineError::Extract(ExtractError::Unsupported { .. })) => NO_BACKEND,
        _ => FAILED,
    }
}

/// The output contract, published so a caller does not have to infer it.
fn schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "docfusion output",
        "contract": {
            "stdout": "results only: NDJSON under --json, human lines otherwise",
            "stderr": "progress, warnings and errors, in every mode",
            "paths": "absolute source paths; output paths are relative to --root",
        },
        "exit_codes": {
            "0": "success",
            "1": "failure",
            "2": "some inputs succeeded and some did not",
            "3": "no engine can read this input; it needs a backend that is not configured",
            "4": "the index disagrees with the file tree; run reconcile --fix"
        },
        "commands": {
            "add": {
                "arguments": ["<paths>...", "--dry-run"],
                "emits": ["command", "path", "id", "output", "reused", "conflicts", "status"]
            },
            "show": {
                "arguments": ["<document>"],
                "emits": ["command", "id", "output", "markdown"],
                "note": "the document may be named by content id, source path, output path, or file name"
            },
            "list": {
                "arguments": ["--status <done|partial|failed>"],
                "emits": ["command", "id", "source", "output", "status", "conflicts", "engine", "updated_at"]
            },
            "reconcile": {
                "arguments": ["--fix"],
                "emits": ["command", "drift", "relinked", "forgotten", "untracked"],
                "note": "exits 4 when drift is found and not repaired"
            },
            "bench": {
                "arguments": ["--set <dir>", "--engine <id>"],
                "emits": ["command", "case", "engine", "loss", "character_error_rate", "headings", "tables", "duration_ms", "error"],
                "note": "reads only; one subdirectory per case, each holding `expected.md` beside the source document"
            },
            "schema": {
                "arguments": [],
                "note": "prints this document"
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use docfusion_core::extract::InputProbe;

    #[test]
    fn every_command_is_documented_in_the_schema() {
        let schema = schema();
        let commands = schema["commands"]
            .as_object()
            .expect("commands must be an object");

        for expected in ["add", "show", "list", "reconcile", "bench", "schema"] {
            assert!(commands.contains_key(expected), "missing {expected} in schema");
        }
    }

    #[test]
    fn choosing_an_unknown_engine_names_the_ones_that_exist() {
        let Err(error) = router_for(Some("nope")) else {
            panic!("an unknown engine should be rejected rather than ignored");
        };
        let error = error.to_string();

        assert!(error.contains("nope"), "{error}");
        assert!(error.contains("pdfium"), "{error}");
    }

    #[test]
    fn choosing_a_known_engine_selects_only_it() {
        let router = router_for(Some("pdfium")).unwrap();
        let pdf = docfusion_core::extract::InputProbe::new(b"%PDF-1.7", None, 8);
        let text = docfusion_core::extract::InputProbe::new(b"hello", None, 5);

        assert!(router.can_handle(&pdf));
        assert!(!router.can_handle(&text), "a filtered router holds one engine");
    }

    #[test]
    fn reconciling_reports_an_index_that_is_consistent() {
        let dir = tempfile::tempdir().unwrap();
        let cli = Cli::parse_from(["docfusion", "--root", dir.path().to_str().unwrap(), "reconcile"]);
        let reporter = Reporter::new(false);

        assert_eq!(reconcile(&cli, &reporter, false).unwrap(), OK);
    }

    #[test]
    fn reconciling_reports_drift_with_the_inconsistent_code() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap().to_owned();

        // Process a file, then move its output out from under the index.
        let source = dir.path().join("notes.txt");
        std::fs::write(&source, "hello").unwrap();
        let add = Cli::parse_from(["docfusion", "--root", &root, "add", source.to_str().unwrap()]);
        assert_eq!(run(&add, &Reporter::new(false)).unwrap(), OK);

        let docs = dir.path().join("docs");
        std::fs::rename(docs.join("notes.md"), docs.join("renamed.md")).unwrap();

        let cli = Cli::parse_from(["docfusion", "--root", &root, "reconcile"]);
        assert_eq!(
            reconcile(&cli, &Reporter::new(false), false).unwrap(),
            INDEX_INCONSISTENT
        );

        let fix = Cli::parse_from(["docfusion", "--root", &root, "reconcile", "--fix"]);
        assert_eq!(reconcile(&fix, &Reporter::new(false), true).unwrap(), OK);
        assert_eq!(reconcile(&cli, &Reporter::new(false), false).unwrap(), OK);
    }

    /// A person may keep their own notes beside the extracted documents. The
    /// index claims nothing about such a file, so it cannot be wrong about it.
    #[test]
    fn an_untracked_file_is_reported_but_is_not_drift() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap().to_owned();

        let source = dir.path().join("notes.txt");
        std::fs::write(&source, "hello").unwrap();
        let add = Cli::parse_from(["docfusion", "--root", &root, "add", source.to_str().unwrap()]);
        run(&add, &Reporter::new(false)).unwrap();

        std::fs::write(dir.path().join("docs").join("personal.md"), "mine").unwrap();

        let cli = Cli::parse_from(["docfusion", "--root", &root, "reconcile"]);
        assert_eq!(
            reconcile(&cli, &Reporter::new(true), false).unwrap(),
            OK,
            "an unclaimed file should not be treated as index drift"
        );
    }

    /// A global switch that says "change nothing" must not be overridden by a
    /// command-specific one that says "change this".
    #[test]
    fn dry_run_wins_over_fix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap().to_owned();

        let source = dir.path().join("notes.txt");
        std::fs::write(&source, "hello").unwrap();
        let add = Cli::parse_from(["docfusion", "--root", &root, "add", source.to_str().unwrap()]);
        run(&add, &Reporter::new(false)).unwrap();

        let docs = dir.path().join("docs");
        std::fs::rename(docs.join("notes.md"), docs.join("renamed.md")).unwrap();

        let cli = Cli::parse_from([
            "docfusion",
            "--root",
            &root,
            "--dry-run",
            "reconcile",
            "--fix",
        ]);
        assert_eq!(
            reconcile(&cli, &Reporter::new(true), true).unwrap(),
            INDEX_INCONSISTENT
        );

        let after = Cli::parse_from(["docfusion", "--root", &root, "reconcile"]);
        assert_eq!(
            reconcile(&after, &Reporter::new(false), false).unwrap(),
            INDEX_INCONSISTENT,
            "the dry run should have repaired nothing"
        );
    }

    /// A published code that cannot be emitted is a lie, and a code that is
    /// emitted but unpublished is a trap, so the two lists are checked against
    /// each other.
    #[test]
    fn the_schema_publishes_every_exit_code_the_tool_can_emit() {
        let schema = schema();
        for code in [OK, FAILED, PARTIAL, NO_BACKEND, INDEX_INCONSISTENT] {
            assert!(
                schema["exit_codes"].get(code.to_string()).is_some(),
                "exit code {code} is emitted but not published"
            );
        }
        assert_eq!(
            schema["exit_codes"].as_object().map(|codes| codes.len()),
            Some(5),
            "the schema publishes a code the tool cannot emit"
        );
    }

    #[test]
    fn an_unsupported_input_reports_the_backend_exit_code() {
        let error = CliError::Pipeline(PipelineError::Extract(ExtractError::unsupported(
            "no OCR backend",
        )));
        assert_eq!(exit_code(&error), NO_BACKEND);
    }

    #[test]
    fn any_other_failure_is_a_general_failure() {
        assert_eq!(exit_code(&CliError::NotFound("nope".into())), FAILED);
    }

    #[test]
    fn the_router_covers_every_deterministic_format() {
        let router = router();
        for probe in [
            InputProbe::new(b"%PDF-1.7", None, 8),
            InputProbe::new(b"PK\x03\x04", None, 4),
            InputProbe::new(b"plain text", None, 10),
        ] {
            assert!(
                router.can_handle(&probe),
                "no engine accepts {:?}",
                probe.container
            );
        }
    }
}

