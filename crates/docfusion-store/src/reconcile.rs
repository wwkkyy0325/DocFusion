//! Repairing the index against the file tree.
//!
//! The file tree is authoritative: a user may rename, move, edit or delete
//! anything in it, and the manifest is a cache that follows along. Drift is a
//! designed-for condition, not an error state.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::hash::hash_file;
use crate::manifest::{DocumentRecord, StoreError};
use crate::pipeline::{DOCS_DIR, Pipeline};

/// Where the index and the tree disagree.
#[derive(Debug, Default, PartialEq)]
pub struct Drift {
    /// Files in the document tree that no record claims.
    ///
    /// Not necessarily a problem: a person may keep notes of their own
    /// alongside the extracted documents, so these are reported and left alone.
    pub untracked: Vec<PathBuf>,

    /// Records whose output was found under a different name, paired with where
    /// it is now.
    pub relinked: Vec<(DocumentRecord, PathBuf)>,

    /// Records whose output could not be found at all.
    pub missing: Vec<DocumentRecord>,
}

impl Drift {
    pub fn is_empty(&self) -> bool {
        self.untracked.is_empty() && self.relinked.is_empty() && self.missing.is_empty()
    }

    /// Whether the index needs repairing.
    ///
    /// Untracked files do not count: the index claims nothing about them, so
    /// there is nothing for it to be wrong about.
    pub fn needs_repair(&self) -> bool {
        !self.relinked.is_empty() || !self.missing.is_empty()
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Repair {
    pub relinked: usize,
    pub forgotten: usize,
}

pub fn scan(pipeline: &Pipeline) -> Result<Drift, StoreError> {
    let root = pipeline.root();
    let docs = root.join(DOCS_DIR);
    let files = markdown_files(&docs);
    let records = pipeline.manifest().all()?;

    let claimed: HashSet<&Path> = records
        .iter()
        .map(|record| record.output_path.as_path())
        .collect();
    let present: HashSet<&Path> = files.iter().map(PathBuf::as_path).collect();

    let mut orphans: Vec<(PathBuf, String)> = files
        .iter()
        .filter(|file| !claimed.contains(file.as_path()))
        // `file` is relative to the root, so it is resolved against the root —
        // joining it onto the document directory again would look for
        // `docs/docs/...` and find nothing.
        .filter_map(|file| {
            hash_file(&root.join(file))
                .ok()
                .map(|hash| (file.clone(), hash))
        })
        .collect();

    let mut drift = Drift::default();

    for record in records {
        if present.contains(record.output_path.as_path()) {
            continue;
        }

        // A record written before output hashes existed cannot be matched, and
        // guessing would be worse than reporting.
        let Some(wanted) = record.output_hash.as_deref() else {
            drift.missing.push(record);
            continue;
        };

        match orphans.iter().position(|(_, hash)| hash == wanted) {
            Some(index) => {
                let (path, _) = orphans.remove(index);
                drift.relinked.push((record, path));
            }
            None => drift.missing.push(record),
        }
    }

    drift.untracked = orphans.into_iter().map(|(path, _)| path).collect();
    Ok(drift)
}

pub fn repair(pipeline: &Pipeline, drift: &Drift) -> Result<Repair, StoreError> {
    let manifest = pipeline.manifest();

    for (record, path) in &drift.relinked {
        manifest.relink(&record.id, path)?;
    }
    for record in &drift.missing {
        manifest.forget(&record.id)?;
    }

    Ok(Repair {
        relinked: drift.relinked.len(),
        forgotten: drift.missing.len(),
    })
}

/// Every Markdown file under the document tree.
///
/// Paths are returned relative to the knowledge **root** — `docs/notes.md`,
/// not `notes.md` — because that is the frame the manifest records output paths
/// in, and comparing paths from two frames silently matches nothing.
fn markdown_files(docs: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(docs, docs, &mut found);
    found.sort();
    found
        .into_iter()
        .map(|relative| Path::new(DOCS_DIR).join(relative))
        .collect()
}

fn walk(root: &Path, directory: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };

    for entry in entries.flatten() {
        // The entry's own type rather than `Path::is_dir`, which follows links
        // and would spin forever on a loop.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();

        if kind.is_dir() {
            walk(root, &path, found);
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
            && let Ok(relative) = path.strip_prefix(root)
        {
            found.push(relative.to_path_buf());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::Mode;
    use docfusion_core::extract::{Router, text::TextPassthrough};

    fn router() -> Router {
        Router::new(vec![Box::new(TextPassthrough)])
    }

    fn ingest_one(dir: &Path, name: &str, body: &str) -> Pipeline {
        let mut pipeline = Pipeline::open(dir, router()).unwrap();
        let source = dir.join(name);
        fs::write(&source, body).unwrap();
        pipeline.ingest(&source, Mode::Apply).unwrap();
        pipeline
    }

    fn output_of(pipeline: &Pipeline) -> PathBuf {
        pipeline.root().join(DOCS_DIR).join("notes.md")
    }

    /// An output path in the frame the manifest records it in.
    fn recorded(parts: &[&str]) -> PathBuf {
        let mut path = PathBuf::from(DOCS_DIR);
        for part in parts {
            path.push(part);
        }
        path
    }

    #[test]
    fn a_freshly_ingested_tree_has_no_drift() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = ingest_one(dir.path(), "notes.txt", "hello");

        let drift = scan(&pipeline).unwrap();

        assert!(drift.is_empty(), "{drift:?}");
        assert!(!drift.needs_repair());
    }

    /// The case the output hash exists for: renaming a file breaks the path
    /// link even though nothing about the document changed.
    #[test]
    fn a_moved_file_is_matched_by_its_content() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = ingest_one(dir.path(), "notes.txt", "hello");

        let moved = pipeline.root().join(DOCS_DIR).join("renamed.md");
        fs::rename(output_of(&pipeline), &moved).unwrap();

        let drift = scan(&pipeline).unwrap();

        assert_eq!(drift.relinked.len(), 1);
        assert_eq!(drift.relinked[0].1, recorded(&["renamed.md"]));
        assert!(drift.missing.is_empty());
        assert!(drift.untracked.is_empty());
    }

    #[test]
    fn repairing_a_moved_file_updates_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = ingest_one(dir.path(), "notes.txt", "hello");
        fs::rename(output_of(&pipeline), pipeline.root().join(DOCS_DIR).join("renamed.md"))
            .unwrap();

        let drift = scan(&pipeline).unwrap();
        let repaired = repair(&pipeline, &drift).unwrap();

        assert_eq!(repaired.relinked, 1);
        assert!(scan(&pipeline).unwrap().is_empty(), "repair should settle it");

        let record = pipeline.manifest().all().unwrap().into_iter().next().unwrap();
        assert_eq!(record.output_path, recorded(&["renamed.md"]));
    }

    #[test]
    fn a_deleted_output_is_reported_and_then_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = ingest_one(dir.path(), "notes.txt", "hello");
        fs::remove_file(output_of(&pipeline)).unwrap();

        let drift = scan(&pipeline).unwrap();
        assert_eq!(drift.missing.len(), 1);
        assert!(drift.needs_repair());

        let repaired = repair(&pipeline, &drift).unwrap();
        assert_eq!(repaired.forgotten, 1);
        assert_eq!(pipeline.manifest().document_count().unwrap(), 0);
    }

    /// Forgetting the record means the source is processed again, which is how
    /// a deleted output comes back.
    #[test]
    fn a_forgotten_document_is_processed_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut pipeline = ingest_one(dir.path(), "notes.txt", "hello");
        fs::remove_file(output_of(&pipeline)).unwrap();
        let drift = scan(&pipeline).unwrap();
        repair(&pipeline, &drift).unwrap();

        let source = dir.path().join("notes.txt");
        let outcome = pipeline.ingest(&source, Mode::Apply).unwrap();

        assert!(!outcome.reused);
        assert!(outcome.output.exists());
    }

    /// A person may keep their own notes beside the extracted documents, so an
    /// unclaimed file is reported rather than adopted or deleted.
    #[test]
    fn an_unclaimed_file_is_reported_but_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = ingest_one(dir.path(), "notes.txt", "hello");
        let mine = pipeline.root().join(DOCS_DIR).join("my-own-notes.md");
        fs::write(&mine, "written by hand").unwrap();

        let drift = scan(&pipeline).unwrap();

        assert_eq!(drift.untracked, vec![recorded(&["my-own-notes.md"])]);
        assert!(!drift.needs_repair(), "the index claims nothing about it");

        repair(&pipeline, &drift).unwrap();
        assert!(mine.exists(), "repair must not touch a file it does not own");
    }

    #[test]
    fn files_in_subdirectories_are_found() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = ingest_one(dir.path(), "notes.txt", "hello");

        let nested = pipeline.root().join(DOCS_DIR).join("archive");
        fs::create_dir_all(&nested).unwrap();
        fs::rename(output_of(&pipeline), nested.join("notes.md")).unwrap();

        let drift = scan(&pipeline).unwrap();

        assert_eq!(drift.relinked[0].1, recorded(&["archive", "notes.md"]));
    }

    #[test]
    fn a_record_without_an_output_hash_is_reported_rather_than_guessed_at() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = ingest_one(dir.path(), "notes.txt", "hello");
        let moved = pipeline.root().join(DOCS_DIR).join("renamed.md");
        fs::rename(output_of(&pipeline), &moved).unwrap();

        // Simulate a record written before the column existed.
        let mut record = pipeline.manifest().all().unwrap().into_iter().next().unwrap();
        record.output_hash = None;
        pipeline.manifest().upsert(&record).unwrap();

        let drift = scan(&pipeline).unwrap();

        assert_eq!(drift.missing.len(), 1);
        assert!(drift.relinked.is_empty());
        assert_eq!(drift.untracked, vec![recorded(&["renamed.md"])]);
    }

    #[test]
    fn a_hand_edited_output_is_still_tracked_where_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = ingest_one(dir.path(), "notes.txt", "hello");
        fs::write(output_of(&pipeline), "edited by hand").unwrap();

        let drift = scan(&pipeline).unwrap();

        assert!(drift.is_empty(), "{drift:?}");
    }
}
