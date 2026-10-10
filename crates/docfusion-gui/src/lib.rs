//! The window, and the two directions between it and the pipeline.
//!
//! There is no server behind this. The window is a view over the same process
//! the pipeline runs in, so a document reaches the screen without crossing a
//! socket and there is no port for anything to collide with.
//!
//! `state.rs` is what the interface is showing, as plain data with tests.
//! This file is the other half: getting work done and getting the answer back.
//! The engines are built by `docfusion_store::engines`, the same call the
//! command line makes, so the two read a document identically.

mod state;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use docfusion_core::backend::{PageRequest, VisionBackend};
use docfusion_core::ir::Shapes;
use docfusion_core::extract::{ExtractError, Router};
use docfusion_core::render;
use docfusion_core::verify::{VerifyMode, VerifyPolicy};
use docfusion_store::config;
use docfusion_store::engines;
use docfusion_store::layout;
use docfusion_store::memory;
use docfusion_store::pipeline::{Mode, Pipeline, PipelineError};
use docfusion_store::{structure, table};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use state::{Cause, Conflict, Local, Model, Opened, Reading, Record, Remote, Test, Trouble, View};

/// The application's shared state.
///
/// One knowledge base and one view, because that is what the window shows. A
/// job at a time, because a person reads one document at a time and the
/// alternative is two jobs competing for the same pipelines.
pub struct App {
    root: PathBuf,
    /// Behind a lock because the settings tab can change it while the window is
    /// open. A window that only picked up a new endpoint after a restart would
    /// be a window whose settings page does not work.
    endpoint: Mutex<Option<docfusion_store::endpoint::EndpointConfig>>,
    policy: VerifyPolicy,
    readers: usize,
    view: Mutex<View>,
    /// The stop flag of a fetch that is running, if one is.
    ///
    /// Held here rather than inside the fetch because stopping it is a command
    /// of its own, arriving on a different thread from the one doing the work.
    fetching: Mutex<Option<Arc<AtomicBool>>>,
}

impl App {
    fn new() -> Self {
        // A configuration file that cannot be read is reported rather than
        // defaulted past. The command line refuses it outright; the window
        // cannot refuse to open, so it opens saying what is wrong — which is
        // the same answer, delivered where somebody can act on it.
        //
        // Both ways a settings file goes wrong say the same thing to the person
        // reading it, that the file is the problem, because which way it went
        // wrong is in the account underneath.
        let (config, problem) = match config::Config::load() {
            Ok(config) => (config, None),
            Err(error) => (
                config::Config::default(),
                Some(Trouble::new(Cause::Settings, error.to_string())),
            ),
        };

        let root = config
            .root
            .clone()
            .unwrap_or_else(docfusion_store::dirs::default_knowledge_base);
        let readers = config
            .max_ocr_pipelines
            .unwrap_or_else(memory::page_concurrency);

        let mut policy = VerifyPolicy::default();
        if let Some(pages) = config.max_verify_pages {
            policy.max_verify_pages = pages;
        }
        policy.mode = match config.verify.as_deref() {
            // A value the command line would refuse is refused here by being
            // ignored, since the window is not the place to explain a typo in a
            // file the command line already explains it in.
            Some("off") => VerifyMode::Off,
            Some("always") => VerifyMode::Always,
            Some("fields") => VerifyMode::OnCriticalFields,
            _ => VerifyMode::OnDoubt,
        };
        policy.max_verify_pages = policy.max_verify_pages.max(1);

        let endpoint = endpoint_of(&config);

        let mut view = View::new(
            root.display().to_string(),
            config::path().display().to_string(),
        );
        view.problem = problem;

        Self {
            root,
            endpoint: Mutex::new(endpoint),
            policy,
            readers,
            view: Mutex::new(view),
            fetching: Mutex::new(None),
        }
    }

    fn view(&self) -> View {
        let mut view = self
            .view
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        // The settings are read from the file rather than remembered from
        // startup: it is a file people edit by hand and the command line reads
        // it too, so the tab should show what the next document will actually
        // be read with rather than what was there when the window opened.
        view.model = self.model();
        view.local = self.local();

        view
    }

    /// Which of the two models this app reads a page with are on disk.
    ///
    /// The two answers come from the same remembered check the engine makes
    /// before it decides what to call itself, rather than from a fresh digest
    /// of 124 MB on every view — which is also why fetching has to forget that
    /// check, since otherwise the window would offer to download something it
    /// had just been given.
    fn local(&self) -> Local {
        let root = layout::default_root();
        let structure = structure::available();
        let tables = table::available();

        Local {
            structure,
            tables,
            fetching: self
                .fetching
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some(),
            needed_bytes: [(&layout::STRUCTURE, structure), (&layout::TABLE, tables)]
                .into_iter()
                .filter(|(_, here)| !here)
                .map(|(spec, _)| missing_bytes(spec, &root))
                .sum(),
        }
    }

    /// Puts a fetch in flight, and hands back the flag that stops it.
    ///
    /// Refused when one is already running, because two fetches writing the
    /// same two files would each see the other's partial download and resume
    /// from a length that is not theirs.
    fn begin_fetch(&self) -> Result<Arc<AtomicBool>, String> {
        let mut slot = self
            .fetching
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if slot.is_some() {
            return Err("a download is already running".to_owned());
        }

        let flag = Arc::new(AtomicBool::new(false));
        *slot = Some(flag.clone());
        Ok(flag)
    }

    fn end_fetch(&self) {
        let mut slot = self
            .fetching
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = None;
    }

    /// Asks a running fetch to stop. Nothing to stop is not a failure.
    fn stop_fetch(&self) {
        let slot = self
            .fetching
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(flag) = slot.as_ref() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Reads one document, and puts what came of it on screen.
    ///
    /// A document that will not read is not an error here, it is one of the
    /// things the window can show, and it goes into the view like everything
    /// else. Returning it as an error as well would be two ways of saying the
    /// same thing and two chances for them to disagree.
    fn process(&self, source: &Path) {
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| source.display().to_string());

        {
            let mut view = self.view.lock().unwrap_or_else(|p| p.into_inner());
            view.working(name.clone());
        }

        match self.ingest(source) {
            Ok(opened) => {
                let mut view = self.view.lock().unwrap_or_else(|p| p.into_inner());
                view.ready(opened);
                self.refresh(&mut view);
            }
            Err(trouble) => {
                let mut view = self.view.lock().unwrap_or_else(|p| p.into_inner());
                view.failed(name, trouble);
            }
        }
    }

    fn ingest(&self, source: &Path) -> Result<Opened, Trouble> {
        let mut pipeline = self.pipeline()?;

        let outcome = pipeline
            .ingest(source, Mode::Apply)
            .map_err(|error| Trouble::new(cause_of(&error), error.to_string()))?;

        let output = pipeline.root().join(&outcome.output);
        self.open_at(&outcome.id, &output, outcome.reused)
    }

    /// Loads a document that is already in the knowledge base.
    ///
    /// Takes the name as well as the id because the name is what the window
    /// puts on screen, and the caller is the one that has it: this is reached
    /// from the list of records, where the name was just read out.
    fn open(&self, id: &str, name: &str) {
        match self.open_record(id) {
            Ok(opened) => {
                let mut view = self.view.lock().unwrap_or_else(|p| p.into_inner());
                view.ready(opened);
            }
            Err(trouble) => {
                let mut view = self.view.lock().unwrap_or_else(|p| p.into_inner());
                view.failed(name, trouble);
            }
        }
    }

    fn open_record(&self, id: &str) -> Result<Opened, Trouble> {
        let pipeline = self.pipeline()?;

        let record = pipeline
            .manifest()
            .find(id)
            .map_err(|error| Trouble::new(Cause::Index, error.to_string()))?
            .ok_or_else(|| Trouble::new(Cause::Missing, id.to_owned()))?;

        // Nothing was read to get here, so the window must not say it was.
        let output = pipeline.root().join(&record.output_path);
        self.open_at(&record.id, &output, true)
    }

    fn pipeline(&self) -> Result<Pipeline, Trouble> {
        let endpoint = self
            .endpoint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();

        let router = Router::new(engines::standard(endpoint, self.policy.clone(), self.readers));

        Pipeline::open(&self.root, router)
            .map_err(|error| Trouble::new(cause_of(&error), error.to_string()))
    }

    /// Asks the endpoint to read a number off a page.
    ///
    /// The smallest question that tells "reachable" from "can see". A model
    /// behind a correct address with a correct key and no vision answers this
    /// with confident nonsense about nothing, and a person who finds that out
    /// by pointing the tool at a real document has spent a document finding
    /// out.
    fn test(&self) -> Test {
        // Shorter than the timeout a real page gets. A button that can hang for
        // three minutes on an address with nothing behind it is a button
        // somebody concludes is broken.
        const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

        let Some(config) = self
            .endpoint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        else {
            return Test::Failed {
                detail: "no endpoint is configured".to_owned(),
            };
        };

        let endpoint = docfusion_store::endpoint::OpenAiEndpoint::new(config.with_timeout(TEST_TIMEOUT));
        let page = docfusion_core::image::number_page(TEST_NUMBER);
        let request = PageRequest {
            image: &page,
            page: 0,
        };

        match endpoint.parse(&request) {
            Ok(blocks) => {
                let said = blocks
                    .iter()
                    .map(|block| block.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let expected = TEST_NUMBER.to_string();

                if said.contains(&expected) {
                    Test::Read
                } else {
                    Test::Misread { said, expected }
                }
            }
            Err(error) => Test::Failed {
                detail: error.to_string(),
            },
        }
    }

    /// What the settings tab shows, read from the file rather than remembered.
    ///
    /// Remembered would drift: this is a file somebody edits by hand, and the
    /// command line reads it too. Reading it here means the tab always shows
    /// what the next document will actually be read with.
    fn model(&self) -> Model {
        match config::Config::load() {
            Ok(config) => model_of(&config),
            // The banner at the top already says the file is not usable. There
            // is nothing to add by saying it twice, and blanks are honest about
            // what is known.
            Err(_) => Model::default(),
        }
    }

    /// Writes the settings and starts using them.
    ///
    /// The file first: if it cannot be written, the window must not go on
    /// reading documents with a setting the person believes they changed. An
    /// endpoint with no model, or a model with no endpoint, is left as it was
    /// and reported, since half of the pair reads nothing.
    fn save_model(
        &self,
        endpoint: &str,
        model: &str,
        key: &str,
        reasoning: bool,
        enabled: bool,
    ) -> Result<(), String> {
        let (endpoint, model) = (endpoint.trim(), model.trim());

        if endpoint.is_empty() != model.is_empty() {
            return Err("the endpoint and the model are set together or not at all".to_owned());
        }

        config::Config::save_model(&config::path(), endpoint, model, key, reasoning, enabled)
            .map_err(|error| error.to_string())?;

        // Re-read rather than reuse what was typed: when the key box was left
        // empty the key that is now in effect is the one already in the file,
        // and this is the only way to pick it up.
        *self
            .endpoint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            config::Config::load().ok().as_ref().and_then(endpoint_of);

        Ok(())
    }

    /// Builds what the window shows for a document that is on disk.
    ///
    /// The Markdown is read back rather than kept from the extraction, so that
    /// what is on screen is what is in the knowledge base — including when the
    /// answer came from the index and nothing was extracted at all.
    fn open_at(&self, id: &str, output: &Path, reused: bool) -> Result<Opened, Trouble> {
        let markdown = std::fs::read_to_string(output)
            .map_err(|error| Trouble::new(Cause::Unreadable, format!("{}: {error}", output.display())))?;

        let engines = engines_of(&markdown);
        let conflicts = markdown
            .lines()
            .find_map(|line| line.strip_prefix("conflict_count: "))
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0);

        // The document and the report about it are shown apart. In the file
        // they belong together — a disagreement has to travel with the document
        // it is about — but on a screen, a report on the document sitting
        // inside the document reads as though somebody had written it into the
        // page.
        let (document, report) = split_report(&markdown);

        Ok(Opened {
            id: id.to_owned(),
            name: output
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default(),
            output: output.display().to_string(),
            html: render::html(document),
            engines,
            conflicts,
            report: conflicts_of(report),
            reused,
        })
    }

    fn refresh(&self, view: &mut View) {
        if let Ok(pipeline) = self.pipeline()
            && let Ok(records) = pipeline.manifest().all()
        {
            view.records = records.iter().map(Record::from_stored).collect();
        }
    }
}

/// The number the test page shows, and the number the answer has to contain.
///
/// Four digits and no repeats, so an answer that reads the right shapes in the
/// wrong order is still wrong and still caught by looking for the whole number.
const TEST_NUMBER: u32 = 4_321;

/// Where the report this program appends to a document begins.
const REPORT_HEADING: &str = "## Conflicts";

/// Splits a document from the report this program appends to it.
///
/// The heading is the marker, and it is a heading this program writes, so
/// finding it is reading back what was written rather than guessing at somebody
/// else's Markdown.
fn split_report(markdown: &str) -> (&str, &str) {
    let mut offset = 0;

    for line in markdown.split_inclusive('\n') {
        if line.trim_end() == REPORT_HEADING {
            return (&markdown[..offset], &markdown[offset + line.len()..]);
        }
        offset += line.len();
    }

    (markdown, "")
}

/// Reads the report back into the findings it lists.
///
/// The shape is the one the renderer writes: a line at the left margin naming
/// the page and the kind of disagreement, then one indented block per engine,
/// the first line of it the engine's name and the rest of it what that engine
/// read.
///
/// A report this does not understand yields nothing rather than something
/// wrong. Nothing is lost by that: the document is still on screen either way,
/// and what is missing is the separate view of the doubt rather than any text.
fn conflicts_of(report: &str) -> Vec<Conflict> {
    /// A reading being collected. Its lines are kept apart until the block
    /// ends, because an empty line *inside* a reading is one of its lines and
    /// not the end of it — the renderer indents every line it writes, so a line
    /// at the left margin is the only thing that closes a block.
    struct Open {
        engine: String,
        lines: Vec<String>,
    }

    fn close(conflicts: &mut [Conflict], open: &mut Option<Open>) {
        let Some(open) = open.take() else { return };
        let Some(conflict) = conflicts.last_mut() else {
            return;
        };

        let (engine, shapes) = engine_and_shapes(&open.engine);

        conflict.readings.push(Reading {
            engine,
            text: open.lines.join("\n"),
            shapes,
        });
    }

    let mut conflicts: Vec<Conflict> = Vec::new();
    let mut open: Option<Open> = None;

    for line in report.lines() {
        match line.strip_prefix("    ") {
            // Indented: either the name of a reading or one of its lines.
            Some(indented) => match &mut open {
                Some(open) => open.lines.push(indented.to_owned()),
                None => {
                    open = Some(Open {
                        engine: indented.to_owned(),
                        lines: Vec::new(),
                    });
                }
            },
            // At the left margin: whatever was being collected is finished, and
            // this may be the start of the next finding.
            None => {
                close(&mut conflicts, &mut open);

                if let Some((page, kind)) = page_and_kind(line) {
                    conflicts.push(Conflict {
                        page,
                        kind,
                        readings: Vec::new(),
                    });
                }
            }
        }
    }

    close(&mut conflicts, &mut open);

    conflicts
}

/// `rapidocr — headings 0, lists 0, tables 0, text 12` — the name of an engine
/// and what it read the region as.
///
/// The counts are kept out of the name rather than folded into it, because the
/// name is what everything else calls this engine and the counts are about one
/// reading. A line that named an engine and then said something this does not
/// understand keeps the name: the name is the part worth having.
fn engine_and_shapes(line: &str) -> (String, Option<Shapes>) {
    let Some((engine, counts)) = line.split_once(" — ") else {
        return (line.to_owned(), None);
    };

    let count = |label: &str| -> Option<u32> {
        counts
            .split(", ")
            .find_map(|part| part.strip_prefix(label)?.trim().parse().ok())
    };

    // All four or none: a line that gave three of the counts did not give the
    // shape, and a shape with a zero in it that nobody wrote would be a claim
    // about the reading rather than a reading of the line.
    let shapes = (|| {
        Some(Shapes {
            headings: count("headings ")?,
            lists: count("lists ")?,
            tables: count("tables ")?,
            text: count("text ")?,
        })
    })();

    (engine.to_owned(), shapes)
}

/// `Page 12: the engines read it differently.` — the line that starts a finding.
///
/// The kind is kept as it was written rather than turned into a variant here.
/// This is a window reading back what the tool wrote, and one that recognised
/// the sentences it knows and dropped the ones it did not would lose a finding
/// without saying so. The frontend holds the table of kinds and shows anything
/// outside it as it arrived.
fn page_and_kind(line: &str) -> Option<(u32, String)> {
    let rest = line.strip_prefix("Page ")?;
    let (page, kind) = rest.split_once(": ")?;

    Some((
        page.trim().parse().ok()?,
        kind.trim_end_matches('.').to_owned(),
    ))
}

/// The engines a document was read by, taken from its own front matter.
///
/// The front matter is written by this tool in a shape this tool chose, so
/// reading one field back out of it is not parsing YAML — it is reading back
/// what was written, and it says so rather than reaching for a parser.
fn engines_of(markdown: &str) -> Vec<String> {
    markdown
        .lines()
        .find_map(|line| line.strip_prefix("engines: ["))
        .and_then(|rest| rest.strip_suffix(']'))
        .map(|list| {
            list.split(',')
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The endpoint a configuration describes, or none when it describes half of
/// one.
///
/// Half of one, because one endpoint may serve several models: taking the
/// endpoint's word for which model to use would silently read documents with
/// whatever it happened to be serving. The command line refuses the pair
/// outright; the window cannot be shown a refusal before it has drawn anything,
/// so it reads with the bundled engine and says what is missing.
fn endpoint_of(config: &config::Config) -> Option<docfusion_store::endpoint::EndpointConfig> {
    // Absent means on: a file written before the switch existed keeps reading
    // documents the way it always did.
    if !config.enabled.unwrap_or(true) {
        return None;
    }

    let (Some(url), Some(model)) = (&config.endpoint, &config.model) else {
        return None;
    };

    let mut endpoint = docfusion_store::endpoint::EndpointConfig::new(url, model)
        .with_reasoning(config.reasoning.unwrap_or(false));

    if let Some(key) = config.api_key.clone() {
        endpoint = endpoint.with_api_key(key);
    }

    Some(endpoint)
}

/// What the settings tab shows. The length of the key, never the key.
fn model_of(config: &config::Config) -> Model {
    let key = config.api_key.as_deref().unwrap_or_default().trim();

    Model {
        endpoint: config.endpoint.clone().unwrap_or_default(),
        model: config.model.clone().unwrap_or_default(),
        key_length: key.chars().count(),
        reasoning: config.reasoning.unwrap_or(false),
        enabled: config.enabled.unwrap_or(true),
    }
}

/// What kind of failure this is, in the window's own terms.
///
/// The message a library error carries is English, and it is the message the
/// command line prints. The window has to say the same thing in the reader's
/// language, so it needs the kind rather than the sentence: deciding what to
/// say by matching on a sentence comes apart the moment somebody rewords it.
///
/// Written out rather than defaulted, on purpose. Nothing here is exhaustive by
/// accident: a new way for the library to fail stops this compiling, which is
/// the window being made to decide how to say it instead of finding out later
/// that it has nothing to say.
fn cause_of(error: &PipelineError) -> Cause {
    match error {
        PipelineError::Extract(error) => extract_cause(error),
        PipelineError::Store(_) => Cause::Index,
        PipelineError::Read { .. } => Cause::Unreadable,
        PipelineError::Write { .. } => Cause::Unwritable,
        PipelineError::UnidentifiableEngine => Cause::NothingToRead,
    }
}

fn extract_cause(error: &ExtractError) -> Cause {
    match error {
        // The engine that failed is named in the account. Which one it was is
        // not something the person reading this can act on differently — a
        // page that would not read is a page that would not read.
        ExtractError::Failed { .. } => Cause::EngineFailed,
        ExtractError::Unsupported { .. } => Cause::Unsupported,
        ExtractError::Io(_) => Cause::Unreadable,
        ExtractError::Image(_) => Cause::Unreadable,
    }
}

/// How much of a model that is not here would still have to arrive.
///
/// Answered from the file's length rather than from its digest, because this
/// feeds a sentence about megabytes and the digest of the layout model is
/// 124 MB of hashing. The caller only asks about a model it already knows is
/// missing, so a file of the full length here is one that failed its digest and
/// is replaced rather than resumed: the whole of it is still to come.
fn missing_bytes(spec: &layout::Spec, root: &Path) -> u64 {
    let have = std::fs::metadata(layout::path_of(spec, root))
        .map(|meta| meta.len())
        .unwrap_or(0);

    if have >= spec.bytes {
        spec.bytes
    } else {
        spec.bytes - have
    }
}

// ── The two directions ───────────────────────────────────────────────────────

#[tauri::command]
fn view(app: AppHandle) -> View {
    app.state::<App>().view()
}

#[tauri::command]
fn records(app: AppHandle) -> View {
    let state = app.state::<App>();
    let mut view = state.view();

    // The list is drawn from the index, which is a file, so it is re-read
    // rather than remembered. Everything that writes it is this process.
    let mut fresh = state.view();
    state.refresh(&mut fresh);
    view.records = fresh.records;

    view
}

/// Asks the operating system for a file.
///
/// Done here rather than in the webview because the webview's picker is a
/// plugin with a JavaScript half, and this window has no build chain to install
/// one with. The native dialog is the same dialog either way.
#[tauri::command]
async fn choose_file(app: AppHandle, title: String) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;

    let picked = app.dialog().file().set_title(title).blocking_pick_file();

    picked.map(|path| path.to_string())
}

/// Reads a document, on a thread of its own.
///
/// Reading a scan takes seconds a page and a long document takes minutes, so
/// the work goes to the blocking pool rather than to the runtime the window is
/// waiting on. The window stays drawn while it happens; `Working` is what it
/// shows.
///
/// A document that will not read comes back as a view, not as an error. The
/// only thing that rejects here is the task itself not finishing, which the
/// frontend has no view for and says so in its own words.
#[tauri::command]
async fn process(app: AppHandle, path: String) -> Result<View, String> {
    let handle = app.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<App>();
        state.process(Path::new(&path));
        state.view()
    })
    .await
    .map_err(|error| error.to_string())
}

/// Asks the endpoint which models it is serving.
///
/// Takes the address and the key from the form rather than from the file,
/// because the question is asked while somebody is filling the form in — often
/// before anything has been saved, and often about an address they have just
/// changed.
///
/// An empty key means the one already on file, for the same reason the key
/// field is empty: the window never holds it.
#[tauri::command]
async fn probe_models(endpoint: String, key: String) -> Remote {
    let key = if key.trim().is_empty() {
        config::Config::load().ok().and_then(|config| config.api_key)
    } else {
        Some(key)
    };

    let mut config = docfusion_store::endpoint::EndpointConfig::new(endpoint, "")
        .with_timeout(std::time::Duration::from_secs(20));

    if let Some(key) = key.filter(|key| !key.trim().is_empty()) {
        config = config.with_api_key(key);
    }

    let probe = docfusion_store::endpoint::OpenAiEndpoint::new(config).probe();

    match probe {
        Ok(docfusion_store::endpoint::Probe::Models(models)) => {
            // Only what can be shown a page is offered. A model that cannot see
            // answers every scan with fluent text about nothing, so offering it
            // is offering a configuration that fails on the first page. What
            // said nothing is offered, because most servers say nothing and
            // hiding on silence would hide everything.
            let (offered, blind): (Vec<_>, Vec<_>) =
                models.into_iter().partition(|model| model.reads_images());

            Remote::Models {
                names: offered.into_iter().map(|model| model.id).collect(),
                blind: blind.len(),
            }
        }
        Ok(docfusion_store::endpoint::Probe::Unlisted(detail)) => Remote::Unlisted { detail },
        Err(error) => Remote::Failed {
            detail: error.to_string(),
        },
    }
}

/// Asks the endpoint to read a page it has never seen, and says what came of
/// it.
///
/// Off the main thread, because it is a network call with a model at the far
/// end of it and the window has to stay drawn while it happens.
#[tauri::command]
async fn test_model(app: AppHandle) -> Result<Test, String> {
    let handle = app.clone();

    tauri::async_runtime::spawn_blocking(move || Ok(handle.state::<App>().test()))
        .await
        .map_err(|error| error.to_string())?
}

/// The key itself, for the one case that needs it: somebody pressed the eye.
///
/// Deliberately not part of the view, which is serialised for every command the
/// window sends. A secret that travels with every document read is a secret in
/// more places than the one place that asks for it. Here it is asked for, and
/// it crosses the boundary once.
#[tauri::command]
fn reveal_key() -> Option<String> {
    config::Config::load()
        .ok()
        .and_then(|config| config.api_key)
        .filter(|key| !key.trim().is_empty())
}

/// Writes the model settings, and starts using them.
///
/// The key arrives here and goes no further. It is written to the file, which
/// is where a secret belongs, and the view that comes back says only whether
/// there is one.
#[tauri::command]
async fn save_model(
    app: AppHandle,
    endpoint: String,
    model: String,
    key: String,
    reasoning: bool,
    enabled: bool,
) -> Result<View, String> {
    let state = app.state::<App>();
    state.save_model(&endpoint, &model, &key, reasoning, enabled)?;

    Ok(state.view())
}

/// How a fetch is going, for the window to draw.
///
/// Names the file as well as counting the bytes, because there are two of them
/// and the first is seventeen times the second: a bar with no name on it would
/// look stuck between them, and the name is also the answer to "what is it
/// downloading".
#[derive(Debug, Clone, Serialize)]
struct Progress {
    file: String,
    received: u64,
    total: u64,
}

/// Fetches the two models this app reads a page with.
///
/// Off the main thread, because a quarter of a gigabyte over a network is
/// minutes and the window has to stay drawn while it arrives. Progress goes out
/// as an event rather than as the return value, which comes back once and only
/// at the end — too late to be progress.
///
/// A failure comes back as a view with a problem on it rather than as an error,
/// because the window can draw that and an error it has no view for is a
/// sentence in a language it does not speak. Stopping is not a failure: what
/// arrived is kept, and the next attempt resumes.
#[tauri::command]
async fn fetch_local_models(app: AppHandle) -> Result<View, String> {
    let handle = app.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<App>();
        let flag = state.begin_fetch()?;
        let root = layout::default_root();
        let mut trouble = None;

        for spec in [&layout::STRUCTURE, &layout::TABLE] {
            let outcome = layout::ensure_of(
                spec,
                &root,
                &mut |progress| {
                    let (received, total) = match progress {
                        layout::State::Fetching { received, total, .. } => (*received, *total),
                        layout::State::Verifying => (spec.bytes, spec.bytes),
                        _ => return,
                    };

                    let _ = handle.emit(
                        "local-models",
                        Progress {
                            file: spec.file.to_owned(),
                            received,
                            total,
                        },
                    );
                },
                &flag,
            );

            match outcome {
                Ok(layout::Outcome::Ready(_)) => {}
                Ok(layout::Outcome::Cancelled) => break,
                Err(error) => {
                    trouble = Some(error.to_string());
                    break;
                }
            }
        }

        state.end_fetch();

        // The answer to "is it here now" was taken before there was anything to
        // find. A process that downloads the model is the process that has to
        // notice it arrived.
        structure::forget();
        table::forget();

        let mut view = state.view();
        if let Some(detail) = trouble {
            view.problem = Some(Trouble::new(Cause::Download, detail));
        }

        Ok(view)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Asks a fetch that is running to stop.
///
/// Says nothing back, because the fetch says it: the view it returns when it
/// stops already knows it was stopped, and the window draws that. Two answers
/// to the same question is two chances to disagree.
#[tauri::command]
fn cancel_local_models(app: AppHandle) {
    app.state::<App>().stop_fetch();
}

/// Opens something already in the knowledge base.
#[tauri::command]
async fn open(app: AppHandle, id: String, name: String) -> Result<View, String> {
    let handle = app.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<App>();
        state.open(&id, &name);
        state.view()
    })
    .await
    .map_err(|error| error.to_string())
}

/// Starts the window.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(App::new())
        .invoke_handler(tauri::generate_handler![
            view,
            records,
            choose_file,
            process,
            open,
            save_model,
            reveal_key,
            probe_models,
            fetch_local_models,
            cancel_local_models,
            test_model
        ])
        .run(tauri::generate_context!())
        .expect("the window could not start");
}

#[cfg(test)]
mod tests {
    use super::*;
    use docfusion_core::image::ImageError;
    use docfusion_core::ir::EngineId;
    use docfusion_store::manifest::StoreError;

    /// A model that is not here is all still to come, which is the sentence
    /// the settings tab puts in front of somebody about to spend the time.
    #[test]
    fn a_model_that_is_not_here_is_wanted_whole() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(
            missing_bytes(&layout::STRUCTURE, dir.path()),
            layout::STRUCTURE.bytes
        );
    }

    /// A download that was stopped is resumed rather than restarted, so the
    /// window would be wrong to promise the whole file again.
    #[test]
    fn a_partial_download_is_only_what_is_left() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(layout::path_of(&layout::TABLE, dir.path()), [0u8; 4096]).unwrap();

        assert_eq!(
            missing_bytes(&layout::TABLE, dir.path()),
            layout::TABLE.bytes - 4096
        );
    }

    /// The length check would otherwise read this as a finished download and
    /// say there is nothing to fetch. The fetch does not agree: it checks the
    /// digest, throws this away and starts again.
    #[test]
    fn a_wrong_file_of_the_right_length_is_still_all_to_come() {
        let dir = tempfile::tempdir().unwrap();
        let path = layout::path_of(&layout::TABLE, dir.path());

        std::fs::File::create(&path)
            .unwrap()
            .set_len(layout::TABLE.bytes)
            .unwrap();

        assert_eq!(missing_bytes(&layout::TABLE, dir.path()), layout::TABLE.bytes);
    }

    /// Whether the stylesheet gives `tag` a rule of its own under the pane.
    ///
    /// The name has to **end** where a selector name ends, and looking for the
    /// name alone does not: renaming a rule to `blockquote-removed` leaves
    /// `.document blockquote` in the text, and a check that only looks for that
    /// passes with the rule gone — which is how this was found to be wrong.
    fn styled(css: &str, tag: &str) -> bool {
        let prefix = format!(".document {tag}");
        let mut from = 0;

        while let Some(at) = css[from..].find(&prefix) {
            let end = from + at + prefix.len();
            from = end;

            let after = css[end..].chars().next();
            if matches!(after, Some(' ') | Some(',') | Some('[') | Some('{') | Some(':')) {
                return true;
            }
        }

        false
    }

    /// Every element a rendered document can hold has a rule in the stylesheet.
    ///
    /// The document pane shows whatever `render::html` produces and nothing
    /// else, so the set of elements that can appear in it is fixed — but a pane
    /// styled for the headings and paragraphs somebody happened to think of
    /// leaves the rest to the browser, and the browser's defaults are drawn for
    /// a page of prose: list markers outside the column, an indent four times
    /// this one, a quote with a margin and no mark, headings at twice the
    /// document's spacing.
    ///
    /// Enumerated rather than sampled. The document below uses every one of
    /// them, the tags are taken from what the renderer actually wrote, and each
    /// is looked up. The three that are excused are the ones whose browser
    /// default is the wanted one — a bold run, an italic run and a struck one
    /// are the same in any document.
    #[test]
    fn every_element_a_document_can_hold_is_styled() {
        let markdown = "# One

## Two

### Three

#### Four

##### Five

###### Six

A paragraph with **strong**, *emphasis*, ~~struck~~, `code`,
a [link](https://example.com) and ![an image](x.png).

- one
- two
  - nested

1. first
2. second

> quoted

---

```rust
code
```

| a | b |
| --- | --- |
| c | d |

- [ ] a task
";

        let rendered = docfusion_core::render::html(markdown);
        let css = include_str!("../ui/style.css");

        // Excused, and each for a reason rather than because it failed. The
        // first three are runs inside a line whose browser default is the
        // wanted one. The rest are wrappers: a table's row groups and rows hold
        // nothing of their own, and what is drawn is the cells inside them,
        // which have rules.
        let excused = ["strong", "em", "del", "br", "thead", "tbody", "tr"];
        let mut looked = 0;

        for (at, _) in rendered.match_indices('<') {
            let rest = &rendered[at + 1..];
            let tag: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();

            if tag.is_empty() || excused.contains(&tag.as_str()) {
                continue;
            }

            looked += 1;
            assert!(
                styled(css, &tag),
                "the document pane writes <{tag}> and the stylesheet has no rule for it"
            );
        }

        // The document has to have used them, or this passes by looking at
        // nothing.
        assert!(looked > 12, "only {looked} tags were checked");
    }

    #[test]
    fn the_engines_are_read_back_from_the_front_matter() {
        let markdown = "---\ntitle: Report\nengines: [pdfium, rapidocr]\n---\n# Report\n";

        assert_eq!(engines_of(markdown), vec!["pdfium", "rapidocr"]);
    }

    /// A document read by one engine is the common case, and the one a parser
    /// that assumed a list would get wrong.
    #[test]
    fn one_engine_is_a_list_too() {
        assert_eq!(engines_of("---\nengines: [pdfium]\n---\n"), vec!["pdfium"]);
    }

    #[test]
    fn a_document_without_front_matter_has_no_engines_rather_than_a_wrong_one() {
        assert!(engines_of("# Just a heading\n").is_empty());
    }

    /// The window chooses its sentence from the kind, so this mapping is what
    /// decides whether a failure comes out saying anything true. Every way the
    /// library has of failing is here, which is also what stops a new one from
    /// being added without anybody deciding how to say it.
    #[test]
    fn every_way_reading_can_fail_has_a_kind_the_window_can_say_it_in() {
        let engine = || EngineId::new("rapidocr");

        let cases = [
            (
                PipelineError::Extract(ExtractError::unsupported("nothing reads this")),
                Cause::Unsupported,
            ),
            (
                PipelineError::Extract(ExtractError::Failed {
                    engine: engine(),
                    message: "the endpoint refused the connection".to_owned(),
                }),
                Cause::EngineFailed,
            ),
            (
                PipelineError::Extract(ExtractError::Io(std::io::Error::other("gone"))),
                Cause::Unreadable,
            ),
            (
                PipelineError::Extract(ExtractError::Image(ImageError::Empty)),
                Cause::Unreadable,
            ),
            (
                PipelineError::Read {
                    path: "a.pdf".into(),
                    source: std::io::Error::other("gone"),
                },
                Cause::Unreadable,
            ),
            (
                PipelineError::Write {
                    path: "a.md".into(),
                    source: std::io::Error::other("full"),
                },
                Cause::Unwritable,
            ),
            (PipelineError::UnidentifiableEngine, Cause::NothingToRead),
            (
                PipelineError::Store(StoreError::UnknownStatus("wat".to_owned())),
                Cause::Index,
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(cause_of(&error), expected, "{error}");
        }
    }

    /// Half a pair is not a setting. One endpoint may serve several models, so
    /// taking the endpoint's word for it would silently read documents with
    /// whatever it happened to be serving.
    #[test]
    fn an_endpoint_without_a_model_is_no_endpoint() {
        let only_url = config::Config {
            endpoint: Some("https://api.deepseek.com".to_owned()),
            ..Default::default()
        };
        let only_model = config::Config {
            model: Some("deepseek-flash".to_owned()),
            ..Default::default()
        };

        assert!(endpoint_of(&only_url).is_none());
        assert!(endpoint_of(&only_model).is_none());
    }

    #[test]
    fn a_whole_pair_becomes_the_endpoint_it_describes() {
        let config = config::Config {
            endpoint: Some("https://api.deepseek.com".to_owned()),
            model: Some("deepseek-flash".to_owned()),
            api_key: Some("sk-example".to_owned()),
            ..Default::default()
        };

        let endpoint = endpoint_of(&config).expect("a pair is a setting");

        assert_eq!(endpoint.base_url, "https://api.deepseek.com");
        assert_eq!(endpoint.model, "deepseek-flash");
        assert_eq!(endpoint.api_key.as_deref(), Some("sk-example"));
    }

    /// The view does not carry the key. It carries how long the key is, which
    /// is what the field is masked from and says nothing about the key — and a
    /// key that is present but blank is not a key.
    ///
    /// This is the test standing between a convenient settings page and a
    /// secret in every command the window sends, so it checks the serialised
    /// form rather than the field.
    #[test]
    fn the_settings_never_carry_the_key_itself() {
        let with_key = config::Config {
            api_key: Some("sk-example".to_owned()),
            ..Default::default()
        };
        let blank_key = config::Config {
            api_key: Some("   ".to_owned()),
            ..Default::default()
        };

        assert_eq!(model_of(&with_key).key_length, "sk-example".len());
        assert_eq!(model_of(&blank_key).key_length, 0);
        assert_eq!(model_of(&config::Config::default()).key_length, 0);

        let shown = serde_json::to_string(&model_of(&with_key)).unwrap();
        assert!(!shown.contains("sk-example"), "{shown}");
    }

    /// Does not run by itself: it reaches a paid endpoint with whatever key is
    /// in the configuration on this machine.
    ///
    /// Run it by hand after touching anything about the request —
    /// `cargo test -p docfusion-gui -- --ignored` — because the only way to
    /// know whether a model can read a picture is to show it one. In
    /// particular it is the check on the drawn page: a glyph with a bit in the
    /// wrong place is a number the model reads differently, and nothing that
    /// does not look at the picture can tell.
    #[test]
    #[ignore = "reaches the network and spends money"]
    fn the_configured_endpoint_can_read_the_test_page() {
        match App::new().test() {
            Test::Read => {}
            other => panic!("{other:?}"),
        }
    }

    // ── The report on a document ─────────────────────────────────────────────

    /// The report is written by one crate and read back by another, so the two
    /// have to agree. This is the test that can see both halves: it renders a
    /// document that really does have a disagreement through the real renderer,
    /// and then reads it back the way the window does.
    #[test]
    fn the_report_the_renderer_writes_is_the_one_the_window_reads() {
        let markdown = render::markdown(&disagreeing_document(), "2026-10-09T00:00:00Z");

        let (document, report) = split_report(&markdown);

        // The text is the document and the report is not in it.
        assert!(document.contains("Body text"), "{document}");
        assert!(!document.contains("## Conflicts"), "{document}");

        let conflicts = conflicts_of(report);

        assert_eq!(conflicts.len(), 1, "{report}");
        assert_eq!(conflicts[0].page, 1);
        assert_eq!(conflicts[0].kind, "the engines read it differently");

        let readings = &conflicts[0].readings;
        assert_eq!(readings.len(), 2);
        assert_eq!(readings[0].engine, "deepseek");
        assert_eq!(readings[0].text, "Revenue increased by 13%");
        assert_eq!(readings[1].engine, "rapidocr");
        assert_eq!(readings[1].text, "Revenue increased by 12%");
    }

    /// The line that names an engine also says what it read the region as, and
    /// that is the only place the shape of a reading is written down.
    #[test]
    fn a_reading_line_gives_the_engine_and_what_it_read() {
        let (engine, shapes) =
            engine_and_shapes("rapidocr — headings 0, lists 0, tables 0, text 12");

        assert_eq!(engine, "rapidocr");
        assert_eq!(
            shapes,
            Some(Shapes {
                headings: 0,
                lists: 0,
                tables: 0,
                text: 12,
            })
        );
    }

    /// An engine name is not lost to a line this cannot read. The name is what
    /// everything else calls it; the counts are one reading's.
    #[test]
    fn an_engine_named_on_a_line_this_cannot_read_is_still_named() {
        // No dash at all: an older document, or another program's report.
        assert_eq!(engine_and_shapes("rapidocr"), ("rapidocr".to_owned(), None));

        // A dash, and something after it that is not all four counts. Half a
        // shape is worse than none: the zeroes nobody wrote would be a claim
        // about the reading rather than a reading of the line.
        let (engine, shapes) = engine_and_shapes("rapidocr — headings 0, lists 0");
        assert_eq!(engine, "rapidocr");
        assert_eq!(shapes, None);
    }

    #[test]
    fn a_document_with_nothing_to_report_has_no_report_in_it() {
        let markdown = "---\ntitle: Notes\n---\n\nJust the text.\n";

        let (document, report) = split_report(markdown);

        assert_eq!(document, markdown);
        assert!(conflicts_of(report).is_empty());
    }

    /// Every line of a reading is indented, including the empty ones, so an
    /// empty line is one of the reading's lines rather than the end of it.
    #[test]
    fn a_blank_line_inside_a_reading_is_kept() {
        let report = "\nPage 1: the engines read it differently.\n\n    vision\n    first\n    \n    second\n\n";

        let conflicts = conflicts_of(report);

        assert_eq!(conflicts[0].readings[0].text, "first\n\nsecond");
    }

    /// The indentation is what protects this: a reading of a page that mentions
    /// another page must not be read as the start of another finding.
    #[test]
    fn a_line_that_looks_like_a_finding_inside_a_reading_is_not_one() {
        let report =
            "\nPage 2: the engines read it differently.\n\n    vision\n    Page 3: nothing here.\n\n";

        let conflicts = conflicts_of(report);

        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].page, 2);
        assert_eq!(conflicts[0].readings[0].text, "Page 3: nothing here.");
    }

    /// A report this does not understand yields nothing rather than something
    /// wrong. The document is on screen either way; what is missing is the
    /// separate view of the doubt, not any of the text.
    #[test]
    fn a_report_in_another_shape_is_no_findings_rather_than_a_wrong_one() {
        assert!(conflicts_of("Something else entirely.\n").is_empty());
        assert!(conflicts_of("Page one: not a number.\n").is_empty());
    }

    fn disagreeing_document() -> docfusion_core::ir::Document {
        use docfusion_core::ir::{
            Block, BlockKind, Conflict, ConflictKind, Document, EngineId, Page, PageProvenance,
            SourceRef, Variant,
        };

        let variant = |engine: &str, text: &str| Variant {
            engine: EngineId::from(engine),
            text: text.to_owned(),
            // One paragraph each, which is what these two readings are.
            shapes: Shapes::of(&[Block::text(
                BlockKind::Paragraph,
                text,
                EngineId::from(engine),
            )]),
        };

        Document {
            source: SourceRef {
                path: "report.pdf".into(),
                content_hash: "a".repeat(64),
                mime: "application/pdf".into(),
                size: 10,
                mtime: None,
            },
            pages: vec![Page {
                index: 0,
                size: None,
                blocks: vec![Block::text(
                    BlockKind::Paragraph,
                    "Body text",
                    EngineId::from("deepseek"),
                )],
                provenance: PageProvenance {
                    primary: EngineId::from("deepseek"),
                    engines: vec![EngineId::from("deepseek"), EngineId::from("rapidocr")],
                    verified: true,
                },
            }],
            meta: docfusion_core::ir::DocMeta::default(),
            conflicts: vec![Conflict {
                page: 0,
                bbox: None,
                kind: ConflictKind::TextMismatch,
                variants: vec![
                    variant("deepseek", "Revenue increased by 13%"),
                    variant("rapidocr", "Revenue increased by 12%"),
                ],
                resolved: None,
            }],
        }
    }

    /// The account is passed through rather than paraphrased, so whatever the
    /// library said has to survive the trip to the window intact.
    #[test]
    fn the_account_is_the_library_s_own_words() {
        let error = PipelineError::Extract(ExtractError::Failed {
            engine: EngineId::new("rapidocr"),
            message: "the endpoint refused the connection".to_owned(),
        });

        let trouble = Trouble::new(cause_of(&error), error.to_string());

        assert!(trouble.detail.contains("the endpoint refused the connection"));
        assert!(trouble.detail.contains("rapidocr"));
    }
}
