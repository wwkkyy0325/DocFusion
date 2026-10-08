//! Deduplication behaviour, exercised through the public store API.
//!
//! The real pipeline does not exist yet, so this file drives the store the way
//! the pipeline will: hash the input, consult the manifest, and skip extraction
//! on a hit. When the pipeline lands, its behaviour must match this.

use std::path::{Path, PathBuf};

use docfusion_core::ir::EngineStamp;
use docfusion_store::atomic::write_atomic;
use docfusion_store::hash::hash_file;
use docfusion_store::manifest::{DocumentRecord, Manifest, Status, now_iso8601};

const ENGINE_VERSION: &str = "1.0.0";

fn engine() -> EngineStamp {
    EngineStamp::new("pdfium", ENGINE_VERSION)
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Extraction ran.
    Extracted,
    /// A previous result was reused.
    Cached,
}

struct Pipeline {
    manifest: Manifest,
    root: PathBuf,
    extractions: u32,
}

impl Pipeline {
    fn new(root: &Path) -> Self {
        std::fs::create_dir_all(root.join("docs")).unwrap();
        Self {
            manifest: Manifest::open_in_memory().unwrap(),
            root: root.to_path_buf(),
            extractions: 0,
        }
    }

    fn output_for(&self, source: &Path) -> PathBuf {
        let stem = source
            .file_stem()
            .expect("source has a file name")
            .to_string_lossy();
        self.root.join("docs").join(format!("{stem}.md"))
    }

    fn ingest(&mut self, source: &Path) -> Outcome {
        let content_hash = hash_file(source).unwrap();

        if let Some(_existing) = self.manifest.lookup(&content_hash, &engine()).unwrap() {
            self.manifest.add_alias(&content_hash, source).unwrap();
            return Outcome::Cached;
        }

        // Stands in for extraction and rendering.
        self.extractions += 1;
        let output = self.output_for(source);
        write_atomic(&output, b"# Extracted\n").unwrap();

        let now = now_iso8601();
        let metadata = std::fs::metadata(source).unwrap();
        self.manifest
            .upsert(&DocumentRecord {
                id: content_hash.clone(),
                source_path: source.to_path_buf(),
                source_size: metadata.len(),
                source_mtime: None,
                mime: "application/pdf".into(),
                output_path: output,
                engine: engine(),
                status: Status::Done,
                verified: false,
                conflict_count: 0,
                created_at: now.clone(),
                updated_at: now,
            })
            .unwrap();
        self.manifest.add_alias(&content_hash, source).unwrap();

        Outcome::Extracted
    }
}

#[test]
fn the_same_content_from_two_paths_is_processed_once() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("report.pdf");
    let second = dir.path().join("report-copy.pdf");
    std::fs::write(&first, b"identical bytes").unwrap();
    std::fs::write(&second, b"identical bytes").unwrap();

    let mut pipeline = Pipeline::new(dir.path());

    assert_eq!(pipeline.ingest(&first), Outcome::Extracted);
    assert_eq!(
        pipeline.ingest(&second),
        Outcome::Cached,
        "identical content must not be extracted twice"
    );

    assert_eq!(pipeline.extractions, 1);
    assert_eq!(pipeline.manifest.document_count().unwrap(), 1);

    let id = hash_file(&first).unwrap();
    let aliases = pipeline.manifest.aliases_of(&id).unwrap();
    assert_eq!(aliases.len(), 2, "both paths should be recorded: {aliases:?}");
}

#[test]
fn reprocessing_the_same_path_is_a_cache_hit() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("report.pdf");
    std::fs::write(&source, b"bytes").unwrap();

    let mut pipeline = Pipeline::new(dir.path());

    assert_eq!(pipeline.ingest(&source), Outcome::Extracted);
    assert_eq!(pipeline.ingest(&source), Outcome::Cached);
    assert_eq!(pipeline.extractions, 1);
}

#[test]
fn changed_content_is_extracted_again() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("report.pdf");
    std::fs::write(&source, b"first revision").unwrap();

    let mut pipeline = Pipeline::new(dir.path());
    assert_eq!(pipeline.ingest(&source), Outcome::Extracted);

    std::fs::write(&source, b"second revision").unwrap();
    assert_eq!(
        pipeline.ingest(&source),
        Outcome::Extracted,
        "changed content must not be served from cache"
    );
    assert_eq!(pipeline.extractions, 2);
}

#[test]
fn an_engine_upgrade_invalidates_everything() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("report.pdf");
    std::fs::write(&source, b"bytes").unwrap();

    let manifest = Manifest::open_in_memory().unwrap();
    let id = hash_file(&source).unwrap();
    let now = now_iso8601();

    manifest
        .upsert(&DocumentRecord {
            id: id.clone(),
            source_path: source.clone(),
            source_size: 5,
            source_mtime: None,
            mime: "application/pdf".into(),
            output_path: dir.path().join("docs/report.md"),
            engine: engine(),
            status: Status::Done,
            verified: false,
            conflict_count: 0,
            created_at: now.clone(),
            updated_at: now,
        })
        .unwrap();

    assert!(manifest.lookup(&id, &engine()).unwrap().is_some());
    assert!(
        manifest
            .lookup(&id, &EngineStamp::new("pdfium", "2.0.0"))
            .unwrap()
            .is_none(),
        "a new engine version must force reprocessing"
    );
}
