//! The DocFusion command line.
//!
//! This is the full surface of the tool: the GUI is a deliberate subset of it,
//! so any capability that exists but is not reachable from here is a capability
//! an agent cannot use.

mod bench;
mod config;
mod doctor;
mod output;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use docfusion_core::backend::VisionBackend;
use docfusion_core::extract::pdf::PdfTextLayer;
use docfusion_core::extract::{
    ExtractError, Extractor, Router, docx::DocxParser, pptx::PptxParser, scan::ScanExtractor,
    text::TextPassthrough, xlsx::XlsxParser,
};
use docfusion_core::verify::{DEFAULT_MAX_VERIFY_PAGES, VerifyMode, VerifyPolicy};
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
    /// Default: `knowledge`.
    #[arg(long, global = true, env = "DOCFUSION_ROOT")]
    root: Option<PathBuf>,

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

    /// When a second engine should check the first. `doubt` buys a second
    /// opinion only where the first was unsure of itself or read a value whose
    /// exactness matters; `always` doubles the cost of every page.
    #[arg(long, global = true, value_enum, env = "DOCFUSION_VERIFY")]
    verify: Option<VerifyChoice>,

    /// The most pages of one document a second engine may be asked to read.
    /// Default: 50.
    #[arg(long, global = true, env = "DOCFUSION_MAX_VERIFY_PAGES")]
    max_verify_pages: Option<u32>,

    #[command(subcommand)]
    command: Command,
}

/// The verification policies, as a command line spells them.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum VerifyChoice {
    /// Where the reading was uncertain, or a value needed to be exact.
    Doubt,
    /// Only where the text holds an amount, a date or an identifier.
    Fields,
    /// Every page, whatever it says.
    Always,
    /// Never. One engine reads and nothing checks it.
    Off,
}

impl From<VerifyChoice> for VerifyMode {
    fn from(choice: VerifyChoice) -> Self {
        match choice {
            VerifyChoice::Doubt => VerifyMode::OnDoubt,
            VerifyChoice::Fields => VerifyMode::OnCriticalFields,
            VerifyChoice::Always => VerifyMode::Always,
            VerifyChoice::Off => VerifyMode::Off,
        }
    }
}

/// Where documents go when nothing says otherwise.
const DEFAULT_ROOT: &str = "knowledge";

impl Cli {
    /// The knowledge base, once nothing else is going to supply one.
    fn root(&self) -> PathBuf {
        self.root
            .clone()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_ROOT))
    }

    /// The command line with the configuration file filled in behind it.
    ///
    /// A flag beats an environment variable, which beats the file, which beats
    /// the built-in default. Clap resolves the first two into a single
    /// `Option`, so whatever it left unset is exactly what the file is allowed
    /// to supply — which is why the fields with defaults are `Option` here
    /// rather than carrying those defaults in an attribute.
    fn resolved(self) -> Result<Self, CliError> {
        self.with_config(config::Config::load()?)
    }

    /// The same, with the file already in hand.
    ///
    /// Separated from reading it so that precedence can be tested without a
    /// file and without touching the environment a parallel test is reading.
    fn with_config(mut self, config: config::Config) -> Result<Self, CliError> {
        self.root = self.root.or(config.root);
        self.endpoint = self.endpoint.or(config.endpoint);
        self.model = self.model.or(config.model);
        // An empty value is what a template line looks like before somebody
        // fills it in, and it is not a key. Treating it as one sends a bare
        // `Bearer ` and turns a blank line into an authentication failure.
        self.api_key = self
            .api_key
            .or(config.api_key)
            .filter(|key| !key.trim().is_empty());
        self.max_verify_pages = self.max_verify_pages.or(config.max_verify_pages);

        if self.verify.is_none()
            && let Some(name) = config.verify.as_deref()
        {
            self.verify = Some(VerifyChoice::from_str(name, true).map_err(|_| {
                CliError::Usage(format!(
                    "`verify = \"{name}\"` in {} is not one of: {}",
                    config::path().display(),
                    choices()
                ))
            })?);
        }

        Ok(self)
    }
}

/// The verification policies, as a person would write them.
fn choices() -> String {
    VerifyChoice::value_variants()
        .iter()
        .filter_map(|choice| choice.to_possible_value())
        .map(|value| value.get_name().to_owned())
        .collect::<Vec<_>>()
        .join(", ")
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

    /// Report what this installation can read, and what is missing to read more.
    Doctor,

    /// Publish the output contract: commands, fields and exit codes.
    Schema,
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    Pipeline(#[from] PipelineError),

    #[error(transparent)]
    Store(#[from] StoreError),

    #[error(transparent)]
    Config(#[from] config::ConfigError),

    #[error("{0}")]
    NotFound(String),

    /// The command line asks for something that cannot be honoured as given.
    #[error("{0}")]
    Usage(String),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let reporter = Reporter::new(cli.json);

    let cli = match cli.resolved() {
        Ok(cli) => cli,
        Err(error) => {
            reporter.note(format!("error: {error}"));
            return ExitCode::from(exit_code(&error));
        }
    };

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
        Command::Doctor => doctor::run(cli, reporter),
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
    Ok(Pipeline::open(&cli.root(), router(cli, policy(cli))?)?)
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

/// Builds the endpoint the command line asked for, if any.
///
/// An endpoint without a model name is a configuration mistake worth naming
/// now, rather than a request that fails later with something vaguer.
///
/// Concrete rather than the trait object the router takes, because asking the
/// endpoint what it serves is a question only this type can answer, and
/// `doctor` has to ask it.
fn endpoint(cli: &Cli) -> Result<Option<OpenAiEndpoint>, CliError> {
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

    Ok(Some(OpenAiEndpoint::new(config)))
}

fn vision(cli: &Cli) -> Result<Option<Box<dyn VisionBackend>>, CliError> {
    Ok(endpoint(cli)?.map(|endpoint| Box::new(endpoint) as Box<dyn VisionBackend>))
}

/// The engines the router is holding, named.
///
/// Built rather than hard-coded so that anything describing the installation
/// describes the one that is actually running.
fn engine_ids(cli: &Cli) -> Result<Vec<String>, CliError> {
    Ok(engines(cli, policy(cli))?
        .iter()
        .map(|engine| engine.id().to_string())
        .collect())
}

/// Every engine the tool knows about, best fit first.
///
/// The order is load-bearing: engines that are equally plausible keep their
/// registration order, and the text-layer engine comes first so that a PDF
/// which can be parsed is parsed rather than rendered into images and sent to a
/// model.
fn engines(cli: &Cli, policy: VerifyPolicy) -> Result<Vec<Box<dyn Extractor>>, CliError> {
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
    engines.push(Box::new(
        ScanExtractor::new(Some(Box::new(RapidOcrBackend::bundled())), vision(cli)?)
            .with_policy(policy),
    ));

    Ok(engines)
}

/// The verification policy the command line asked for.
fn policy(cli: &Cli) -> VerifyPolicy {
    VerifyPolicy {
        mode: cli.verify.unwrap_or(VerifyChoice::Doubt).into(),
        max_verify_pages: cli.max_verify_pages.unwrap_or(DEFAULT_MAX_VERIFY_PAGES),
        ..VerifyPolicy::default()
    }
}

fn router(cli: &Cli, policy: VerifyPolicy) -> Result<Router, CliError> {
    Ok(Router::new(engines(cli, policy)?))
}

/// A router holding one named engine, or all of them.
///
/// Selecting a single engine is what makes a comparison meaningful: with the
/// whole router in place, each case is handled by whichever engine happens to
/// fit, and the run measures the router rather than the engine.
fn router_for(cli: &Cli, engine: Option<&str>) -> Result<Router, CliError> {
    // Verification off, always. A benchmark measures what one engine reads,
    // and the conflict report is not part of a reading — leaving it on would
    // charge a model for the second engine's opinion and let a disagreement
    // change the score of a run that is meant to be about accuracy alone.
    let available = engines(cli, VerifyPolicy::off())?;

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
        "config": {
            "path": config::path().display().to_string(),
            "format": "TOML; a misspelt key is an error rather than a setting quietly ignored",
            "keys": ["root", "endpoint", "model", "api_key", "verify", "max_verify_pages"],
            "precedence": "flag, then environment variable, then this file, then the built-in default",
            "override": "DOCFUSION_CONFIG"
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
                "arguments": ["<paths>...", "--dry-run", "--verify <off|doubt|fields|always>", "--max-verify-pages <n>"],
                "emits": ["command", "path", "id", "output", "reused", "conflicts", "status"],
                "note": "conflicts counts regions a second engine read differently; both readings are written into the document under `## Conflicts`, and `verified` in the front matter says how many pages were checked at all"
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
                "note": "reads only; one subdirectory per case, each holding `expected.md` beside the source document. Verification is always off: a benchmark measures what one engine reads, and a second engine's opinion is not part of a reading"
            },
            "doctor": {
                "arguments": [],
                "emits": ["command", "root", "root_exists", "engines", "endpoint", "model", "serves", "model_listed", "tier", "notes", "problems", "config"],
                "note": "reports what this installation can read; `tier` is `vision` only when a configured endpoint answered and listed the model. Exits 1 when a configured endpoint could not be confirmed; exits 0 with no endpoint at all, which is a supported configuration"
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

    // ── Configuration file ───────────────────────────────────────────────────

    fn file(endpoint: &str, model: &str) -> config::Config {
        config::Config {
            endpoint: Some(endpoint.to_owned()),
            model: Some(model.to_owned()),
            api_key: Some("sk-from-the-file".to_owned()),
            ..config::Config::default()
        }
    }

    #[test]
    fn the_file_supplies_what_the_command_line_left_out() {
        let cli = configured(&[])
            .with_config(file("http://file:1", "from-file"))
            .expect("valid");

        assert_eq!(cli.endpoint.as_deref(), Some("http://file:1"));
        assert_eq!(cli.model.as_deref(), Some("from-file"));
        assert_eq!(cli.api_key.as_deref(), Some("sk-from-the-file"));
    }

    /// The whole point of a precedence order: a one-off run has to be able to
    /// override what is written down.
    #[test]
    fn a_flag_beats_the_file() {
        let cli = configured(&["--endpoint", "http://flag:2", "--model", "from-flag"])
            .with_config(file("http://file:1", "from-file"))
            .expect("valid");

        assert_eq!(cli.endpoint.as_deref(), Some("http://flag:2"));
        assert_eq!(cli.model.as_deref(), Some("from-flag"));
        // But a setting the flag did not mention still comes from the file.
        assert_eq!(cli.api_key.as_deref(), Some("sk-from-the-file"));
    }

    #[test]
    fn the_file_can_carry_the_settings_that_have_defaults() {
        let cli = configured(&[])
            .with_config(config::Config {
                root: Some(PathBuf::from("elsewhere")),
                verify: Some("always".into()),
                max_verify_pages: Some(3),
                ..config::Config::default()
            })
            .expect("valid");

        assert_eq!(cli.root(), PathBuf::from("elsewhere"));
        assert!(matches!(cli.verify, Some(VerifyChoice::Always)));
        assert_eq!(cli.max_verify_pages, Some(3));
    }

    #[test]
    fn with_nothing_configured_every_setting_falls_back() {
        let cli = configured(&[])
            .with_config(config::Config::default())
            .expect("valid");

        assert_eq!(cli.root(), PathBuf::from(DEFAULT_ROOT));
        assert_eq!(policy(&cli).mode, VerifyMode::OnDoubt);
        assert_eq!(policy(&cli).max_verify_pages, DEFAULT_MAX_VERIFY_PAGES);
    }

    /// A policy the tool does not have, written in a file nobody re-reads, is
    /// worth saying out loud — and worth saying what the choices are.
    #[test]
    fn an_unknown_policy_in_the_file_names_the_valid_ones() {
        let error = match configured(&[]).with_config(config::Config {
            verify: Some("sometimes".into()),
            ..config::Config::default()
        }) {
            Ok(_) => panic!("a policy the tool does not have should be refused"),
            Err(error) => error,
        };

        let message = error.to_string();
        assert!(message.contains("sometimes"), "{message}");
        assert!(message.contains("always") && message.contains("off"), "{message}");
    }

    /// `--verify` on the command line is already validated by clap, so the file
    /// must not be able to override a value that was given.
    #[test]
    fn a_flag_beats_an_unusable_file_policy() {
        let cli = configured(&["--verify", "off"])
            .with_config(config::Config {
                verify: Some("sometimes".into()),
                ..config::Config::default()
            })
            .expect("the file's value is never reached");

        assert!(matches!(cli.verify, Some(VerifyChoice::Off)));
    }

    fn png_probe() -> InputProbe {
        InputProbe::new(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A], None, 8)
    }

    /// The promise that makes the tool useful with no setup at all.
    #[test]
    fn an_image_is_readable_with_nothing_configured() {
        let router = router(&configured(&[]), VerifyPolicy::default()).unwrap();

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
        let without = router(&configured(&[]), VerifyPolicy::default()).unwrap();
        let with = router(&configured(&["--endpoint", "http://127.0.0.1:1", "--model", "m"]), VerifyPolicy::default())
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
        let router = router(&configured(&[]), VerifyPolicy::default()).unwrap();
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

