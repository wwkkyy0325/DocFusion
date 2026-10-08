//! The DocFusion command line.
//!
//! This is the full surface of the tool: the GUI is a deliberate subset of it,
//! so any capability that exists but is not reachable from here is a capability
//! an agent cannot use.

mod output;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use docfusion_core::extract::pdf::PdfTextLayer;
use docfusion_core::extract::{
    ExtractError, Router, docx::DocxParser, pptx::PptxParser, text::TextPassthrough,
    xlsx::XlsxParser,
};
use docfusion_store::manifest::StoreError;
use docfusion_store::pipeline::{Mode, Pipeline, PipelineError};
use serde_json::{Value, json};

use output::{FAILED, NO_BACKEND, OK, PARTIAL, Reporter};

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
        Command::Schema => {
            println!("{}", schema());
            Ok(OK)
        }
    }
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

/// Every engine the tool knows about, best fit first.
fn router() -> Router {
    Router::new(vec![
        Box::new(PdfTextLayer::new()),
        Box::new(DocxParser),
        Box::new(XlsxParser),
        Box::new(PptxParser),
        Box::new(TextPassthrough),
    ])
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
            "3": "no engine can read this input; it needs a backend that is not configured"
        },
        "commands": {
            "add": {
                "arguments": ["<paths>...", "--dry-run"],
                "emits": ["command", "path", "id", "output", "reused", "conflicts", "status"]
            },
            "show": {
                "arguments": ["<document>"],
                "emits": ["command", "id", "output", "markdown"],
                "note": "the document may be named by content id, source path, or output path"
            },
            "list": {
                "arguments": ["--status <done|partial|failed>"],
                "emits": ["command", "id", "source", "output", "status", "conflicts", "engine", "updated_at"]
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

        for expected in ["add", "show", "list", "schema"] {
            assert!(commands.contains_key(expected), "missing {expected} in schema");
        }
    }

    /// A published code that cannot be emitted is a lie, and a code that is
    /// emitted but unpublished is a trap, so the two lists are checked against
    /// each other.
    #[test]
    fn the_schema_publishes_every_exit_code_the_tool_can_emit() {
        let schema = schema();
        for code in [OK, FAILED, PARTIAL, NO_BACKEND] {
            assert!(
                schema["exit_codes"].get(code.to_string()).is_some(),
                "exit code {code} is emitted but not published"
            );
        }
        assert_eq!(
            schema["exit_codes"].as_object().map(|codes| codes.len()),
            Some(4),
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

