//! What the window is showing.
//!
//! Plain data with no window types in it, so the part that decides what the
//! interface looks like can be tested without starting one. The window is a
//! view of this and nothing more: every branch the interface can take is a
//! value here, and every one of them is reachable from a test.

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
    /// True when the answer came from the index rather than from reading the
    /// document again.
    pub reused: bool,
}

/// One line of the records list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Record {
    pub id: String,
    pub name: String,
    pub output: String,
    pub status: String,
    pub conflicts: u32,
    pub created_at: String,
}

impl Record {
    /// The list the interface shows, newest last, from what the index holds.
    ///
    /// The path is carried through as text rather than as a path, because the
    /// window only ever displays it and a `PathBuf` serialises differently on
    /// Windows than it reads.
    pub fn from_stored(record: &DocumentRecord) -> Self {
        Self {
            id: record.id.clone(),
            name: file_name(&record.source_path.to_string_lossy()),
            output: record.output_path.to_string_lossy().into_owned(),
            status: record.status.as_str().to_owned(),
            conflicts: record.conflict_count,
            created_at: record.created_at.clone(),
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

        let shown = Record::from_stored(&record);

        assert_eq!(shown.name, "2024 annual report.pdf");
        assert_eq!(shown.status, "done");
        assert_eq!(shown.conflicts, 3);
    }

    #[test]
    fn a_record_from_a_unix_path_also_shows_just_the_file() {
        let mut record = stored();
        record.source_path = "/home/someone/scan.png".into();

        assert_eq!(Record::from_stored(&record).name, "scan.png");
    }

    fn opened() -> Opened {
        Opened {
            id: "0".repeat(64),
            name: "report.pdf".to_owned(),
            output: "kb/docs/report.md".to_owned(),
            html: "<h1>Report</h1>".to_owned(),
            engines: vec!["pdfium".to_owned()],
            conflicts: 0,
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
        }
    }
}
