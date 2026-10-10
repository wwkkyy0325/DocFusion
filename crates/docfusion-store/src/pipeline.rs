//! The ingestion pipeline: read, hash, extract, render, write, record.
//!
//! This is the effectful orchestration that joins the pure core to the disk.
//! It lives beside the manifest rather than in the CLI so that the GUI runs the
//! same sequence rather than a second, subtly different one.

use std::fs;
use std::path::{Path, PathBuf};

use docfusion_core::extract::{ExtractError, InputProbe, Router};
use docfusion_core::ir::{Document, EngineStamp, SourceRef};
use docfusion_core::mime::SNIFF_LENGTH;
use docfusion_core::render;

use crate::atomic::write_atomic;
use crate::hash::content_hash;
use crate::manifest::{DocumentRecord, Manifest, Status, StoreError, now_iso8601};

/// Directory holding the sidecar index, kept out of the document tree so that
/// `.md` files stay the only thing in it.
pub const STATE_DIR: &str = ".docfusion";
pub const MANIFEST_FILE: &str = "manifest.db";
/// Extracted documents are written here, under the knowledge root.
pub const DOCS_DIR: &str = "docs";

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("{0}")]
    Extract(#[from] ExtractError),

    #[error("{0}")]
    Store(#[from] StoreError),

    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("the extractor produced a document with no pages, so it has no engine to record")]
    UnidentifiableEngine,

    #[error("no document in the knowledge base matches {query}")]
    NoSuchDocument { query: String },

    #[error("{query} names {count} documents; give more of the id")]
    Ambiguous { query: String, count: usize },

    #[error("could not delete {path}: {source}")]
    Delete {
        path: PathBuf,
        source: std::io::Error,
    },

    /// Refused rather than attempted, which is the one thing here that has to
    /// be: this is the message that stops a delete from following a path out
    /// of the knowledge base.
    #[error("{path} is not inside the knowledge base, so nothing was deleted")]
    Outside { path: PathBuf },
}

/// Whether a run should change anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Apply,
    /// Runs everything except the write and the manifest update, so that what
    /// is reported is the real result rather than a prediction of one.
    DryRun,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// BLAKE3 of the source bytes: the document's identity.
    pub id: String,
    pub source: PathBuf,
    pub output: PathBuf,
    /// True when a previous result was reused and nothing was extracted.
    pub reused: bool,
    pub conflicts: usize,
    pub status: Status,
}

pub struct Pipeline {
    root: PathBuf,
    manifest: Manifest,
    router: Router,
}

impl Pipeline {
    /// Opens a knowledge base, creating its directories if they are missing.
    pub fn open(root: &Path, router: Router) -> Result<Self, PipelineError> {
        let docs = root.join(DOCS_DIR);
        let state = root.join(STATE_DIR);
        create_dir(&docs)?;
        create_dir(&state)?;

        Ok(Self {
            manifest: Manifest::open(&state.join(MANIFEST_FILE))?,
            root: root.to_path_buf(),
            router,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The document `query` names.
    ///
    /// Everything [`Manifest::find`] accepts — an id, a path, a file name —
    /// plus the **first few characters of an id**, which is what somebody
    /// working from `docfusion list` actually has in front of them: an id is a
    /// 64-character hash of the document's bytes, and nobody types one.
    ///
    /// A prefix naming more than one document is refused rather than guessed
    /// at. The two commands that take a document by name are the two that take
    /// one *away*, and "whichever of these you meant" is not an answer worth
    /// giving when the answer might be the wrong document.
    pub fn resolve(&self, query: &str) -> Result<DocumentRecord, PipelineError> {
        let query = query.trim();

        if let Some(record) = self.manifest.find(query)? {
            return Ok(record);
        }

        let named: Vec<DocumentRecord> = if query.is_empty() {
            Vec::new()
        } else {
            self.manifest
                .all()?
                .into_iter()
                .filter(|record| record.id.starts_with(query))
                .collect()
        };

        match named.len() {
            0 => Err(PipelineError::NoSuchDocument {
                query: query.to_owned(),
            }),
            1 => Ok(named.into_iter().next().expect("one, just counted")),
            count => Err(PipelineError::Ambiguous {
                query: query.to_owned(),
                count,
            }),
        }
    }

    /// Takes a document out of the index and leaves its Markdown where it is.
    ///
    /// The reversible half of taking a document out of the knowledge base, and
    /// the reason the two are separate functions rather than one with a flag:
    /// they are two different sentences, and one of them destroys a file.
    ///
    /// Nothing is lost, so nothing here is refused for being destructive. The
    /// document becomes untracked — `reconcile` reports it and does not put it
    /// back — and reading the source again restores it.
    ///
    /// Answers with what it removed, because the caller has to name it: the
    /// window shows the list without the row and the command line prints which
    /// document it just forgot.
    pub fn forget(&self, query: &str) -> Result<DocumentRecord, PipelineError> {
        let record = self.resolve(query)?;
        self.manifest.forget(&record.id)?;

        Ok(record)
    }

    /// Deletes a document and the Markdown it was written to.
    ///
    /// **The file goes first.** A row left behind by a file that would not
    /// delete is a row that opens onto nothing, which every reader of the index
    /// already knows how to show; a file left behind by a row that was removed
    /// is a document nobody can find and nothing that would report it. The
    /// order is chosen so that a failure leaves something somebody can act on.
    ///
    /// **The source document is not here to delete.** The PDF or the picture
    /// was never copied into the knowledge base, so what goes is what this
    /// program wrote — which is why both front ends can say that neither way of
    /// taking a document out touches the file anybody chose.
    pub fn delete(&self, query: &str) -> Result<DocumentRecord, PipelineError> {
        let record = self.resolve(query)?;
        let path = self.output_of(&record)?;

        fs::remove_file(&path).map_err(|source| PipelineError::Delete {
            path: path.clone(),
            source,
        })?;

        self.manifest.forget(&record.id)?;

        Ok(record)
    }

    /// Where a record's Markdown is, once it is known to be somewhere this
    /// pipeline is allowed to reach.
    ///
    /// The one destructive command in the program, and the path comes out of a
    /// database file somebody could have edited by hand. So it is checked
    /// against what a record's output may be rather than trusted.
    ///
    /// `starts_with(root)` is not the check, and looks like it is: it walks
    /// components and `..` is one, so `root.join("../../x")` starts with `root`
    /// and the guard somebody reaches for first passes. A leading slash and a
    /// drive letter have the same problem, which is why all three are refused
    /// before the join rather than after it.
    fn output_of(&self, record: &DocumentRecord) -> Result<PathBuf, PipelineError> {
        let outside = record.output_path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        });

        if outside {
            return Err(PipelineError::Outside {
                path: record.output_path.clone(),
            });
        }

        Ok(self.root.join(&record.output_path))
    }

    pub fn ingest(&mut self, source: &Path, mode: Mode) -> Result<Outcome, PipelineError> {
        let bytes = fs::read(source).map_err(|source_error| PipelineError::Read {
            path: source.to_path_buf(),
            source: source_error,
        })?;

        let id = content_hash(&bytes);
        let probe = InputProbe::new(
            &bytes[..bytes.len().min(SNIFF_LENGTH)],
            source
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
            bytes.len() as u64,
        );

        if let Some(outcome) = self.reuse(&id, source, &probe, mode)? {
            return Ok(outcome);
        }

        let document = self
            .router
            .extract(&probe, &source_ref(source, &id, &probe, &bytes), &bytes)?;
        let stamp = self.stamp_of(&document)?;
        let created_at = now_iso8601();
        let markdown = render::markdown(&document, &created_at);
        let output = self.output_path(&document, &id)?;

        if mode == Mode::Apply {
            write_atomic(&output, markdown.as_bytes()).map_err(|source| PipelineError::Write {
                path: output.clone(),
                source,
            })?;

            self.manifest.upsert(&DocumentRecord {
                id: id.clone(),
                source_path: source.to_path_buf(),
                source_size: bytes.len() as u64,
                source_mtime: mtime(source),
                mime: document.source.mime.clone(),
                output_path: self.relative(&output),
                engine: stamp,
                status: Status::Done,
                verified: document.pages.iter().all(|page| page.provenance.verified),
                conflict_count: document.conflicts.len() as u32,
                created_at: created_at.clone(),
                updated_at: created_at,
                // Recorded so `reconcile` can recognise this output after the
                // user moves it: a rename breaks the path link even though the
                // document itself never changed.
                output_hash: Some(content_hash(markdown.as_bytes())),
            })?;
            self.manifest.add_alias(&id, source)?;
        }

        Ok(Outcome {
            id,
            source: source.to_path_buf(),
            output,
            reused: false,
            conflicts: document.conflicts.len(),
            status: Status::Done,
        })
    }

    /// Returns a previous result when one is still valid.
    ///
    /// Each candidate engine's stamp is tried in the order extraction would try
    /// them, because which engine applies is only settled by opening the input.
    /// The content hash pins that behaviour — the same bytes route the same way
    /// every time — so the first engine that would succeed is the first whose
    /// stamp matches.
    fn reuse(
        &mut self,
        id: &str,
        source: &Path,
        probe: &InputProbe,
        mode: Mode,
    ) -> Result<Option<Outcome>, PipelineError> {
        for engine in self.router.candidates(probe) {
            let Some(record) = self.manifest.lookup(id, &engine.stamp())? else {
                continue;
            };

            // A record whose output has been deleted is not a usable cache
            // entry. Reporting a hit would hand back a path to a file that is
            // not there and skip the write that would put it back, so the
            // lookup is treated as a miss and the document is produced again.
            if !self.root.join(&record.output_path).exists() {
                continue;
            }

            if mode == Mode::Apply {
                self.manifest.add_alias(id, source)?;
            }

            return Ok(Some(Outcome {
                id: id.to_owned(),
                source: source.to_path_buf(),
                output: self.root.join(&record.output_path),
                reused: true,
                conflicts: record.conflict_count as usize,
                status: record.status,
            }));
        }

        Ok(None)
    }

    fn stamp_of(&self, document: &Document) -> Result<EngineStamp, PipelineError> {
        document
            .pages
            .first()
            .and_then(|page| self.router.stamp_of(&page.provenance.primary))
            .ok_or(PipelineError::UnidentifiableEngine)
    }

    /// Chooses where the document should be written.
    ///
    /// Two documents can share a title, and letting the second overwrite the
    /// first would lose a document without saying so, so the second is
    /// disambiguated by its content hash. The hash is stable, so re-processing
    /// the same document always lands on the same name.
    fn output_path(&self, document: &Document, id: &str) -> Result<PathBuf, PipelineError> {
        let stem = render::slug(&render::title(document));
        let plain = self.root.join(DOCS_DIR).join(format!("{stem}.md"));

        match self.manifest.document_at_output(&self.relative(&plain))? {
            Some(owner) if owner != id => Ok(self
                .root
                .join(DOCS_DIR)
                .join(format!("{stem}-{}.md", &id[..8]))),
            _ => Ok(plain),
        }
    }

    /// Paths are recorded relative to the knowledge root so the base directory
    /// can be moved or synced without invalidating the index.
    fn relative(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn create_dir(path: &Path) -> Result<(), PipelineError> {
    fs::create_dir_all(path).map_err(|source| PipelineError::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// Seconds since the Unix epoch, as the manifest stores it.
fn mtime(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_secs() as i64)
}

fn source_ref(source: &Path, id: &str, probe: &InputProbe, bytes: &[u8]) -> SourceRef {
    SourceRef {
        path: source.to_path_buf(),
        content_hash: id.to_owned(),
        mime: probe.container.mime_type().to_owned(),
        size: bytes.len() as u64,
        mtime: mtime(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use docfusion_core::extract::text::TextPassthrough;
    use docfusion_core::extract::Extractor;

    fn router() -> Router {
        Router::new(vec![Box::new(TextPassthrough)])
    }

    fn setup() -> (tempfile::TempDir, Pipeline) {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = Pipeline::open(dir.path(), router()).unwrap();
        (dir, pipeline)
    }

    fn write_source(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    fn markdown_at(path: &Path) -> String {
        fs::read_to_string(path).expect("read output")
    }

    /// Taking a document out of the index leaves the document where it is.
    ///
    /// The whole difference between the two ways out, and the reason one of
    /// them is offered without a confirmation: nothing is destroyed, so there
    /// is nothing to be sure about.
    #[test]
    fn forgetting_leaves_the_markdown_alone() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "the words");
        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();

        let forgotten = pipeline.forget(&wrote.id).expect("forget");

        assert_eq!(forgotten.id, wrote.id);
        assert!(wrote.output.is_file(), "the Markdown should still be here");
        assert!(pipeline.manifest().find(&wrote.id).unwrap().is_none());
    }

    /// The other way out takes the file with it.
    #[test]
    fn deleting_takes_the_markdown_with_it() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "the words");
        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();

        pipeline.delete(&wrote.id).expect("delete");

        assert!(!wrote.output.exists(), "the Markdown should be gone");
        assert!(pipeline.manifest().find(&wrote.id).unwrap().is_none());
    }

    /// **The document somebody chose is never touched, either way.**
    ///
    /// The promise both front ends make in words, kept here in a test. It is
    /// true because nothing ever copied the source into the knowledge base, and
    /// it is the kind of thing that stops being true the moment somebody adds
    /// "keep a copy of what was read" without reading this.
    #[test]
    fn neither_way_out_touches_the_source() {
        for (name, act) in [
            ("forget", Pipeline::forget as fn(&Pipeline, &str) -> _),
            ("delete", Pipeline::delete),
        ] {
            let (dir, mut pipeline) = setup();
            let source = write_source(dir.path(), "notes.txt", "the words");
            pipeline.ingest(&source, Mode::Apply).unwrap();

            act(&pipeline, "notes.txt").unwrap_or_else(|error| panic!("{name}: {error}"));

            assert!(source.is_file(), "{name} removed the document it read");
        }
    }

    /// A document can be named the way somebody has it: eight characters of its
    /// id, which is what `list` prints and all anybody is going to type.
    #[test]
    fn a_prefix_of_an_id_names_the_document() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "the words");
        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();

        assert_eq!(pipeline.resolve(&wrote.id[..8]).unwrap().id, wrote.id);
        assert_eq!(pipeline.resolve("notes.txt").unwrap().id, wrote.id);
        assert_eq!(pipeline.resolve(&wrote.id).unwrap().id, wrote.id);
    }

    /// A document whose id is not a hash of anything, for the tests that need
    /// two that start the same way.
    ///
    /// Real ids are BLAKE3 sums, so a shared prefix cannot be arranged by
    /// choosing the input — and a test that waited for two hashes to collide
    /// would pass by taking its other branch almost every time it ran.
    fn record_named(id: &str, output: &str) -> DocumentRecord {
        DocumentRecord {
            id: id.to_owned(),
            source_path: PathBuf::from(format!("{id}.txt")),
            source_size: 1,
            source_mtime: None,
            mime: "text/plain".into(),
            output_path: PathBuf::from(output),
            engine: EngineStamp::new(
                docfusion_core::ir::EngineId::new("text-passthrough"),
                "1.0.0",
            ),
            status: Status::Done,
            verified: false,
            conflict_count: 0,
            created_at: now_iso8601(),
            updated_at: now_iso8601(),
            output_hash: None,
        }
    }

    /// And a prefix that names two is refused rather than guessed at.
    ///
    /// The commands that take a document by name are the ones that take one
    /// away, so "whichever of these you meant" is not an answer to give.
    #[test]
    fn a_prefix_that_names_two_documents_is_refused() {
        let (_dir, pipeline) = setup();
        pipeline
            .manifest()
            .upsert(&record_named("aaaa1111", "docs/a.md"))
            .unwrap();
        pipeline
            .manifest()
            .upsert(&record_named("aabb2222", "docs/b.md"))
            .unwrap();

        let error = pipeline.resolve("aa").expect_err("should refuse");
        assert!(
            matches!(error, PipelineError::Ambiguous { count: 2, .. }),
            "{error}"
        );

        // Long enough to be one, and it stops being a question.
        assert_eq!(pipeline.resolve("aaaa").unwrap().id, "aaaa1111");

        // And a name that is in nobody's index is not a reason to guess.
        let error = pipeline.resolve("zz").expect_err("should not match");
        assert!(
            matches!(error, PipelineError::NoSuchDocument { .. }),
            "{error}"
        );
    }

    /// A path out of the index is checked before anything is removed.
    ///
    /// Written as a test because `starts_with(root)` does **not** catch it and
    /// looks like it does: it walks components, `..` is one, so
    /// `root.join("../../x")` starts with `root` and the guard somebody reaches
    /// for first passes.
    #[test]
    fn a_path_that_leaves_the_knowledge_base_is_refused() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "the words");
        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();

        let outside = dir.path().parent().unwrap().join("docfusion-outside.md");
        fs::write(&outside, "not ours").unwrap();

        pipeline
            .manifest()
            .relink(&wrote.id, Path::new("../docfusion-outside.md"))
            .expect("relink");

        let error = pipeline.delete(&wrote.id).expect_err("should refuse");

        assert!(matches!(error, PipelineError::Outside { .. }), "{error}");
        assert!(outside.is_file(), "it deleted a file outside the knowledge base");
        assert!(
            pipeline.manifest().find(&wrote.id).unwrap().is_some(),
            "and the record stays, since nothing happened"
        );

        fs::remove_file(&outside).ok();
    }

    #[test]
    fn ingesting_writes_markdown_with_frontmatter() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "# Title\n\nBody text");

        let outcome = pipeline.ingest(&source, Mode::Apply).unwrap();

        assert!(!outcome.reused);
        assert!(outcome.output.exists());
        let markdown = markdown_at(&outcome.output);
        assert!(markdown.starts_with("---\n"), "{markdown}");
        assert!(markdown.contains("# Title"));
        assert!(markdown.contains("Body text"));
    }

    #[test]
    fn the_output_lands_under_the_docs_directory() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "hello");

        let outcome = pipeline.ingest(&source, Mode::Apply).unwrap();

        assert_eq!(
            outcome.output,
            dir.path().join("docs").join("notes.md"),
            "output should be named after the source stem"
        );
    }

    #[test]
    fn ingesting_the_same_content_twice_reuses_the_result() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "hello");

        let first = pipeline.ingest(&source, Mode::Apply).unwrap();
        let second = pipeline.ingest(&source, Mode::Apply).unwrap();

        assert!(!first.reused);
        assert!(second.reused);
        assert_eq!(first.output, second.output);
    }

    /// Reuse must mean the file was not touched. Editing the output and then
    /// re-ingesting proves it: a rewrite would discard the edit.
    #[test]
    fn reuse_does_not_rewrite_the_output() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "hello");

        let outcome = pipeline.ingest(&source, Mode::Apply).unwrap();
        fs::write(&outcome.output, "edited by hand").unwrap();

        pipeline.ingest(&source, Mode::Apply).unwrap();

        assert_eq!(markdown_at(&outcome.output), "edited by hand");
    }

    #[test]
    fn identical_content_from_two_paths_is_processed_once() {
        let (dir, mut pipeline) = setup();
        let first = write_source(dir.path(), "a.txt", "same bytes");
        let second = write_source(dir.path(), "b.txt", "same bytes");

        let one = pipeline.ingest(&first, Mode::Apply).unwrap();
        let two = pipeline.ingest(&second, Mode::Apply).unwrap();

        assert!(!one.reused);
        assert!(two.reused);
        assert_eq!(one.id, two.id);
        assert_eq!(pipeline.manifest().document_count().unwrap(), 1);
    }

    /// A record whose output has been deleted is not a usable cache entry:
    /// reporting a hit would hand back a path to a file that is not there and
    /// skip the write that would put it back.
    #[test]
    fn a_deleted_output_is_rebuilt_rather_than_reported_as_cached() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "hello");

        let first = pipeline.ingest(&source, Mode::Apply).unwrap();
        fs::remove_file(&first.output).unwrap();

        let second = pipeline.ingest(&source, Mode::Apply).unwrap();

        assert!(!second.reused, "a missing output must not count as a hit");
        assert!(second.output.exists(), "the output should have been rewritten");
    }

    #[test]
    fn changed_content_is_processed_again() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "first");

        let one = pipeline.ingest(&source, Mode::Apply).unwrap();
        fs::write(&source, "second revision").unwrap();
        let two = pipeline.ingest(&source, Mode::Apply).unwrap();

        assert!(!two.reused);
        assert_ne!(one.id, two.id);
        assert!(markdown_at(&two.output).contains("second revision"));
    }

    /// Two documents that share a title must not share a file.
    #[test]
    fn documents_that_share_a_title_get_distinct_output_paths() {
        let (dir, mut pipeline) = setup();
        let first_dir = dir.path().join("one");
        let second_dir = dir.path().join("two");
        fs::create_dir_all(&first_dir).unwrap();
        fs::create_dir_all(&second_dir).unwrap();

        let first = write_source(&first_dir, "report.txt", "first document");
        let second = write_source(&second_dir, "report.txt", "second document");

        let one = pipeline.ingest(&first, Mode::Apply).unwrap();
        let two = pipeline.ingest(&second, Mode::Apply).unwrap();

        assert_ne!(one.output, two.output);
        assert!(markdown_at(&one.output).contains("first document"));
        assert!(markdown_at(&two.output).contains("second document"));
    }

    #[test]
    fn a_dry_run_writes_nothing() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "hello");

        let outcome = pipeline.ingest(&source, Mode::DryRun).unwrap();

        assert!(!outcome.reused, "a dry run still reports what would happen");
        assert!(!outcome.output.exists());
        assert_eq!(pipeline.manifest().document_count().unwrap(), 0);
    }

    #[test]
    fn a_dry_run_reports_a_cache_hit_without_recording_anything() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "hello");
        pipeline.ingest(&source, Mode::Apply).unwrap();

        let outcome = pipeline.ingest(&source, Mode::DryRun).unwrap();

        assert!(outcome.reused);
        assert_eq!(pipeline.manifest().document_count().unwrap(), 1);
    }

    #[test]
    fn reopening_a_knowledge_base_reuses_its_index() {
        let dir = tempfile::tempdir().unwrap();
        let source = write_source(dir.path(), "notes.txt", "hello");

        {
            let mut pipeline = Pipeline::open(dir.path(), router()).unwrap();
            pipeline.ingest(&source, Mode::Apply).unwrap();
        }

        let mut reopened = Pipeline::open(dir.path(), router()).unwrap();
        let outcome = reopened.ingest(&source, Mode::Apply).unwrap();

        assert!(outcome.reused, "the index should survive a restart");
    }

    /// The same engine at a higher version, to prove an upgrade invalidates
    /// previous output rather than serving it silently.
    struct UpgradedText(TextPassthrough);

    impl Extractor for UpgradedText {
        fn id(&self) -> docfusion_core::ir::EngineId {
            self.0.id()
        }

        fn version(&self) -> &str {
            "2.0.0"
        }

        fn accepts(&self, probe: &InputProbe) -> docfusion_core::extract::Fit {
            self.0.accepts(probe)
        }

        fn extract(
            &self,
            input: &docfusion_core::extract::Input<'_>,
        ) -> Result<Document, ExtractError> {
            self.0.extract(input)
        }
    }

    #[test]
    fn upgrading_an_engine_invalidates_previous_results() {
        let dir = tempfile::tempdir().unwrap();
        let source = write_source(dir.path(), "notes.txt", "hello");

        {
            let mut pipeline = Pipeline::open(dir.path(), router()).unwrap();
            assert!(!pipeline.ingest(&source, Mode::Apply).unwrap().reused);
        }

        let upgraded_router = Router::new(vec![Box::new(UpgradedText(TextPassthrough))]);
        let mut upgraded = Pipeline::open(dir.path(), upgraded_router).unwrap();

        assert!(
            !upgraded.ingest(&source, Mode::Apply).unwrap().reused,
            "a new engine version must not reuse the old output"
        );
    }

    #[test]
    fn an_unsupported_input_reports_why() {
        let (dir, mut pipeline) = setup();
        let source = dir.path().join("scan.png");
        fs::write(&source, [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]).unwrap();

        let error = pipeline.ingest(&source, Mode::Apply).unwrap_err();

        assert!(
            matches!(
                error,
                PipelineError::Extract(ExtractError::Unsupported { .. })
            ),
            "got {error}"
        );
        assert!(error.to_string().contains("OCR"), "{error}");
    }

    #[test]
    fn the_provenance_engine_is_recorded() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "hello");

        pipeline.ingest(&source, Mode::Apply).unwrap();

        let record = pipeline.manifest().all().unwrap().into_iter().next().unwrap();
        assert_eq!(record.engine.id.as_str(), TextPassthrough.id().as_str());
        assert_eq!(record.engine.version, TextPassthrough.version());
    }
}
