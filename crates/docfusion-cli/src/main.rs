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
use docfusion_core::backend::VisionBackend;
use docfusion_core::extract::pdf::PdfTextLayer;
use docfusion_core::extract::{
    ExtractError, Extractor, Router, docx::DocxParser, pptx::PptxParser, scan::ScanExtractor,
    text::TextPassthrough, xlsx::XlsxParser,
};
use docfusion_store::endpoint::{EndpointConfig, OpenAiEndpoint};
use docfusion_store::ocr::RapidOcrBackend;
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

    /// An OpenAI-compatible endpoint for reading scanned pages, such as
    /// `http://127.0.0.1:8080`. Without one, only documents with a text layer
    /// can be read. See the setup guide.
    #[arg(long, global = true, env = "DOCFUSION_ENDPOINT")]
    endpoint: Option<String>,

    /// The model the endpoint should serve. Required with `--endpoint`, because
    /// an endpoint may host several.
    #[arg(long, global = true, env = "DOCFUSION_MODEL")]
    model: Option<String>,

    /// Sent as a bearer token, for endpoints that require one.
    #[arg(long, global = true, env = "DOCFUSION_API_KEY")]
    api_key: Option<String>,

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

    /// The command line asks for something that cannot be honoured as given.
    #[error("{0}")]
    Usage(String),
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
        Command::Bench { set, engine } => run_bench(cli, reporter, set, engine.as_deref()),
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
    let pipeline = open(cli)?;
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
    let mut pipeline = open(cli)?;
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
    let pipeline = open(cli)?;
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
    let pipeline = open(cli)?;

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

fn open(cli: &Cli) -> Result<Pipeline, CliError> {
    Ok(Pipeline::open(&cli.root, router(cli)?)?)
}

/// Measures engines against a corpus. Reads only: nothing is written and the
/// knowledge base is not involved.
fn run_bench(
    cli: &Cli,
    reporter: &Reporter,
    set: &Path,
    engine: Option<&str>,
) -> Result<u8, CliError> {
    let cases = bench::load(set)?;
    if cases.is_empty() {
        return Err(CliError::NotFound(format!(
            "no cases in {}; a corpus holds one subdirectory per case, \
             each with `expected.md` beside the document to read",
            set.display()
        )));
    }

    let router = router_for(cli, engine)?;
    let report = bench::run(&cases, &router, engine.unwrap_or("router"));
    bench::report(reporter, &report);

    Ok(OK)
}

/// Builds the vision backend the command line asked for, if any.
///
/// An endpoint without a model name is a configuration mistake worth naming
/// now, rather than a request that fails later with something vaguer.
fn vision(cli: &Cli) -> Result<Option<Box<dyn VisionBackend>>, CliError> {
    let Some(url) = cli.endpoint.as_deref() else {
        if cli.model.is_some() || cli.api_key.is_some() {
            return Err(CliError::Usage(
                "--model and --api-key only mean something with --endpoint".into(),
            ));
        }
        return Ok(None);
    };

    let model = cli.model.clone().ok_or_else(|| {
        CliError::Usage(format!(
            "--endpoint {url} needs --model: an endpoint may serve several, and guessing \
             which one would silently produce whatever it happened to pick"
        ))
    })?;

    let mut config = EndpointConfig::new(url, model);
    if let Some(key) = &cli.api_key {
        config = config.with_api_key(key.clone());
    }

    Ok(Some(Box::new(OpenAiEndpoint::new(config))))
}

/// Every engine the tool knows about, best fit first.
///
/// The order is load-bearing: engines that are equally plausible keep their
/// registration order, and the text-layer engine comes first so that a PDF
/// which can be parsed is parsed rather than rendered into images and sent to a
/// model.
fn engines(cli: &Cli) -> Result<Vec<Box<dyn Extractor>>, CliError> {
    let mut engines: Vec<Box<dyn Extractor>> = vec![
        Box::new(PdfTextLayer::new()),
        Box::new(DocxParser),
        Box::new(XlsxParser),
        Box::new(PptxParser),
        Box::new(TextPassthrough),
    ];

    // The local OCR engine always joins, and costs nothing until a page
    // actually needs reading. It is what makes a scan readable with no setup:
    // the weights are in the binary, so there is nothing to configure and
    // nothing to download.
    engines.push(Box::new(ScanExtractor::new(
        Some(Box::new(RapidOcrBackend::bundled())),
        vision(cli)?,
    )));

    Ok(engines)
}

fn router(cli: &Cli) -> Result<Router, CliError> {
    Ok(Router::new(engines(cli)?))
}

/// A router holding one named engine, or all of them.
///
/// Selecting a single engine is what makes a comparison meaningful: with the
/// whole router in place, each case is handled by whichever engine happens to
/// fit, and the run measures the router rather than the engine.
fn router_for(cli: &Cli, engine: Option<&str>) -> Result<Router, CliError> {
    let available = engines(cli)?;

    let Some(wanted) = engine else {
        return Ok(Router::new(available));
    };

    let known: Vec<String> = available.iter().map(|c| c.id().to_string()).collect();
    let selected: Vec<Box<dyn Extractor>> = available
        .into_iter()
        .filter(|candidate| candidate.id().as_str() == wanted)
        .collect();

    if selected.is_empty() {
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

    /// A command line with whatever global options a test cares about.
    ///
    /// A subcommand is mandatory, so one is appended that the caller cannot
    /// mean: these tests are about building the engine list, not running it.
    fn configured(extra: &[&str]) -> Cli {
        let mut args = vec!["docfusion"];
        args.extend_from_slice(extra);
        args.push("schema");
        Cli::parse_from(args)
    }

    fn png_probe() -> InputProbe {
        InputProbe::new(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A], None, 8)
    }

    /// The promise that makes the tool useful with no setup at all.
    #[test]
    fn an_image_is_readable_with_nothing_configured() {
        let router = router(&configured(&[])).unwrap();

        assert!(
            router.can_handle(&png_probe()),
            "the bundled OCR engine should be there without configuration"
        );
    }

    /// Pointing the tool at a vision endpoint changes what will read a scanned
    /// page, so it has to change what the cache is keyed on — otherwise the
    /// documents read by the previous configuration are served as if they had
    /// been read by this one.
    #[test]
    fn an_endpoint_changes_what_the_scan_engine_is_keyed_on() {
        let without = router(&configured(&[])).unwrap();
        let with = router(&configured(&["--endpoint", "http://127.0.0.1:1", "--model", "m"]))
            .unwrap();

        let scan = docfusion_core::ir::EngineId::from("scan");

        assert_ne!(
            without.stamp_of(&scan).unwrap().model,
            with.stamp_of(&scan).unwrap().model
        );
    }

    /// An endpoint may serve several models, so guessing which would silently
    /// produce whatever it happened to pick.
    #[test]
    fn an_endpoint_without_a_model_is_refused() {
        let Err(error) = vision(&configured(&["--endpoint", "http://127.0.0.1:1"])) else {
            panic!("an endpoint without a model should be refused");
        };

        assert!(error.to_string().contains("--model"), "{error}");
    }

    #[test]
    fn a_model_without_an_endpoint_is_refused() {
        let Err(error) = vision(&configured(&["--model", "m"])) else {
            panic!("a model without an endpoint should be refused");
        };

        assert!(error.to_string().contains("--endpoint"), "{error}");
    }

    #[test]
    fn choosing_an_unknown_engine_names_the_ones_that_exist() {
        let Err(error) = router_for(&configured(&[]), Some("nope")) else {
            panic!("an unknown engine should be rejected rather than ignored");
        };
        let error = error.to_string();

        assert!(error.contains("nope"), "{error}");
        assert!(error.contains("pdfium"), "{error}");
    }

    #[test]
    fn choosing_a_known_engine_selects_only_it() {
        let router = router_for(&configured(&[]), Some("pdfium")).unwrap();
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
        let router = router(&configured(&[])).unwrap();
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

