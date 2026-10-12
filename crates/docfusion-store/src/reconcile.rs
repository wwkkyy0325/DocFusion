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
use crate::pipeline::{DOCS_DIR, Pipeline, PipelineError, SOURCES_DIR, STATE_DIR};

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

    /// Records whose copy of the source is not in the state directory, and
    /// whose source is still where the index says it is.
    ///
    /// Two ways to get here: a record written before copies existed, which is
    /// every record in every knowledge base made before this, and a copy
    /// somebody deleted. Both are fixed the same way and neither costs a read —
    /// the bytes are copied, not interpreted.
    pub recoverable: Vec<DocumentRecord>,

    /// Copies in the state directory that no record points at.
    ///
    /// What `forget` leaves behind, by decision: taking a row out of the index
    /// stays a pure index operation, and reclaiming the room is a separate act.
    /// Reported and left alone, exactly like untracked Markdown — a file the
    /// index claims nothing about is not a file the index gets to delete, and
    /// a person may have put it there.
    pub orphan_copies: Vec<PathBuf>,
}

impl Drift {
    pub fn is_empty(&self) -> bool {
        self.untracked.is_empty()
            && self.relinked.is_empty()
            && self.missing.is_empty()
            && self.recoverable.is_empty()
            && self.orphan_copies.is_empty()
    }

    /// Whether the index needs repairing.
    ///
    /// Untracked files and orphaned copies do not count: the index claims
    /// nothing about them, so there is nothing for it to be wrong about.
    pub fn needs_repair(&self) -> bool {
        !self.relinked.is_empty() || !self.missing.is_empty() || !self.recoverable.is_empty()
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Repair {
    pub relinked: usize,
    pub forgotten: usize,
    /// Copies of sources put back where records say they should be.
    pub recovered: usize,
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

    for record in &records {
        if present.contains(record.output_path.as_path()) {
            continue;
        }

        // A record written before output hashes existed cannot be matched, and
        // guessing would be worse than reporting.
        let Some(wanted) = record.output_hash.as_deref() else {
            drift.missing.push(record.clone());
            continue;
        };

        match orphans.iter().position(|(_, hash)| hash == wanted) {
            Some(index) => {
                let (path, _) = orphans.remove(index);
                drift.relinked.push((record.clone(), path));
            }
            None => drift.missing.push(record.clone()),
        }
    }

    // A record whose copy is missing but whose source is still where the index
    // says it is can be put back without reading anything. One whose source has
    // gone cannot, and is not reported here — its output is intact and there is
    // nothing to do about the rest.
    for record in &records {
        if record
            .source_copy
            .as_ref()
            .is_some_and(|copy| root.join(copy).is_file())
        {
            continue;
        }

        if source_is_reachable(pipeline, record) {
            drift.recoverable.push(record.clone());
        }
    }

    drift.untracked = orphans.into_iter().map(|(path, _)| path).collect();
    drift.orphan_copies = orphaned_copies(pipeline, &records);
    Ok(drift)
}

/// Whether a record's source can still be found, by any of its names.
///
/// Asked without reading, because this runs over every record in the knowledge
/// base: whether the bytes are the right ones is [`Pipeline::keep_source_of`]'s
/// question, and it asks it by reading them.
fn source_is_reachable(pipeline: &Pipeline, record: &DocumentRecord) -> bool {
    if record.source_path.is_file() {
        return true;
    }

    pipeline
        .manifest()
        .aliases_of(&record.id)
        .is_ok_and(|aliases| aliases.iter().any(|path| path.is_file()))
}

/// Copies in the state directory that no record points at.
///
/// Compared by file name rather than by path, because the name is the document
/// id and the extension: a record says `sources/<id>.pdf` and the file on disk
/// is that same name, so the two meet without either having to be resolved.
fn orphaned_copies(pipeline: &Pipeline, records: &[DocumentRecord]) -> Vec<PathBuf> {
    let claimed: HashSet<&std::ffi::OsStr> = records
        .iter()
        .filter_map(|record| record.source_copy.as_ref())
        .filter_map(|copy| copy.file_name())
        .collect();

    let Ok(entries) = fs::read_dir(pipeline.root().join(STATE_DIR).join(SOURCES_DIR)) else {
        return Vec::new();
    };

    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| !claimed.contains(path.file_name().unwrap_or_default()))
        .collect();

    found.sort();
    found
}

pub fn repair(pipeline: &Pipeline, drift: &Drift) -> Result<Repair, PipelineError> {
    let manifest = pipeline.manifest();

    for (record, path) in &drift.relinked {
        manifest.relink(&record.id, path)?;
    }
    for record in &drift.missing {
        manifest.forget(&record.id)?;
    }

    let mut recovered = 0;
    for record in &drift.recoverable {
        let copy = pipeline.keep_source_of(record)?;
        let mut kept = record.clone();
        kept.source_copy = Some(copy);
        manifest.upsert(&kept)?;
        recovered += 1;
    }

    Ok(Repair {
        relinked: drift.relinked.len(),
        forgotten: drift.missing.len(),
        recovered,
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

    /// A record with no copy of its source is something `--fix` can put right,
    /// and nothing has to be read to do it.
    #[test]
    fn a_record_with_no_copy_can_be_given_one() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = ingest_one(dir.path(), "notes.txt", "hello");

        // As a knowledge base made before copies existed looks: the column is
        // null for every record in it.
        let record = pipeline.manifest().all().unwrap().remove(0);
        pipeline
            .manifest()
            .upsert(&DocumentRecord {
                source_copy: None,
                edited_at: None,
                ..record
            })
            .unwrap();

        let drift = scan(&pipeline).unwrap();
        assert_eq!(drift.recoverable.len(), 1, "{drift:?}");
        assert!(drift.needs_repair());

        let repaired = repair(&pipeline, &drift).unwrap();
        assert_eq!(repaired.recovered, 1);

        let after = scan(&pipeline).unwrap();
        assert!(after.recoverable.is_empty(), "{after:?}");
    }

    /// And one whose source has gone cannot be, which is not drift — there is
    /// nothing to be done about it, and the Markdown is intact.
    #[test]
    fn a_record_whose_source_is_gone_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = ingest_one(dir.path(), "notes.txt", "hello");

        let record = pipeline.manifest().all().unwrap().remove(0);
        pipeline
            .manifest()
            .upsert(&DocumentRecord {
                source_copy: None,
                edited_at: None,
                ..record
            })
            .unwrap();
        fs::remove_file(dir.path().join("notes.txt")).unwrap();

        let drift = scan(&pipeline).unwrap();
        assert!(drift.recoverable.is_empty(), "{drift:?}");
        assert!(!drift.needs_repair());
    }

    /// A copy nothing points at is reported and left where it is.
    ///
    /// This is what `forget` leaves behind, by decision: taking a row out of
    /// the index is a pure index operation, and reclaiming the room is a
    /// separate act somebody has to mean. A file the index claims nothing about
    /// is not a file the index gets to delete — the same rule untracked
    /// Markdown lives under.
    #[test]
    fn a_copy_nothing_points_at_is_reported_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let pipeline = ingest_one(dir.path(), "notes.txt", "hello");

        let record = pipeline.manifest().all().unwrap().remove(0);
        let copy = pipeline.root().join(record.source_copy.clone().expect("a copy"));

        pipeline.forget(&record.id).unwrap();

        let drift = scan(&pipeline).unwrap();
        assert_eq!(drift.orphan_copies, vec![copy.clone()], "{drift:?}");

        // Reported, and not a reason to repair anything: nothing is wrong.
        assert!(!drift.needs_repair());
        assert!(copy.is_file(), "reconcile removed a file it claims nothing about");
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
