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
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use docfusion_core::backend::{PageRequest, VisionBackend};
use docfusion_core::extract::PageSource;
use docfusion_core::extract::pdf::RenderedPages;
use docfusion_core::image::PageImage;
use docfusion_core::ir::Shapes;
use docfusion_core::extract::{ExtractError, Router};
use docfusion_core::export::{self, Format};
use docfusion_core::render;
use docfusion_core::verify::{VerifyMode, VerifyPolicy};
use docfusion_store::config;
use docfusion_store::engines;
use docfusion_store::models;
use docfusion_store::memory;
use docfusion_store::pipeline::{Mode, Pipeline, PipelineError};
use docfusion_store::{ocr, structure, table};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use state::{
    Cause, Conflict, Local, Match, Model, Opened, Reading, Record, Remote, Screen, Source, Test,
    Trouble, View,
};

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
    /// The source a pane is looking at, while it is looking at it.
    ///
    /// Held because the pages are rendered one at a time and a PDF is reopened
    /// for each of them: cheap when a page costs seconds to recognise, and not
    /// when they are being scrolled through. Without this, a two hundred page
    /// scan would be read and reopened two hundred times.
    ///
    /// One document, because there is one source pane.
    watching: Mutex<Option<Watched>>,
    /// The fetch that is running, if one is.
    ///
    /// What is held is the flag the store takes to interrupt itself, and
    /// nothing in this window ever sets it — there is no Stop button. It is
    /// here because the slot is also what says a fetch is in flight, which is
    /// both what the card draws from and what refuses a second one.
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
            watching: Mutex::new(None),
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

    /// How much of the set this app reads a page with is on disk.
    ///
    /// The three answers come from the same remembered checks the engine makes
    /// before it decides what to call itself, rather than from a fresh digest
    /// of 138 MB on every view — which is also why fetching has to forget them,
    /// since otherwise the window would offer to download something it had just
    /// been given.
    fn local(&self) -> Local {
        let absent = models::absent_from(
            ocr::available(),
            structure::available(),
            table::available(),
        );

        Local {
            ready: absent.is_empty(),
            fetching: self
                .fetching
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some(),
            needed_bytes: models::still_to_fetch(&absent, &models::default_root()),
            total_bytes: models::set_bytes(),
        }
    }

    /// Puts a fetch in flight, and hands back the flag it is watched by.
    ///
    /// Refused when one is already running, because two fetches writing the
    /// same files would each see the other's partial download and resume from a
    /// length that is not theirs.
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

    /// Writes somebody's own words into a document, and shows the result.
    ///
    /// The pane is re-rendered from what is now on disk rather than from the
    /// text that was sent, so that what is on screen is the file — the same
    /// rule [`App::open_at`] follows, and the reason the two go through the
    /// same function.
    fn save_source(&self, id: &str, body: &str) -> Result<Opened, Trouble> {
        let pipeline = self.pipeline()?;

        let record = pipeline
            .save_body(id, body, &docfusion_store::manifest::now_iso8601())
            .map_err(|error| Trouble::new(cause_of(&error), error.to_string()))?;

        let output = pipeline.root().join(&record.output_path);
        self.open_at(&record.id, &output, false)
    }

    /// What the editor is handed: a document's words, and neither of the two
    /// things this program wrapped around them.
    ///
    /// The front matter is where the document says where it came from and the
    /// report is what the engines disagreed about. Both are about the document
    /// rather than in it, and an editor that could reach them could damage the
    /// record of what happened to it.
    fn source_of(&self, id: &str) -> Result<String, Trouble> {
        let pipeline = self.pipeline()?;

        let record = pipeline
            .resolve(id)
            .map_err(|error| Trouble::new(cause_of(&error), error.to_string()))?;

        let path = pipeline.root().join(&record.output_path);
        let markdown = std::fs::read_to_string(&path).map_err(|error| {
            Trouble::new(Cause::Unreadable, format!("{}: {error}", path.display()))
        })?;

        Ok(render::parts(&markdown).body.to_owned())
    }

    /// Whether reading this file again would replace somebody's own edits.
    ///
    /// `None` when the file is not in the knowledge base, or is in it unedited.
    /// The two are the same answer to the question being asked, which is
    /// whether there is anything to warn about, and the answer is no either
    /// way.
    ///
    /// What is asked before a **re-read**, and not before every add: adding a
    /// file that is already in the knowledge base without having changed is a
    /// cache hit, and a cache hit does not write the document at all. The edit
    /// survives that, so there is nothing to warn about. It does not survive
    /// the engines changing or `reindex`, which is what this is for.
    fn edited_record(&self, path: &str) -> Option<String> {
        let pipeline = self.pipeline().ok()?;
        let record = pipeline.resolve(path).ok()?;

        record.edited_at.as_ref()?;

        // Named the way everything else here names a document — by the file it
        // was read from, not by the path it was written to.
        Some(
            record
                .source_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| record.id.clone()),
        )
    }

    /// The source copy of a document, and what kind of file it is.
    ///
    /// Read once and kept while a pane is looking at it; see [`App::watching`].
    fn copy_of(&self, id: &str) -> Result<(Arc<Vec<u8>>, String), Trouble> {
        {
            let watched = self
                .watching
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());

            if let Some(watched) = watched.as_ref()
                && watched.id == id
            {
                return Ok((watched.bytes.clone(), watched.mime.clone()));
            }
        }

        let pipeline = self.pipeline()?;

        let record = pipeline
            .resolve(id)
            .map_err(|error| Trouble::new(cause_of(&error), error.to_string()))?;

        // Resolved rather than read out of the record: the copy is kept if it is
        // here and found again by content if it is not, and a record whose copy
        // somebody deleted is a record that can still be looked at.
        let copy = pipeline
            .keep_source_of(&record)
            .map_err(|error| Trouble::new(cause_of(&error), error.to_string()))?;

        let bytes = std::fs::read(&copy).map_err(|error| {
            Trouble::new(Cause::Unreadable, format!("{}: {error}", copy.display()))
        })?;

        let bytes = Arc::new(bytes);
        *self
            .watching
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Watched {
            id: id.to_owned(),
            mime: record.mime.clone(),
            bytes: bytes.clone(),
        });

        Ok((bytes, record.mime))
    }

    /// Lets go of the source a pane was looking at.
    fn stop_watching(&self) {
        *self
            .watching
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    /// How many pages of a document's source there are to look at.
    fn source_pages(&self, id: &str) -> Result<Source, Trouble> {
        let (bytes, mime) = self.copy_of(id)?;

        Ok(match page_count(&bytes, &mime)? {
            Some(count) => Source::Pages { count },
            None => Source::Nothing,
        })
    }

    /// One of them, as a picture.
    ///
    /// Two very different amounts of work behind one answer. **An image is
    /// already a picture of itself**, so its own bytes go: decoding it and
    /// encoding it again is a great deal of work to hand back a worse and larger
    /// copy of what arrived. A PDF page has to be rendered, and it is rendered
    /// as a JPEG — a page beside a column of text is a picture rather than a
    /// document, and the same page as a PNG is several times the bytes over the
    /// bridge and several times the encoding.
    fn source_page(&self, id: &str, page: usize) -> Result<String, Trouble> {
        let (bytes, mime) = self.copy_of(id)?;

        if mime.starts_with("image/") {
            if page > 0 {
                return Err(Trouble::new(
                    Cause::Unreadable,
                    format!("an image is one page, and there is no page {}", page + 1),
                ));
            }

            return Ok(format!("data:{mime};base64,{}", BASE64.encode(bytes.as_ref())));
        }

        let rendered = rendered_page(&bytes, page)?;

        let jpeg = rendered
            .to_jpeg(PAGE_QUALITY)
            .map_err(|error| Trouble::new(Cause::Unreadable, error.to_string()))?;

        Ok(format!("data:image/jpeg;base64,{}", BASE64.encode(&jpeg)))
    }

    fn forget_record(&self, id: &str) -> Result<(), Trouble> {
        let outcome = forget(&self.pipeline()?, id);

        if outcome.is_ok() {
            blank_if_showing(&self.view, id);
        }

        outcome
    }

    fn delete_record(&self, id: &str) -> Result<(), Trouble> {
        let outcome = delete(&self.pipeline()?, id);

        if outcome.is_ok() {
            blank_if_showing(&self.view, id);
        }

        outcome
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
            html: window_html(document),
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
            let root = pipeline.root();
            view.records = records
                .iter()
                .map(|record| Record::from_stored(record, root))
                .collect();
        }
    }
}

/// The document as the window draws it: one element per top-level block, each
/// saying which block it is.
///
/// From the same list an export numbers by, and the number is written onto the
/// element rather than counted by the window — the alternative is the window
/// assuming that the nth element it finds is the nth block, which is true today
/// because `render::html` is the blocks joined, and would stop being true
/// without anything saying so.
///
/// The wrapper adds no spacing of its own, so the blocks' margins collapse
/// through it and the page lays out as it did before it was there.
fn window_html(markdown: &str) -> String {
    let mut out = String::new();

    for (at, block) in render::blocks(markdown).into_iter().enumerate() {
        // A table is marked as one here rather than recognised as a `<table>`
        // by the window, so that the formats the window offers for a block and
        // the formats the other half will write are one fact read twice, rather
        // than two facts that happen to agree.
        let kind = if block.table { " block-table" } else { "" };

        out.push_str(&format!(
            "<section class=\"block{kind}\" data-block=\"{at}\">\
             <div class=\"block-body\">{}</div></section>",
            block.html
        ));
    }

    out
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
/// so it reads with the local engine and says what is missing.
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

        // Both mean the query did not name exactly one document, which from
        // here is one sentence. The window always has a full id — it is
        // reading it out of a row — so the ambiguous half is for the command
        // line, where somebody is typing a prefix.
        PipelineError::NoSuchDocument { .. } | PipelineError::Ambiguous { .. } => Cause::Missing,

        // And the two that are about a delete: the file would not go, or the
        // record pointed somewhere this program will not reach.
        PipelineError::Delete { .. } | PipelineError::Outside { .. } => Cause::Delete,

        // The bytes a document was read from are not where the index says.
        // Nothing the window does reaches this — it is `reconcile` putting
        // copies back — but the sentence is about a file that is not there.
        PipelineError::SourceGone { .. } => Cause::Missing,
    }
}

fn extract_cause(error: &ExtractError) -> Cause {
    match error {
        // The engine that failed is named in the account. Which one it was is
        // not something the person reading this can act on differently — a
        // page that would not read is a page that would not read.
        ExtractError::Failed { .. } => Cause::EngineFailed,
        // An engine that never ran because its weights are not here. The same
        // cause the fetch itself reports, because it is the same thing from the
        // reader's side — a download they have not done — and the sentence for
        // it names the settings tab.
        ExtractError::Unavailable { .. } => Cause::Download,
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

/// Finds documents by what is in them.
///
/// Returns the matches rather than a [`View`], because a search is a question
/// and the answer to it is not part of what is on screen: the view is the
/// archive's state and this is a reply, and a view that carried a reply would
/// have to be given another one every time the window redrew.
///
/// Not on a thread of its own, unlike reading a document: the index does the
/// finding and what is read afterwards is twenty passages out of twenty files
/// the operating system wrote and still has in hand.
#[tauri::command]
fn search(app: AppHandle, query: String) -> Result<Vec<Match>, String> {
    let state = app.state::<App>();

    let pipeline = state.pipeline().map_err(|trouble| trouble.detail)?;
    let found = pipeline
        .search(&query, SEARCH_LIMIT)
        .map_err(|error| error.to_string())?;

    Ok(found
        .into_iter()
        .map(|hit| Match {
            id: hit.id,
            name: hit.name,
            excerpt: hit.excerpt,
        })
        .collect())
}

/// How many documents a search answers with.
///
/// A term that appears in everything would otherwise return the archive, and a
/// list that long is not an answer to anything. The same number the command
/// line defaults to, so the two tools answer the same question the same way.
const SEARCH_LIMIT: usize = 20;

/// Takes a record out of the index, leaving its Markdown alone.
///
/// The work is [`Pipeline::forget`], which is where it lives so that the
/// command line can do the same thing — this program's own rule being that the
/// window must not be the only place a capability lives. What is left here is
/// the half the window owns: turning a library error into the kind of trouble
/// the failure panel has a sentence for.
fn forget(pipeline: &Pipeline, id: &str) -> Result<(), Trouble> {
    pipeline
        .forget(id)
        .map(|_| ())
        .map_err(|error| Trouble::new(cause_of(&error), error.to_string()))
}

/// Deletes a record and the Markdown it points at. See [`Pipeline::delete`].
fn delete(pipeline: &Pipeline, id: &str) -> Result<(), Trouble> {
    pipeline
        .delete(id)
        .map(|_| ())
        .map_err(|error| Trouble::new(cause_of(&error), error.to_string()))
}

/// Blanks the document pane when the record it is showing is no longer there.
///
/// The window would otherwise keep displaying a document that has just been
/// taken out of the knowledge base — and in one of the two cases the file it was
/// rendered from is gone, so what is on screen is a reading of something that no
/// longer exists with nothing to say so.
///
/// Not the same as deciding to go back to the start: somebody reading a document
/// and then tidying the list behind it has not asked for that, but there is
/// nothing left to show either. `Screen::Empty` is the answer the window already
/// has for "nothing is open", and it says so. It also carries the knowledge base
/// path, which is the one useful thing to put in front of somebody who has just
/// emptied the pane.
///
/// Over the lock rather than a method on [`App`], like the two acts above,
/// because the thing worth a test is what it does to a screen — and an `App`
/// cannot be pointed at a temporary directory.
fn blank_if_showing(view: &Mutex<View>, id: &str) {
    let mut view = view.lock().unwrap_or_else(|p| p.into_inner());

    if matches!(&view.screen, Screen::Ready(opened) if opened.id == id) {
        view.screen = Screen::Empty;
    }
}

/// The view with the record list read out of the index again.
///
/// The list is drawn from a file rather than remembered, so it is re-read
/// rather than remembered — and every command that changes it has to say so,
/// including the two that take a record away. Everything that writes the index
/// is this process.
fn with_fresh_records(state: &App) -> View {
    let mut view = state.view();
    let mut fresh = state.view();
    state.refresh(&mut fresh);
    view.records = fresh.records;

    view
}

#[tauri::command]
fn records(app: AppHandle) -> View {
    with_fresh_records(&app.state::<App>())
}

/// Takes a record out of the index, leaving its Markdown where it is.
///
/// The reversible half of taking a document out of the knowledge base, and a
/// command of its own because it is a different act from the irreversible one:
/// this one leaves the file, so nothing is lost and nothing has to be
/// confirmed. The document becomes untracked — `reconcile` reports it and does
/// not put it back — and re-adding the source reads it again.
///
/// Returns a view with the list in it, because the list is what changed.
/// The view a removal leaves behind.
///
/// Takes the removal as an argument rather than being called after it, because
/// the order is the whole of this and is not obvious. Removing a record also
/// blanks the document pane when the record being shown is the one being
/// removed, and a view built before that carried the screen from before it —
/// which is what this did at first, and nothing could see it, because the
/// commands need an `AppHandle` and an `App` cannot be pointed at a temp
/// directory. As an argument, the removal cannot happen after the view is
/// built: the callee is the one that builds it.
fn after_removal(state: &App, outcome: Result<(), Trouble>) -> View {
    let mut view = with_fresh_records(state);

    if let Err(trouble) = outcome {
        view.problem = Some(trouble);
    }

    view
}

#[tauri::command]
fn forget_record(app: AppHandle, id: String) -> View {
    let state = app.state::<App>();
    after_removal(&state, state.forget_record(&id))
}

/// Deletes a record and the Markdown it points at.
///
/// The irreversible half, and the one command in this program that destroys
/// something. Separate from [`forget_record`] for exactly that reason, and the
/// window asks which of the two was meant before either is called: one button
/// that guessed between them would be a button that sometimes deletes a file.
///
/// The source document — the PDF or the picture somebody dragged in — is not
/// touched by either. It was never copied into the knowledge base, so there is
/// nothing there to delete; what goes is what this program wrote.
///
/// A failure comes back as the view with the reason on it, like everything
/// else here, so that a file that would not delete leaves the row in place and
/// says why rather than leaving the list and the disk disagreeing.
#[tauri::command]
fn delete_record(app: AppHandle, id: String) -> View {
    let state = app.state::<App>();
    after_removal(&state, state.delete_record(&id))
}

/// What the editor is handed to edit: a document's own words.
///
/// A command of its own rather than a field of the view. The view is serialised
/// for every command the window sends, and doubling it so that a panel most
/// documents are never opened in can have the text is a cost every document
/// read would pay.
#[tauri::command]
fn source(app: AppHandle, id: String) -> Result<String, Trouble> {
    app.state::<App>().source_of(&id)
}

/// Renders words that are not on disk yet.
///
/// The pane's own renderer, so what the editor shows beside the text is what
/// the document will look like — and numbered the same way, so a block's menu
/// keeps meaning what it meant while somebody types.
#[tauri::command]
fn preview(body: String) -> String {
    window_html(&body)
}

/// Writes somebody's own words into a document.
///
/// A failure is an `Err` and not a view with a problem on it, so that the window
/// can tell the two apart: a save that did not happen leaves what was typed
/// unsaved, and the window says so. A screen replaced by a report of what went
/// wrong would be that text gone with it — which is why this does not draw the
/// failure panel either.
#[tauri::command]
async fn save_source(app: AppHandle, id: String, body: String) -> Result<View, Trouble> {
    let handle = app.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<App>();
        let opened = state.save_source(&id, &body)?;

        let mut view = state.view();
        view.ready(opened);
        state.refresh(&mut view);

        Ok(view)
    })
    .await
    // The task itself did not finish, which is the save not happening rather
    // than a document that could not be read.
    .map_err(|error| Trouble::new(Cause::Unwritable, error.to_string()))?
}

/// Whether reading this file again would replace somebody's own edits.
///
/// `None` for a file that is not in the knowledge base, which is the common
/// answer and the same one as for a file nobody has edited: there is nothing to
/// warn about either way.
#[tauri::command]
fn edited(app: AppHandle, path: String) -> Option<String> {
    app.state::<App>().edited_record(&path)
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

/// Writes a document, or one block of it, out as a file.
///
/// `block` is the number the pane put on the block, or `None` for the whole
/// document, and `format` is one of [`Format`]'s names. Which of them suits
/// what is [`export::write`]'s rule and not the window's: the list the window
/// offers is built from the same facts, so a table cannot be a spreadsheet in
/// one half of the program and not in the other.
///
/// `Ok(None)` when the save dialog was dismissed, which is not a failure and is
/// not said as one.
///
/// A failure comes back as a [`Trouble`] rather than as a string, because the
/// window already has sentences for its kinds and a place to put them — see
/// `drawProblem`.
#[tauri::command]
async fn export(
    app: AppHandle,
    id: String,
    title: String,
    format: String,
    block: Option<usize>,
) -> Result<Option<String>, Trouble> {
    use tauri_plugin_dialog::DialogExt;

    let Some(chosen) = Format::parse(&format) else {
        return Err(Trouble::new(
            Cause::Unsupported,
            format!("`{format}` is not a format this program writes"),
        ));
    };

    let state = app.state::<App>();
    let pipeline = state.pipeline()?;

    let record = pipeline
        .resolve(&id)
        .map_err(|error| Trouble::new(cause_of(&error), error.to_string()))?;

    let source = pipeline.root().join(&record.output_path);
    let markdown = std::fs::read_to_string(&source).map_err(|error| {
        Trouble::new(Cause::Unreadable, format!("{}: {error}", source.display()))
    })?;

    // The file carries the report this program appends to the document, and the
    // pane draws the document without it. A block is numbered by what is on
    // screen, so a block is taken from the same part of the file the pane drew;
    // only the whole document is the whole file.
    let (document, _) = split_report(&markdown);
    let drawn = if block.is_some() { document } else { &markdown };

    if let Some(index) = block
        && index >= render::blocks(drawn).len()
    {
        return Err(Trouble::new(
            Cause::Missing,
            format!("the document no longer has a block {index}"),
        ));
    }

    let Some(written) = export::write(drawn, chosen, block) else {
        // Not reachable from the window, which offers only the formats that
        // suit what is being exported. It is here because the command is
        // reachable from anything that can talk to it.
        return Err(Trouble::new(
            Cause::Unsupported,
            format!("this document cannot be written as {chosen}"),
        ));
    };

    let name = suggested_name(&source, chosen, block);

    let Some(where_to) = app
        .dialog()
        .file()
        .set_title(title)
        .set_file_name(name)
        .add_filter(chosen.as_str().to_uppercase(), &[chosen.extension()])
        .blocking_save_file()
    else {
        return Ok(None);
    };

    let where_to = where_to
        .into_path()
        .map_err(|error| Trouble::new(Cause::Unwritable, error.to_string()))?;

    // The one spreadsheet that gets a byte-order mark and everything else that
    // does not. The reason is in `export::wants_a_mark`, and it is the same call
    // the command line makes — which matters more here than there, since a mark
    // in front of an Office package is a file that will not open at all.
    let mut bytes = Vec::with_capacity(written.len() + 3);
    if export::wants_a_mark(chosen) {
        bytes.extend_from_slice("\u{feff}".as_bytes());
    }
    bytes.extend_from_slice(&written);

    std::fs::write(&where_to, &bytes).map_err(|error| {
        Trouble::new(Cause::Unwritable, format!("{}: {error}", where_to.display()))
    })?;

    Ok(Some(where_to.display().to_string()))
}

/// What to call the file the save dialog opens with.
///
/// The document's own name, because that is what the person calls it, and the
/// block's number when it is one block: two exports of the same document
/// proposed under the same name is a question the save dialog answers with
/// " (2)".
fn suggested_name(source: &Path, format: Format, block: Option<usize>) -> String {
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_owned());

    let extension = format.extension();

    match block {
        Some(index) => format!("{stem}-{index}.{extension}"),
        None => format!("{stem}.{extension}"),
    }
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
/// The bytes of the whole set, not of the file being fetched. Six files each
/// running from zero to a hundred would look like six downloads and would make
/// the bar fall back five times; the question the card answers is how much of
/// the set is here, and that is a number that only goes up.
#[derive(Debug, Clone, Serialize)]
struct Progress {
    received: u64,
    total: u64,
}

/// Fetches the set of models this app reads a page with.
///
/// Off the main thread, because a quarter of a gigabyte over a network is
/// minutes and the window has to stay drawn while it arrives. Progress goes out
/// as an event rather than as the return value, which comes back once and only
/// at the end — too late to be progress.
///
/// A failure comes back as a view with a problem on it rather than as an error,
/// because the window can draw that and an error it has no view for is a
/// sentence in a language it does not speak.
///
/// There is no way to stop one from here. A download that is interrupted — the
/// window closed, the machine asleep — leaves what arrived, and the next
/// attempt resumes from it, which is the thing a Stop button would have been
/// for and is already true without one.
#[tauri::command]
async fn fetch_local_models(app: AppHandle) -> Result<View, String> {
    let handle = app.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<App>();
        let flag = state.begin_fetch()?;
        let root = models::default_root();
        let total = models::set_bytes();
        let mut trouble = None;

        // What is already here counts towards the bar from the first frame, so
        // that a fetch that has nothing to do does not sit at zero while it
        // checks six digests.
        let mut landed: u64 = 0;

        for spec in models::SET {
            if models::present_spec(spec, &root) {
                landed += spec.bytes;
                continue;
            }

            let base = landed;
            let outcome = models::ensure_of(
                spec,
                &root,
                &mut |progress| {
                    let received = match progress {
                        models::State::Fetching { received, .. } => *received,
                        models::State::Verifying => spec.bytes,
                        _ => return,
                    };

                    let _ = handle.emit(
                        "local-models",
                        Progress {
                            received: base.saturating_add(received).min(total),
                            total,
                        },
                    );
                },
                &flag,
            );

            match outcome {
                Ok(models::Outcome::Ready(_)) => landed += spec.bytes,
                Ok(models::Outcome::Cancelled) => break,
                Err(error) => {
                    trouble = Some(error.to_string());
                    break;
                }
            }
        }

        state.end_fetch();

        // The answer to "is it here now" was taken before there was anything to
        // find. A process that downloads the models is the process that has to
        // notice they arrived.
        docfusion_store::forget_models();

        let mut view = state.view();
        if let Some(detail) = trouble {
            view.problem = Some(Trouble::new(Cause::Download, detail));
        }

        Ok(view)
    })
    .await
    .map_err(|error| error.to_string())?
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

/// The source a pane is looking at, so that one page does not read the whole
/// file again.
struct Watched {
    id: String,
    mime: String,
    bytes: Arc<Vec<u8>>,
}

/// The resolution a source page is rendered at for looking at.
///
/// Lower than the 200 the recognition path renders at, and deliberately: this is
/// a page beside a column of text rather than a page being read character by
/// character, and four times the pixels is four times the bytes over the bridge
/// for a picture nobody is zooming into.
const PAGE_DPI: f32 = 110.0;

/// What a rendered page is encoded at.
///
/// High, because the page is the evidence: this is the thing somebody looks at
/// to decide whether a character was read correctly, and an artefact that lands
/// on a stroke is a letter that cannot be checked. It is still several times
/// smaller than the same page as a PNG, which is what the first version sent.
const PAGE_QUALITY: u8 = 85;

/// How many pages a file has that are worth looking at, or `None` for a file
/// that has none.
///
/// `None` rather than zero, because they are different answers: no pages is a
/// thing to explain to somebody, and none of anything is not.
fn page_count(bytes: &[u8], mime: &str) -> Result<Option<usize>, Trouble> {
    if mime == "application/pdf" {
        let pages = RenderedPages::open(bytes, PAGE_DPI)
            .map_err(|error| Trouble::new(Cause::Unreadable, error.to_string()))?;

        return Ok(Some(pages.len()));
    }

    // An image is one page and is already one. Everything else this program
    // reads has no picture of itself: an Office document is a zip of XML, and a
    // text file's page is its words, which are on the other side of the pane.
    Ok(mime.starts_with("image/").then_some(1))
}

/// One page of a PDF, rendered.
///
/// The document is reopened for every page, because that is what
/// [`RenderedPages`] does and holding one open would mean holding PDFium's lock
/// across calls. It costs milliseconds against a page that costs a second to
/// recognise; it stops being free when the pages are being scrolled through,
/// which is why the bytes behind it are read once and kept.
fn rendered_page(bytes: &[u8], page: usize) -> Result<Arc<PageImage>, Trouble> {
    let pages = RenderedPages::open(bytes, PAGE_DPI)
        .map_err(|error| Trouble::new(Cause::Unreadable, error.to_string()))?;

    pages
        .page(page)
        .map_err(|error| Trouble::new(Cause::Unreadable, error.to_string()))
}

/// What a document's source is, for looking at.
#[tauri::command]
fn source_pages(app: AppHandle, id: String) -> Result<Source, Trouble> {
    app.state::<App>().source_pages(&id)
}

/// One page of it, as a picture in a data URI.
///
/// A data URI rather than a file or a blob URL: the content policy allows
/// `img-src data:` and nothing else, and the copy is a file inside the knowledge
/// base rather than one of this window's own assets.
#[tauri::command]
fn source_page(app: AppHandle, id: String, page: usize) -> Result<String, Trouble> {
    app.state::<App>().source_page(&id, page)
}

/// Lets go of the source a pane was looking at.
///
/// A command of its own because what is held is the whole file: a scan that was
/// open a minute ago is not a thing to keep in memory for the rest of the
/// session.
#[tauri::command]
fn close_source(app: AppHandle) {
    app.state::<App>().stop_watching();
}

/// Starts the window.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(App::new())
        .invoke_handler(tauri::generate_handler![
            view,
            records,
            search,
            forget_record,
            delete_record,
            choose_file,
            source,
            preview,
            source_pages,
            source_page,
            close_source,
            save_source,
            edited,
            export,
            process,
            open,
            save_model,
            reveal_key,
            probe_models,
            fetch_local_models,
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

    /// Two exports of the same document are not proposed under the same name.
    #[test]
    fn an_export_is_named_after_the_document_it_came_out_of() {
        let source = Path::new("/library/docs/notes.md");

        assert_eq!(suggested_name(source, Format::Html, None), "notes.html");
        assert_eq!(suggested_name(source, Format::Markdown, Some(3)), "notes-3.md");
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

    /// A knowledge base with one document in it: its directory, the pipeline
    /// over it, the id it was filed under, and where the Markdown went.
    ///
    /// A text file, so that nothing here needs a model to be downloaded — what
    /// these tests are about is a row and a file, not a reading.
    fn one_document() -> (tempfile::TempDir, Pipeline, String, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let mut pipeline = Pipeline::open(dir.path(), reading_text()).unwrap();

        let source = dir.path().join("notes.txt");
        std::fs::write(&source, "the words").unwrap();

        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();
        let output = wrote.output.clone();

        (dir, pipeline, wrote.id, output)
    }

    fn reading_text() -> Router {
        Router::new(vec![
            Box::new(docfusion_core::extract::text::TextPassthrough)
                as Box<dyn docfusion_core::extract::Extractor>,
        ])
    }

    /// What the dialog needs to know before it draws: whether there is
    /// anything to delete.
    #[test]
    fn a_record_says_whether_its_markdown_is_still_there() {
        let (dir, _pipeline, id, output) = one_document();

        let stored = {
            let pipeline = Pipeline::open(
                dir.path(),
                Router::new(vec![Box::new(docfusion_core::extract::text::TextPassthrough)
                    as Box<dyn docfusion_core::extract::Extractor>]),
            )
            .unwrap();
            pipeline.manifest().find(&id).unwrap().expect("a record")
        };

        assert!(Record::from_stored(&stored, dir.path()).on_disk);

        std::fs::remove_file(&output).unwrap();
        assert!(!Record::from_stored(&stored, dir.path()).on_disk);
    }

    /// The pane is emptied when — and only when — the document it is showing
    /// has just been taken out of the knowledge base.
    ///
    /// The first version of this was a method called after the view had already
    /// been cloned, so the answer carried a screen from before the delete and
    /// the pane kept showing a document whose file was gone. Nothing could see
    /// it: an `App` cannot be pointed at a temporary directory, which is why
    /// this is over a lock instead.
    #[test]
    fn the_pane_is_emptied_only_for_the_document_that_went() {
        let showing = |id: &str| {
            let mut view = View::new("/library", "/config.toml");
            view.ready(Opened {
                id: id.to_owned(),
                name: "notes.txt".to_owned(),
                output: "docs/notes.md".to_owned(),
                html: "<p>the words</p>".to_owned(),
                engines: vec!["text-passthrough".to_owned()],
                conflicts: 0,
                report: Vec::new(),
                reused: false,
            });
            Mutex::new(view)
        };

        let mine = showing("aaa");
        blank_if_showing(&mine, "aaa");
        assert_eq!(mine.lock().unwrap().screen, Screen::Empty);

        // A different record: somebody tidying the list behind the document
        // they are reading has not asked to lose their place.
        let someone_elses = showing("aaa");
        blank_if_showing(&someone_elses, "bbb");
        assert!(
            matches!(someone_elses.lock().unwrap().screen, Screen::Ready(_)),
            "the document on screen was not the one that went"
        );

        // And a pane that is already empty stays empty rather than becoming
        // something else.
        let nothing = Mutex::new(View::new("/library", "/config.toml"));
        blank_if_showing(&nothing, "aaa");
        assert_eq!(nothing.lock().unwrap().screen, Screen::Empty);
    }

    /// The number the pane hangs on a block is the number an export takes.
    ///
    /// The whole feature rests on this. A block's buttons send back the
    /// `data-block` of the element they were drawn on, and the other side takes
    /// that index out of `render::blocks` — so if the two ever numbered
    /// differently, pressing Export on a table would write a paragraph, and
    /// nothing on screen would say so.
    ///
    /// The wrapper is what makes them agree rather than two lists that happen
    /// to line up, which is what this checks: the number, the block inside it,
    /// and that there is exactly one of each.
    #[test]
    fn the_pane_numbers_the_blocks_the_way_an_export_takes_them() {
        let markdown = "# One\n\nA paragraph.\n\n- one\n- two\n\n> quoted\n\n\
                        | a | b |\n| --- | --- |\n| c | d |\n\n```\ncode\n```\n";

        let drawn = window_html(markdown);
        let blocks = render::blocks(markdown);

        // Or the loops below would pass by finding nothing.
        assert!(blocks.len() > 4, "only {} blocks were drawn", blocks.len());
        assert_eq!(drawn.matches("data-block=").count(), blocks.len());

        for (at, block) in blocks.iter().enumerate() {
            assert!(
                drawn.contains(&format!(
                    "data-block=\"{at}\"><div class=\"block-body\">{}</div></section>",
                    block.html
                )),
                "the block numbered {at} is not the one that came out of `blocks`"
            );
        }

        // The one table is marked as one, and it is the only thing that is —
        // which is the fact the window's list of formats is built from.
        assert_eq!(blocks.iter().filter(|block| block.table).count(), 1);
        assert_eq!(drawn.matches("block-table").count(), 1);
    }

    /// Every element the script looks up is in the markup.
    ///
    /// `el()` throws when the id is not there, and the throw lands wherever in
    /// the module it happens to be — so a mistyped id does not break one panel,
    /// it breaks everything the module does after that line. Two of these are
    /// made by the script itself, at the moment it draws them.
    #[test]
    fn every_element_the_script_looks_up_is_in_the_markup() {
        let html = include_str!("../ui/index.html");
        let js = include_str!("../ui/app.js");

        let made_by_the_script = ["findings", "elapsed"];
        let mut looked = 0;

        for id in js
            .match_indices("el(\"")
            .filter_map(|(at, _)| js[at + 4..].split('"').next())
        {
            if made_by_the_script.contains(&id) {
                continue;
            }

            looked += 1;
            assert!(
                html.contains(&format!("id=\"{id}\"")),
                "the script looks up `{id}` and the markup has no such element"
            );
        }

        assert!(looked > 20, "only {looked} ids were checked");
    }

    /// **A file that has pages, and a file that does not.**
    ///
    /// The two answers the source pane is built on, and the second is the one
    /// worth a test: most of what this program reads would be a pane with
    /// nothing in it, and "no pages to show" and "one page" are different
    /// answers that end up as different things on screen.
    #[test]
    fn only_a_file_with_pages_has_any_to_look_at() {
        let page = docfusion_core::image::number_page(4321)
            .to_png()
            .expect("a picture of a number");

        assert_eq!(page_count(&page, "image/png").unwrap(), Some(1));

        // Rendering a page is the PDF's, and bytes that are not a PDF cannot be
        // asked for one: a copy that has gone bad says so rather than saying
        // there is nothing to look at.
        assert!(rendered_page(&page, 0).is_err());

        // An Office document is a zip of XML and a text file's page is its
        // words: neither has a picture to put in that half.
        assert_eq!(page_count(b"PK", "application/msword").unwrap(), None);
        assert_eq!(page_count(b"words", "text/plain").unwrap(), None);

        assert!(page_count(b"%PDF-1.4 not really a pdf", "application/pdf").is_err());
    }

    /// **No name is declared twice at the top of the script.**
    ///
    /// This is a test because the failure it catches is total and silent: two
    /// declarations of one name at the top of a module is a `SyntaxError` at
    /// parse time, which takes the whole file with it — no wiring, no drawing, a
    /// window that opens and does nothing. It happened, on the day the editor
    /// was added: the settings form's `dirty()` and the editor's `dirty`
    /// collided, and everything in the window stopped, including the parts that
    /// had nothing to do with editing.
    ///
    /// Top level only, which is what the column check is for: a name inside a
    /// function may shadow anything it likes, and this is about the one scope
    /// where a repeat is an error rather than a choice.
    #[test]
    fn no_name_is_declared_twice_at_the_top_of_the_script() {
        let js = include_str!("../ui/app.js");
        let mut seen: Vec<String> = Vec::new();
        let mut looked = 0;

        for line in js.lines() {
            let rest = ["async function ", "function ", "let ", "const ", "var "]
                .iter()
                .find_map(|head| line.strip_prefix(head));
            let Some(rest) = rest else { continue };

            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if name.is_empty() {
                continue;
            }

            looked += 1;
            assert!(
                !seen.contains(&name),
                "`{name}` is declared twice at the top of the script, which is a                  SyntaxError that takes the whole module with it"
            );
            seen.push(name);
        }

        // A floor rather than a count: what it catches is the scan above quietly
        // finding nothing, which would make everything after it pass.
        assert!(looked > 60, "only {looked} names were checked");
    }

    /// Every sentence the script asks for has been written.
    ///
    /// The failure this catches is silent on screen: a key that is not there is
    /// `undefined`, and `undefined` interpolated into a template is drawn as the
    /// word. A rename on one side of the boundary is the whole of the risk, and
    /// nothing else here can see across it.
    ///
    /// One way of asking, counted everywhere it appears: `TEXT.name`, whether
    /// that is inline or in the run that fills the markup. It was two ways while
    /// there were two languages and a `t()` that chose between them; there is one
    /// table now and the name is read straight out of it.
    #[test]
    fn every_sentence_the_script_asks_for_has_been_written() {
        let js = include_str!("../ui/app.js");

        let table = blocks(js, "const TEXT = {");
        assert_eq!(table.len(), 1, "there should be one table of words");

        let written = keys_in(table[0]);
        let mut asked = 0;

        for (at, _) in js.match_indices("TEXT.") {
            let rest = &js[at + "TEXT.".len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();

            if name.is_empty() {
                continue;
            }

            asked += 1;
            assert!(written.contains(&name), "`{name}` is asked for and not written");
        }

        // A floor rather than a count: what it catches is the parse above
        // quietly finding nothing, which would make everything after it pass.
        assert!(asked > 80, "only {asked} sentences were checked");
    }

    /// The keys of an object literal, as the frontend writes them.
    fn keys_in(block: &str) -> Vec<String> {
        let mut found = Vec::new();

        for line in block.lines() {
            let line = line.trim_start();

            // Two spellings, because JavaScript has two: a key that is an
            // identifier is written bare and one with a dash in it is quoted.
            let named = match line.strip_prefix('"') {
                Some(rest) => match rest.split_once('"') {
                    Some((name, after)) if after.starts_with(':') => Some(name),
                    _ => None,
                },
                None => match line.split_once(':') {
                    Some((name, _))
                        if !name.is_empty()
                            && name
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '_') =>
                    {
                        Some(name)
                    }
                    _ => None,
                },
            };

            if let Some(name) = named {
                found.push(name.to_owned());
            }
        }

        found
    }

    /// Every name an engine can give itself has words behind it.
    ///
    /// **This is the test for something that went wrong.** `rapidocr+layout+tables`
    /// was drawn raw in the status bar — in Chinese, to somebody who had just
    /// installed all three models and could not read it. The window's list held
    /// the names it had thought of, and an engine assembles its name out of
    /// what it had to read with, so installing a model produced a name nobody
    /// had written words for.
    ///
    /// The window now reads an assembled name back the same way it was built,
    /// so what has to be named is the **parts** rather than the combinations.
    /// The list here is every part this program can produce: the engines that
    /// read a page, and the two capabilities that get appended to one. It is
    /// short because the ways of appending are two — see `RapidOcrBackend::id`
    /// and `PdfTextLayer::id`, which are the only places a `+` is put in.
    #[test]
    fn every_part_of_an_engine_name_has_words() {
        let js = include_str!("../ui/app.js");
        let table = blocks(js, "engine_names: {");

        assert_eq!(table.len(), 1, "there should be one list of engine names");

        let written = keys_in(table[0]);

        for part in [
            // What can read a page.
            "pdfium",
            "scan",
            "docx",
            "xlsx",
            "pptx",
            "text-passthrough",
            "rapidocr",
            "openai-compatible",
            // What gets appended to one, wherever the `+` is written.
            "layout",
            "tables",
        ] {
            assert!(
                written.iter().any(|key| key == part),
                "no name for `{part}`, so an engine called that is drawn raw"
            );
        }
    }

    /// Every kind of failure has a sentence.
    ///
    /// The gap this exists to catch was real and was here: `Download` was added
    /// to `Cause` for the local models, reached the backend and the failure
    /// panel, and never reached the strings — so a download that failed was
    /// drawn as "Something went wrong.", which is the one thing it was not.
    ///
    /// The list of causes is matched exhaustively below, so that adding one
    /// fails to compile rather than failing silently.
    #[test]
    fn every_cause_has_a_sentence() {
        fn spoken_for(cause: Cause) {
            match cause {
                Cause::Unsupported
                | Cause::EngineFailed
                | Cause::Unreadable
                | Cause::Unwritable
                | Cause::Index
                | Cause::Missing
                | Cause::Settings
                | Cause::NothingToRead
                | Cause::Download
                | Cause::Delete => {}
            }
        }

        let js = include_str!("../ui/app.js");
        let causes = blocks(js, "causes: {");

        assert_eq!(causes.len(), 1, "there should be one set of sentences");

        for cause in [
            Cause::Unsupported,
            Cause::EngineFailed,
            Cause::Unreadable,
            Cause::Unwritable,
            Cause::Index,
            Cause::Missing,
            Cause::Settings,
            Cause::NothingToRead,
            Cause::Download,
            Cause::Delete,
        ] {
            spoken_for(cause);

            let written = serde_json::to_string(&cause).expect("a cause serialises");
            let name = written.trim_matches('"');

            // Two spellings, because JavaScript has two: a key that happens to
            // be an identifier is written bare and one with a dash in it is
            // quoted. Most of these have a dash in them, which is why the
            // quoted form is the one that looks like the rule.
            let written = [format!("\"{name}\":"), format!("{name}:")];

            assert!(
                written.iter().any(|form| causes[0].contains(form.as_str())),
                "cause `{name}` is missing from the set of sentences"
            );
        }
    }

    /// Every `opener … }` in the frontend that opens a balanced block.
    ///
    /// Brace-matched rather than searched to the next `}`, because these hold
    /// nested objects — the causes are inside the language blocks — and a
    /// search for the next closing brace would end one at the first inner one.
    ///
    /// The opener has to begin a word, which it does not by default: `en: {`
    /// is somewhere inside `screen: { kind: … }`, and the first version of this
    /// counted that as a second English block.
    fn blocks<'a>(js: &'a str, opener: &str) -> Vec<&'a str> {
        let mut found = Vec::new();
        let mut from = 0;

        while let Some(at) = js[from..].find(opener) {
            let start = from + at;
            from = start + 1;

            let before = js[..start].chars().next_back();
            if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }

            let mut depth = 0;
            let mut end = js.len();

            for (offset, character) in js[start..].char_indices() {
                match character {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = start + offset;
                            break;
                        }
                    }
                    _ => {}
                }
            }

            found.push(&js[start..end]);
            from = end;
        }

        found
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
                PipelineError::Extract(ExtractError::unavailable(
                    engine(),
                    "the models are not on disk: ppocrv6-tiny/PP-OCRv6_det_tiny.onnx".to_owned(),
                )),
                Cause::Download,
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
                PipelineError::NoSuchDocument {
                    query: "aaaa".to_owned(),
                },
                Cause::Missing,
            ),
            (
                PipelineError::Ambiguous {
                    query: "aa".to_owned(),
                    count: 2,
                },
                Cause::Missing,
            ),
            (
                PipelineError::Delete {
                    path: "docs/notes.md".into(),
                    source: std::io::Error::other("in use"),
                },
                Cause::Delete,
            ),
            (
                PipelineError::Outside {
                    path: "../elsewhere.md".into(),
                },
                Cause::Delete,
            ),
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
