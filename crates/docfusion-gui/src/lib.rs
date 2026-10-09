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
use std::sync::Mutex;

use docfusion_core::extract::{ExtractError, Router};
use docfusion_core::render;
use docfusion_core::verify::{VerifyMode, VerifyPolicy};
use docfusion_store::config;
use docfusion_store::engines;
use docfusion_store::memory;
use docfusion_store::pipeline::{Mode, Pipeline, PipelineError};

use tauri::{AppHandle, Manager};

use state::{Cause, Opened, Record, Trouble, View};

/// The application's shared state.
///
/// One knowledge base and one view, because that is what the window shows. A
/// job at a time, because a person reads one document at a time and the
/// alternative is two jobs competing for the same pipelines.
pub struct App {
    root: PathBuf,
    endpoint: Option<docfusion_store::endpoint::EndpointConfig>,
    policy: VerifyPolicy,
    readers: usize,
    view: Mutex<View>,
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

        let endpoint = match (&config.endpoint, &config.model) {
            (Some(url), Some(model)) => {
                let mut endpoint = docfusion_store::endpoint::EndpointConfig::new(url, model);
                if let Some(key) = config.api_key.clone() {
                    endpoint = endpoint.with_api_key(key);
                }
                Some(endpoint)
            }
            // An endpoint without a model is a half-written setting. The
            // command line refuses it; the window cannot be shown a refusal
            // before it has drawn anything, so it reads documents with the
            // bundled engine and `doctor` says what is missing.
            _ => None,
        };

        let mut view = View::new(
            root.display().to_string(),
            config::path().display().to_string(),
        );
        view.problem = problem;

        Self {
            root,
            endpoint,
            policy,
            readers,
            view: Mutex::new(view),
        }
    }

    fn view(&self) -> View {
        self.view
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
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
        let router = Router::new(engines::standard(
            self.endpoint.clone(),
            self.policy.clone(),
            self.readers,
        ));

        Pipeline::open(&self.root, router)
            .map_err(|error| Trouble::new(cause_of(&error), error.to_string()))
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

        Ok(Opened {
            id: id.to_owned(),
            name: output
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default(),
            output: output.display().to_string(),
            html: render::html(&markdown),
            engines,
            conflicts,
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
        .invoke_handler(tauri::generate_handler![view, records, choose_file, process, open])
        .run(tauri::generate_context!())
        .expect("the window could not start");
}

#[cfg(test)]
mod tests {
    use super::*;
    use docfusion_core::image::ImageError;
    use docfusion_core::ir::EngineId;
    use docfusion_store::manifest::StoreError;

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
