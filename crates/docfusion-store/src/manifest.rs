//! The sidecar index.
//!
//! The Markdown file tree is the external contract; this database is a
//! convenience layer that remembers what has already been processed. It can
//! always be rebuilt from the tree with `reconcile`, so it is never the only
//! copy of anything.

use std::path::{Path, PathBuf};

use docfusion_core::ir::EngineStamp;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// Bumped whenever the schema below changes, tracked in `PRAGMA user_version`.
const SCHEMA_VERSION: i32 = 1;

/// The columns `row_to_record` expects, in the order it reads them.
const COLUMNS: &str = "id, source_path, source_size, source_mtime, mime, output_path, \
                       engine_id, engine_version, model, prompt_version, \
                       status, verified, conflict_count, created_at, updated_at";

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS documents (
  id             TEXT PRIMARY KEY,
  source_path    TEXT NOT NULL,
  source_size    INTEGER NOT NULL,
  source_mtime   INTEGER,
  mime           TEXT NOT NULL,
  output_path    TEXT NOT NULL,
  engine_id      TEXT NOT NULL,
  engine_version TEXT NOT NULL,
  model          TEXT,
  prompt_version TEXT,
  status         TEXT NOT NULL,
  verified       INTEGER NOT NULL,
  conflict_count INTEGER NOT NULL,
  created_at     TEXT NOT NULL,
  updated_at     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS aliases (
  document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  path        TEXT NOT NULL,
  PRIMARY KEY (document_id, path)
);

CREATE TABLE IF NOT EXISTS runs (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  document_id TEXT NOT NULL,
  started_at  TEXT NOT NULL,
  finished_at TEXT,
  engines     TEXT NOT NULL,
  outcome     TEXT NOT NULL,
  error       TEXT
);

CREATE INDEX IF NOT EXISTS runs_by_document ON runs(document_id);
"#;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("unrecognised document status `{0}` in the manifest")]
    UnknownStatus(String),
}

/// How far a document got through the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Done,
    Partial,
    Failed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Partial => "partial",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "done" => Ok(Self::Done),
            "partial" => Ok(Self::Partial),
            "failed" => Ok(Self::Failed),
            other => Err(StoreError::UnknownStatus(other.to_owned())),
        }
    }
}

/// A processed document as recorded in the manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentRecord {
    /// BLAKE3 of the source bytes, as lowercase hex.
    pub id: String,
    pub source_path: PathBuf,
    pub source_size: u64,
    pub source_mtime: Option<i64>,
    pub mime: String,
    pub output_path: PathBuf,
    pub engine: EngineStamp,
    pub status: Status,
    pub verified: bool,
    pub conflict_count: u32,
    pub created_at: String,
    pub updated_at: String,
}

/// Current UTC time as RFC 3339.
///
/// Callers stamp records with this rather than the manifest inventing times,
/// so a batch run can record when it actually did the work.
pub fn now_iso8601() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC 3339 formatting cannot fail for a valid OffsetDateTime")
}

pub struct Manifest {
    connection: Connection,
}

impl Manifest {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Self::prepare(Connection::open(path)?)
    }

    /// An ephemeral manifest, for tests and dry runs.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(connection: Connection) -> Result<Self, StoreError> {
        // Needed for the alias foreign key to actually cascade.
        connection.pragma_update(None, "foreign_keys", true)?;
        Self::migrate(&connection)?;
        Ok(Self { connection })
    }

    fn migrate(connection: &Connection) -> Result<(), StoreError> {
        let existing: i32 =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;

        if existing < 1 {
            connection.execute_batch(SCHEMA_V1)?;
        }

        if existing != SCHEMA_VERSION {
            connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }

        Ok(())
    }

    /// Returns the record for this content, but only if the same engine
    /// configuration produced it.
    ///
    /// A hit means the previous output is still valid and extraction can be
    /// skipped. Any difference in the engine stamp is a miss, because the
    /// output would differ.
    pub fn lookup(
        &self,
        content_hash: &str,
        engine: &EngineStamp,
    ) -> Result<Option<DocumentRecord>, StoreError> {
        let record = self
            .connection
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM documents
                     WHERE id = ?1
                       AND engine_id = ?2
                       AND engine_version = ?3
                       AND model IS ?4
                       AND prompt_version IS ?5"
                ),
                params![
                    content_hash,
                    engine.id.as_str(),
                    engine.version,
                    engine.model,
                    engine.prompt_version,
                ],
                row_to_record,
            )
            .optional()?;

        Ok(record)
    }

    /// Every document, most recently updated first.
    pub fn all(&self) -> Result<Vec<DocumentRecord>, StoreError> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {COLUMNS} FROM documents ORDER BY updated_at DESC, id"
        ))?;
        let rows = statement.query_map([], row_to_record)?;

        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    /// The document currently written to an output path, if any.
    ///
    /// Used to detect two different documents that would claim the same file
    /// name — silently overwriting one with the other would lose a document
    /// without saying so.
    pub fn document_at_output(&self, output_path: &Path) -> Result<Option<String>, StoreError> {
        let owner = self
            .connection
            .query_row(
                "SELECT id FROM documents WHERE output_path = ?1",
                params![path_to_string(output_path)],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        Ok(owner)
    }

    /// Inserts a record, or replaces the existing one for the same content.
    ///
    /// `created_at` is preserved across replacements; `updated_at` is taken
    /// from the supplied record.
    pub fn upsert(&self, record: &DocumentRecord) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO documents (
                 id, source_path, source_size, source_mtime, mime, output_path,
                 engine_id, engine_version, model, prompt_version,
                 status, verified, conflict_count, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(id) DO UPDATE SET
                 source_path    = excluded.source_path,
                 source_size    = excluded.source_size,
                 source_mtime   = excluded.source_mtime,
                 mime           = excluded.mime,
                 output_path    = excluded.output_path,
                 engine_id      = excluded.engine_id,
                 engine_version = excluded.engine_version,
                 model          = excluded.model,
                 prompt_version = excluded.prompt_version,
                 status         = excluded.status,
                 verified       = excluded.verified,
                 conflict_count = excluded.conflict_count,
                 updated_at     = excluded.updated_at",
            params![
                record.id,
                path_to_string(&record.source_path),
                record.source_size as i64,
                record.source_mtime,
                record.mime,
                path_to_string(&record.output_path),
                record.engine.id.as_str(),
                record.engine.version,
                record.engine.model,
                record.engine.prompt_version,
                record.status.as_str(),
                record.verified as i64,
                record.conflict_count as i64,
                record.created_at,
                record.updated_at,
            ],
        )?;

        Ok(())
    }

    /// Records that the same content was also seen at `path`.
    pub fn add_alias(&self, document_id: &str, path: &Path) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT OR IGNORE INTO aliases (document_id, path) VALUES (?1, ?2)",
            params![document_id, path_to_string(path)],
        )?;
        Ok(())
    }

    pub fn aliases_of(&self, document_id: &str) -> Result<Vec<PathBuf>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT path FROM aliases WHERE document_id = ?1 ORDER BY path")?;
        let rows = statement.query_map(params![document_id], |row| row.get::<_, String>(0))?;

        let mut paths = Vec::new();
        for row in rows {
            paths.push(PathBuf::from(row?));
        }
        Ok(paths)
    }

    pub fn document_count(&self) -> Result<u64, StoreError> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM documents", [], |row| row.get(0))?;
        Ok(count as u64)
    }
}

fn row_to_record(row: &Row<'_>) -> rusqlite::Result<DocumentRecord> {
    let status_text: String = row.get(10)?;
    let status = Status::parse(&status_text).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(err))
    })?;

    Ok(DocumentRecord {
        id: row.get(0)?,
        source_path: PathBuf::from(row.get::<_, String>(1)?),
        source_size: row.get::<_, i64>(2)? as u64,
        source_mtime: row.get(3)?,
        mime: row.get(4)?,
        output_path: PathBuf::from(row.get::<_, String>(5)?),
        engine: EngineStamp {
            id: row.get::<_, String>(6)?.into(),
            version: row.get(7)?,
            model: row.get(8)?,
            prompt_version: row.get(9)?,
        },
        status,
        verified: row.get::<_, i64>(11)? != 0,
        conflict_count: row.get::<_, i64>(12)? as u32,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

/// Paths are stored as text. On Windows a path is UTF-16 at the OS boundary, so
/// the only lossy case is a lone surrogate — which cannot name a real file.
fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> EngineStamp {
        EngineStamp::new("pdfium", "1.0.0")
    }

    fn record(id: &str, hash: &str) -> DocumentRecord {
        let now = now_iso8601();
        DocumentRecord {
            id: id.to_owned(),
            source_path: PathBuf::from("in").join(hash),
            source_size: 1024,
            source_mtime: Some(1_700_000_000),
            mime: "application/pdf".into(),
            output_path: PathBuf::from("docs").join(format!("{hash}.md")),
            engine: engine(),
            status: Status::Done,
            verified: false,
            conflict_count: 0,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    #[test]
    fn lookup_misses_on_unknown_content() {
        let manifest = Manifest::open_in_memory().unwrap();
        assert!(manifest.lookup("unknown", &engine()).unwrap().is_none());
    }

    #[test]
    fn lookup_hits_on_matching_content_and_engine() {
        let manifest = Manifest::open_in_memory().unwrap();
        manifest.upsert(&record("hash-a", "a")).unwrap();

        let found = manifest.lookup("hash-a", &engine()).unwrap();
        assert_eq!(found.expect("hit").id, "hash-a");
    }

    #[test]
    fn lookup_misses_when_the_engine_version_changes() {
        let manifest = Manifest::open_in_memory().unwrap();
        manifest.upsert(&record("hash-a", "a")).unwrap();

        let newer = EngineStamp::new("pdfium", "2.0.0");
        assert!(manifest.lookup("hash-a", &newer).unwrap().is_none());
    }

    #[test]
    fn lookup_misses_when_the_prompt_version_changes() {
        let manifest = Manifest::open_in_memory().unwrap();
        let prompted = engine().with_model("glm-ocr").with_prompt_version("v1");
        let mut stored = record("hash-a", "a");
        stored.engine = prompted.clone();
        manifest.upsert(&stored).unwrap();

        assert!(manifest.lookup("hash-a", &prompted).unwrap().is_some());

        let edited = prompted.with_prompt_version("v2");
        assert!(
            manifest.lookup("hash-a", &edited).unwrap().is_none(),
            "editing a prompt must invalidate cached output"
        );
    }

    #[test]
    fn lookup_misses_when_a_model_is_added() {
        let manifest = Manifest::open_in_memory().unwrap();
        manifest.upsert(&record("hash-a", "a")).unwrap();

        // The stored record has no model at all; a model-bearing stamp is a
        // different configuration, so NULL must not compare equal to a value.
        assert!(
            manifest
                .lookup("hash-a", &engine().with_model("glm-ocr"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn replacing_preserves_created_at() {
        let manifest = Manifest::open_in_memory().unwrap();
        let mut first = record("hash-a", "a");
        first.created_at = "2020-01-01T00:00:00Z".into();
        manifest.upsert(&first).unwrap();

        let mut second = record("hash-a", "a");
        second.created_at = "2026-10-08T00:00:00Z".into();
        second.status = Status::Partial;
        manifest.upsert(&second).unwrap();

        let stored = manifest.lookup("hash-a", &engine()).unwrap().unwrap();
        assert_eq!(stored.created_at, "2020-01-01T00:00:00Z");
        assert_eq!(stored.status, Status::Partial);
    }

    #[test]
    fn aliases_accumulate_without_duplicates() {
        let manifest = Manifest::open_in_memory().unwrap();
        manifest.upsert(&record("hash-a", "a")).unwrap();

        manifest.add_alias("hash-a", Path::new("one.pdf")).unwrap();
        manifest.add_alias("hash-a", Path::new("two.pdf")).unwrap();
        manifest.add_alias("hash-a", Path::new("one.pdf")).unwrap();

        let aliases = manifest.aliases_of("hash-a").unwrap();
        assert_eq!(
            aliases,
            vec![PathBuf::from("one.pdf"), PathBuf::from("two.pdf")]
        );
    }

    #[test]
    fn reopening_an_existing_database_keeps_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.db");

        {
            let manifest = Manifest::open(&path).unwrap();
            manifest.upsert(&record("hash-a", "a")).unwrap();
        }

        let reopened = Manifest::open(&path).unwrap();
        assert_eq!(reopened.document_count().unwrap(), 1);
        assert!(reopened.lookup("hash-a", &engine()).unwrap().is_some());
    }

    #[test]
    fn a_corrupt_status_is_reported_rather_than_guessed() {
        let manifest = Manifest::open_in_memory().unwrap();
        manifest.upsert(&record("hash-a", "a")).unwrap();
        manifest
            .connection
            .execute("UPDATE documents SET status = 'wat'", [])
            .unwrap();

        assert!(manifest.lookup("hash-a", &engine()).is_err());
    }
}
