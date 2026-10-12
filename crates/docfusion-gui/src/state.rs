//! What the window is showing.
//!
//! Plain data with no window types in it, so the part that decides what the
//! interface looks like can be tested without starting one. The window is a
//! view of this and nothing more: every branch the interface can take is a
//! value here, and every one of them is reachable from a test.

use docfusion_core::ir::Shapes;
use docfusion_store::manifest::DocumentRecord;
use serde::Serialize;

/// Everything the window needs to draw itself.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct View {
    pub knowledge_base: String,
    pub config_path: String,
    /// Something wrong with the installation rather than with a document.
    ///
    /// Separate from `Screen` because it is true whatever is on screen: a
    /// settings file that cannot be read is worth saying once, at the top,
    /// rather than replacing whatever the person is looking at.
    pub problem: Option<Trouble>,
    pub screen: Screen,
    pub records: Vec<Record>,
    pub model: Model,
    pub local: Local,
}

/// What the settings tab shows about reading with a model.
///
/// The key is not in here. This view is serialised for every command the window
/// sends — every document read carries one — and a secret that travels on every
/// call is a secret in more places than the one that needs it. What is here is
/// how long the key is, which is enough to mask the field with the right number
/// of dots and says nothing about the key itself.
///
/// Seeing the key is a thing somebody asks for, so it is a command of its own:
/// `reveal_key`, in `lib.rs`. It crosses the boundary when the eye is pressed
/// and at no other time.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Model {
    pub endpoint: String,
    pub model: String,
    /// In characters, because it is a count of dots a person looks at.
    pub key_length: usize,
    /// Whether the model has been told it may think before answering.
    ///
    /// Off unless somebody says otherwise, because the provider's default is on
    /// and it spends the majority of the output tokens. There is a switch
    /// rather than a constant because the request carries the setting, and a
    /// provider that rejects a field it does not recognise needs to be able to
    /// not have it sent.
    pub reasoning: bool,
    /// Whether the model is used at all.
    ///
    /// A switch rather than the absence of an endpoint, because emptying the
    /// endpoint to stop the model also throws away the address and the key.
    pub enabled: bool,
}

/// The one set of models this app reads a page with, and how much of it is
/// here.
///
/// One set rather than a list, which is the shape this type took when the
/// window stopped drawing two models and started drawing one card: what it has
/// to know is whether the set is complete, how far along a fetch is, and what
/// the whole thing weighs. Which files make it up is the backend's business —
/// see `docfusion_store::models`.
///
/// Not part of [`Model`], which is about a model at the other end of a network
/// and the key that reaches it. These are files on this disk, nothing here goes
/// to an endpoint, and no setting switches them off.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Local {
    /// Whether every file of the set is here, so that a page reads as headings,
    /// tables and text rather than as a run of lines.
    pub ready: bool,
    /// Whether a fetch is running now.
    pub fetching: bool,
    /// How many bytes a fetch would still have to transfer.
    ///
    /// What is left rather than the whole set, because a fetch is resumed
    /// rather than restarted: after a cancelled one the window would otherwise
    /// promise to download megabytes it already has.
    pub needed_bytes: u64,
    /// What the whole set weighs, which is what the card says before anything
    /// has been asked for.
    ///
    /// A fact about this build rather than about this machine, and it is here
    /// rather than written into the frontend so that the number on the card and
    /// the number of bytes that arrive cannot drift apart.
    pub total_bytes: u64,
}

/// One document a search found.
///
/// Its own type rather than a [`Record`] with the passage in it, because the
/// two are asked for differently and drawn differently: a record is the
/// archive's whole list and a match is the answer to a question, and merging
/// them would mean the list had a field that is empty except after a search.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Match {
    pub id: String,
    /// The file this was read from, as somebody would recognise it.
    pub name: String,
    /// The passage the match is in, with it between two control characters the
    /// window splits on — see [`docfusion_core::text::HIT_START`]. Not markup:
    /// the text is somebody's document, and the window builds nodes out of it
    /// rather than parsing anything.
    ///
    /// `None` when the document's Markdown has gone since it was indexed, which
    /// is drift rather than a failure and is said in words rather than by
    /// dropping the row.
    pub excerpt: Option<String>,
}

/// Something that went wrong, split the way the window has to say it.
///
/// Two halves, and the split is the point. `cause` is the kind of failure, the
/// only part this program owns and therefore the only part it can put into the
/// reader's language. `detail` is the account the engine or the operating
/// system gave — somebody else's words, in somebody else's language — and it is
/// carried through rather than paraphrased, because a paraphrase of a cause is
/// a guess dressed as an explanation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Trouble {
    pub cause: Cause,
    pub detail: String,
}

impl Trouble {
    pub fn new(cause: Cause, detail: impl Into<String>) -> Self {
        Self {
            cause,
            detail: detail.into(),
        }
    }
}

/// What kind of failure this is.
///
/// Deliberately the kind and not the message. The library's messages are
/// English and are what the command line prints; the window picks the kind and
/// writes its own sentence, because matching on a sentence to decide what to
/// say would come apart the moment somebody reworded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cause {
    /// Nothing here can read this kind of file.
    Unsupported,
    /// Something that could have read it failed trying.
    EngineFailed,
    /// A file could not be read.
    Unreadable,
    /// The result could not be written.
    Unwritable,
    /// The index of what is already in the knowledge base is not usable.
    Index,
    /// The index does not have the document that was asked for.
    Missing,
    /// The settings file is not usable.
    Settings,
    /// It was read, and there was nothing in it.
    NothingToRead,
    /// A model this app reads pages with is not here, or could not be fetched.
    ///
    /// One cause for the two halves, because from where the reader is standing
    /// they are one thing: something has not been downloaded, and there is a
    /// button for it in the settings tab. Whether the fetch was never asked for
    /// or was asked for and failed changes the sentence not at all.
    Download,
    /// A document could not be taken out of the knowledge base.
    ///
    /// Its own kind rather than [`Cause::Unwritable`], which is about writing a
    /// result and would be a sentence about the wrong operation — and it is the
    /// one failure here that leaves a row and a file disagreeing, so it is
    /// worth being able to say which half went wrong.
    Delete,
}

/// What a document's source is, for looking at.
///
/// Not part of [`View`], which is serialised for every command the window sends
/// and would carry a page count for every document in the list. This is asked
/// for when somebody opens the source pane, and for the document on screen.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Source {
    /// Pages that can be shown: a PDF, which is rendered one at a time, or an
    /// image, which is one page and is already one.
    Pages { count: usize },
    /// A file with no pages to show.
    ///
    /// An Office document is a zip of XML and there is no picture of it
    /// anywhere; a text file's page is its words, which are already on the other
    /// side of the pane. A name for it rather than an empty half, because an
    /// empty half reads as a pane that failed to load.
    Nothing,
}

/// Which of the four things the window can be showing.
///
/// Tagged for the frontend, which switches on `kind` and never has to guess
/// which fields are present.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Screen {
    /// Nothing has been opened yet.
    Empty,
    /// A document is being read.
    ///
    /// Worth a state of its own because reading is not instant: a scan takes
    /// seconds a page, and a window that cannot say what it is waiting for
    /// looks like one that has stopped working.
    Working { name: String },
    Ready(Opened),
    Failed { name: String, trouble: Trouble },
}

/// A document that has been read and is on screen.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Opened {
    pub id: String,
    /// The file somebody chose, as they would recognise it.
    pub name: String,
    /// Where the result was written inside the knowledge base.
    pub output: String,
    /// The document as HTML, rendered by the same Markdown pipeline the command
    /// line writes with.
    pub html: String,
    pub engines: Vec<String>,
    pub conflicts: usize,
    /// What the tool had to say about the document, kept apart from it.
    ///
    /// The Markdown carries the same thing at the end, under a heading this
    /// program writes, so that a disagreement travels with the document and an
    /// agent reading it sees the doubt as well as the text. That is the right
    /// place for it in a file. It is the wrong place for it on a screen, where
    /// a report about the document sitting inside the document reads as though
    /// somebody had written it into the page.
    pub report: Vec<Conflict>,
    /// True when the answer came from the index rather than from reading the
    /// document again.
    pub reused: bool,
}

/// What asking the endpoint which models it serves turned up.
///
/// Three outcomes, because the three need different things said about them.
/// "It does not list them" is not a fault and must not read like one — a server
/// that reads pages perfectly well is under no obligation to implement a
/// listing, and telling somebody to go and repair it would be sending them
/// after the wrong thing.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum Remote {
    /// The models it is serving that can be shown a page, and how many it named
    /// that cannot.
    ///
    /// The blind ones are left out rather than listed and disabled: they are
    /// not choices somebody declines, they are models that cannot do the job at
    /// all. The count is carried so that leaving them out is said rather than
    /// done silently.
    Models { names: Vec<String>, blind: usize },
    /// It answered, and does not list its models.
    Unlisted { detail: String },
    /// It could not be reached, or refused. The detail is its own account.
    Failed { detail: String },
}

/// What came of asking the endpoint to read a page it has never seen.
///
/// Four outcomes rather than a yes and a no, because "it answered and the
/// answer was wrong" is the one a person most needs told apart and the one a
/// connection check cannot see: a model served without its vision half answers
/// every request in fluent text about nothing.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum Test {
    /// It read what it was shown.
    Read,
    /// It answered, and the answer was not what it was shown.
    Misread { said: String, expected: String },
    /// It never got as far as answering. The detail is the endpoint's own
    /// account of why.
    Failed { detail: String },
    /// There is no endpoint to ask — the switch is off, or the address and the
    /// model are not both filled in.
    ///
    /// **Its own outcome rather than a `Failed`, and this is the whole reason
    /// it exists.** Nothing was asked and nothing went wrong, so "did not
    /// answer" is a claim about a request that was never made; the window said
    /// exactly that, and printed the English detail underneath it, so a
    /// Chinese screen read *"端点没能回答：no endpoint is configured"*. Both
    /// halves of that sentence were wrong and they contradicted each other.
    Unconfigured,
}

/// One place two engines read the same part of a page differently.
///
/// Neither reading is preferred and neither is marked as the right one. The
/// tool has no basis for choosing — that is the whole reason it reports this
/// rather than resolving it — so both are carried here and the person decides.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conflict {
    /// One-based, because it is said to a person rather than to a page index.
    pub page: u32,
    /// The kind of disagreement, in the words the report used. Matched against
    /// a table of the kinds this program writes, and shown as it came when it
    /// is one the table does not have.
    pub kind: String,
    pub readings: Vec<Reading>,
}

/// What one engine read, as it wrote it down, and as what.
///
/// The text and the shape of it are both here because neither is much use
/// alone: two engines returning the same words have agreed about nothing if one
/// read a table and the other read a paragraph, and the words are identical in
/// both.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reading {
    pub engine: String,
    pub text: String,
    /// `None` for a report that did not say — an older document, or a line this
    /// does not understand. The text is still worth showing; the shape is
    /// simply not known.
    pub shapes: Option<Shapes>,
}

/// One line of the records list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Record {
    pub id: String,
    pub name: String,
    /// Where the result was written, relative to the knowledge base.
    pub output: String,
    /// Whether that file is still there.
    ///
    /// The window offers to delete it, and a document whose Markdown has
    /// already gone has nothing to delete. Asked here rather than discovered by
    /// a button that fails, because the answer decides whether the button is
    /// offered at all — and a greyed-out button beside the sentence saying why
    /// is the version of that people do not press twice.
    pub on_disk: bool,
    pub status: String,
    pub conflicts: u32,
    pub created_at: String,
    /// Whether somebody has edited this document's words by hand.
    ///
    /// A document that has is no longer what the engines read, and the row says
    /// so because the difference matters: a re-read will take it away, and
    /// somebody looking at the list should be able to see which ones they have
    /// touched before that happens.
    pub edited: bool,
}

impl Record {
    /// The list the interface shows, newest first, from what the index holds.
    ///
    /// `newest last` is what this said until the list was drawn the right way
    /// up: the index has always answered newest-first, and the window was
    /// reversing it. See `the_records_list_is_drawn_in_the_order_it_arrives`.
    ///
    /// The path is carried through as text rather than as a path, because the
    /// window only ever displays it and a `PathBuf` serialises differently on
    /// Windows than it reads.
    ///
    /// Takes the root because the index records the output relative to it, and
    /// whether the file is there is a question for the disk rather than for the
    /// index — a document whose Markdown somebody deleted by hand is still a row
    /// until `reconcile` catches up.
    pub fn from_stored(record: &DocumentRecord, root: &std::path::Path) -> Self {
        Self {
            id: record.id.clone(),
            name: file_name(&record.source_path.to_string_lossy()),
            output: record.output_path.to_string_lossy().into_owned(),
            on_disk: root.join(&record.output_path).is_file(),
            status: record.status.as_str().to_owned(),
            conflicts: record.conflict_count,
            created_at: record.created_at.clone(),
            edited: record.edited_at.is_some(),
        }
    }
}

impl View {
    pub fn new(knowledge_base: impl Into<String>, config_path: impl Into<String>) -> Self {
        Self {
            knowledge_base: knowledge_base.into(),
            config_path: config_path.into(),
            problem: None,
            screen: Screen::Empty,
            records: Vec::new(),
            model: Model::default(),
            local: Local::default(),
        }
    }

    pub fn working(&mut self, name: impl Into<String>) {
        self.screen = Screen::Working { name: name.into() };
    }

    pub fn failed(&mut self, name: impl Into<String>, trouble: Trouble) {
        self.screen = Screen::Failed {
            name: name.into(),
            trouble,
        };
    }

    pub fn ready(&mut self, opened: Opened) {
        self.screen = Screen::Ready(opened);
    }
}

/// The last component of a path, whatever the separator.
///
/// Written out rather than taken from `Path::file_name`, because this runs on
/// a string that came from JSON rather than from the filesystem.
pub fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_view_is_empty_and_knows_where_things_are() {
        let view = View::new("kb", "config.toml");

        assert_eq!(view.screen, Screen::Empty);
        assert_eq!(view.knowledge_base, "kb");
        assert_eq!(view.config_path, "config.toml");
        assert!(view.records.is_empty());
    }

    /// Reading is not instant, and a window that cannot say what it is waiting
    /// for looks like one that has stopped working.
    #[test]
    fn the_window_can_say_what_it_is_waiting_for() {
        let mut view = View::new("kb", "config.toml");

        view.working("report.pdf");

        assert_eq!(
            view.screen,
            Screen::Working {
                name: "report.pdf".to_owned()
            }
        );
    }

    /// What the window shows about a document is what was read from it, and the
    /// footer is drawn from these fields rather than from the HTML.
    #[test]
    fn a_document_on_screen_carries_what_was_read_from_it() {
        let mut view = View::new("kb", "config.toml");

        view.ready(opened());

        match view.screen {
            Screen::Ready(document) => {
                assert_eq!(document.engines, vec!["pdfium"]);
                assert_eq!(document.output, "kb/docs/report.md");
                assert!(document.html.contains("<h1>"));
                assert!(!document.reused);
            }
            other => panic!("expected a document, got {other:?}"),
        }
    }

    /// A failure keeps the name, because the useful question about something
    /// that would not open is which thing it was — and it keeps the kind and
    /// the account apart, because the window says the first in the reader's
    /// language and passes the second through.
    #[test]
    fn a_failure_keeps_the_kind_and_the_account_apart() {
        let mut view = View::new("kb", "config.toml");

        view.failed(
            "report.pdf",
            Trouble::new(Cause::EngineFailed, "could not reach the endpoint"),
        );

        match view.screen {
            Screen::Failed { name, trouble } => {
                assert_eq!(name, "report.pdf");
                assert_eq!(trouble.cause, Cause::EngineFailed);
                assert!(trouble.detail.contains("could not reach"));
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    /// The frontend picks its words by reading this string, so it is a contract
    /// between two languages and it is checked where it is written rather than
    /// discovered when a message comes out blank.
    #[test]
    fn the_kinds_of_failure_have_the_names_the_frontend_looks_up() {
        let names: Vec<String> = [
            Cause::Unsupported,
            Cause::EngineFailed,
            Cause::Unreadable,
            Cause::Unwritable,
            Cause::Index,
            Cause::Missing,
            Cause::Settings,
            Cause::NothingToRead,
        ]
        .into_iter()
        .map(|cause| serde_json::to_value(cause).unwrap().as_str().unwrap().to_owned())
        .collect();

        assert_eq!(
            names,
            [
                "unsupported",
                "engine-failed",
                "unreadable",
                "unwritable",
                "index",
                "missing",
                "settings",
                "nothing-to-read",
            ]
        );
    }

    /// The list is drawn from the index, so the mapping from what is stored to
    /// what is shown is the one thing that has to be right about it.
    #[test]
    fn a_record_shows_the_file_somebody_chose() {
        let mut record = stored();
        record.source_path = "C:\\notes\\2024 annual report.pdf".into();

        let shown = Record::from_stored(&record, std::path::Path::new(""));

        assert_eq!(shown.name, "2024 annual report.pdf");
        assert_eq!(shown.status, "done");
        assert_eq!(shown.conflicts, 3);
    }

    #[test]
    fn a_record_from_a_unix_path_also_shows_just_the_file() {
        let mut record = stored();
        record.source_path = "/home/someone/scan.png".into();

        assert_eq!(Record::from_stored(&record, std::path::Path::new("")).name, "scan.png");
    }

    fn opened() -> Opened {
        Opened {
            id: "0".repeat(64),
            name: "report.pdf".to_owned(),
            output: "kb/docs/report.md".to_owned(),
            html: "<h1>Report</h1>".to_owned(),
            engines: vec!["pdfium".to_owned()],
            conflicts: 0,
            report: Vec::new(),
            reused: false,
        }
    }

    fn stored() -> DocumentRecord {
        use docfusion_core::ir::EngineStamp;
        use docfusion_store::manifest::Status;

        DocumentRecord {
            id: "0".repeat(64),
            source_path: "report.pdf".into(),
            source_size: 10,
            source_mtime: None,
            mime: "application/pdf".into(),
            output_path: "kb/docs/report.md".into(),
            engine: EngineStamp::new("pdfium", "1.0.0"),
            status: Status::Done,
            verified: false,
            conflict_count: 3,
            created_at: "2026-10-08T12:00:00Z".into(),
            updated_at: "2026-10-08T12:00:00Z".into(),
            output_hash: None,
            source_copy: None,
            edited_at: None,
        }
    }
}
