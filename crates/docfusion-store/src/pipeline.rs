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
