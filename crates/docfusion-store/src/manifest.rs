//! The sidecar index.
//!
//! The Markdown file tree is the external contract; this database is a
//! convenience layer that remembers what has already been processed. It can
//! always be rebuilt from the tree with `reconcile`, so it is never the only
//! copy of anything.

use std::path::{Path, PathBuf};

use docfusion_core::ir::EngineStamp;
use docfusion_core::text;
use rusqlite::{Connection, OptionalExtension, Row, params};

/// Bumped whenever the schema below changes, tracked in `PRAGMA user_version`.
const SCHEMA_VERSION: i32 = 5;

/// How long a writer waits for another writer, in milliseconds.
///
/// Long enough to cover a batch that is ingesting while another starts, short
/// enough that a genuinely stuck process is reported rather than waited on.
const BUSY_TIMEOUT_MS: u32 = 5_000;

/// The columns `row_to_record` expects, in the order it reads them.
const COLUMNS: &str = "id, source_path, source_size, source_mtime, mime, output_path, \
                       engine_id, engine_version, model, prompt_version, \
                       status, verified, conflict_count, created_at, updated_at, output_hash, \
                       source_copy, edited_at";

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

/// Version 2 records the content hash of the rendered output.
///
/// It exists so that `reconcile` can recognise a file the user moved: the
/// output path is what the manifest remembers, and a rename breaks that link
/// even though nothing about the document changed.
const MIGRATION_V2: &str = "ALTER TABLE documents ADD COLUMN output_hash TEXT";

/// Version 3 records where the bytes that were read are kept.
///
/// The knowledge base used to be Markdown plus a pointer at a file somebody
/// else owns, which made a record whose source moved a dead pointer and
/// re-reading a changed engine impossible. The copy is under `.docfusion/` and
/// content-addressed; this column is where it is, relative to the root.
///
/// Recorded rather than derived, so that "which records have no copy" is a
/// query rather than a walk, and so the file's extension cannot be guessed
/// differently in two places. Null for every row that existed before this,
/// which is exactly what those rows are.
const MIGRATION_V3: &str = "ALTER TABLE documents ADD COLUMN source_copy TEXT";

/// Version 4 is the full-text index.
///
/// A cache over the tree in the same way the manifest is: every row can be
/// rebuilt from the Markdown, so an index that is empty or wrong is a thing to
/// rebuild rather than to repair.
///
/// **`unicode61`, with the Chinese pulled apart before it is stored.** See
/// `text::indexed` for why: the default tokenizer does not segment Chinese, so
/// without that a whole clause is one token, and `trigram` — the obvious
/// alternative — matches nothing shorter than three characters, which is most
/// Chinese words. The pulling apart happens in the caller, so what is in this
/// table is not readable text and is never shown to anybody.
///
/// **The row is the document's**, which is why `document_id` is stored as well
/// as being the `documents` table's key. Writing an entry is then a delete and
/// an insert by rowid — two lookups — rather than a delete by matching a
/// column, which FTS5 answers by reading the whole content table.
/// Version 5 records that somebody has edited a document by hand.
///
/// **The document is the pipeline's output, and an edit makes it something
/// else** — no longer what the engines read, and no longer reproducible from
/// the source. That is worth knowing: the list says so, and a re-read can warn
/// before it takes the edit away. `None` means nobody has, which is every
/// document until somebody opens the editor.
const MIGRATION_V5: &str = "ALTER TABLE documents ADD COLUMN edited_at TEXT";

const MIGRATION_V4: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS text USING fts5(
  document_id UNINDEXED,
  title,
  body
);
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
    /// BLAKE3 of the rendered Markdown, so that `reconcile` can recognise this
    /// document's output after the file has been moved or renamed.
    ///
    /// `None` for records written before this column existed; such a document
    /// can only be relinked by re-processing it.
    pub output_hash: Option<String>,
    /// Where the bytes that were read are kept, relative to the knowledge root.
    ///
    /// Under `.docfusion/sources/` and named after the document's content hash,
    /// so the same file arriving from two folders is stored once. `None` for a
    /// record written before copies existed, and for one whose source could not
    /// be found when it was re-read.
    pub source_copy: Option<PathBuf>,
    /// When somebody last edited the body by hand, or `None`.
    ///
    /// The record of an edit rather than the edit itself: the words are in the
    /// file, and this is what says that file is no longer only what the engines
    /// read.
    pub edited_at: Option<String>,
}

/// Current UTC time as RFC 3339, to the second.
///
/// Callers stamp records with this rather than the manifest inventing times, so
/// a batch run can record when it actually did the work.
///
/// Sub-second precision is dropped deliberately. It carries no information a
/// knowledge base acts on, and it makes two timestamps taken seconds apart look
/// as different as two taken microseconds apart.
pub fn now_iso8601() -> String {
    time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .expect("zero is a valid nanosecond")
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

        // How long to wait for another process to finish writing before giving
        // up, in milliseconds. The default is zero, which means a second
        // process writing to the same knowledge base fails immediately rather
        // than waiting its turn — the difference between "two runs overlap" and
        // "one of them errors for no reason a person can see".
        connection.pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)?;

        // A write-ahead log lets readers and a writer work at the same time
        // instead of taking turns on the whole file. It is a property of the
        // database rather than of a connection, so setting it on every open is
        // idempotent and keeps a database made before this honest.
        //
        // Deliberately not `synchronous = OFF`: the write-ahead log makes
        // committing cheaper without giving up durability, and the speed it
        // would buy is worth less than the index surviving a power cut.
        connection.pragma_update(None, "journal_mode", "WAL")?;

        Self::migrate(&connection)?;
        Ok(Self { connection })
    }

    /// Applies each step in turn, so an older database is upgraded rather than
    /// rejected — an index that cannot be opened is an index that gets deleted,
    /// and deleting it throws away which documents were already processed.
    fn migrate(connection: &Connection) -> Result<(), StoreError> {
        let existing: i32 =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;

        if existing < 1 {
            connection.execute_batch(SCHEMA_V1)?;
        }
        if existing < 2 {
            connection.execute_batch(MIGRATION_V2)?;
        }
        if existing < 3 {
            connection.execute_batch(MIGRATION_V3)?;
        }
        if existing < 4 {
            connection.execute_batch(MIGRATION_V4)?;
        }
        if existing < 5 {
            connection.execute_batch(MIGRATION_V5)?;
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
                 status, verified, conflict_count, created_at, updated_at, output_hash,
                 source_copy, edited_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
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
                 updated_at     = excluded.updated_at,
                 output_hash    = excluded.output_hash,
                 source_copy    = excluded.source_copy,
                 edited_at      = excluded.edited_at",
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
                record.output_hash,
                record.source_copy.as_deref().map(path_to_string),
                record.edited_at,
            ],
        )?;

        Ok(())
    }

    /// Points a document at a new output path, leaving everything else alone.
    ///
    /// Used by `reconcile` when a file has been moved: the document did not
    /// change, only where its output lives.
    pub fn relink(&self, id: &str, output_path: &Path) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE documents SET output_path = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, path_to_string(output_path), now_iso8601()],
        )?;
        Ok(())
    }

    /// Forgets a document entirely.
    ///
    /// Only for records whose output is genuinely gone: the manifest is a cache
    /// over the file tree, and a cache entry pointing at nothing is not worth
    /// keeping. Re-processing the source rebuilds it.
    /// Puts a document's text in the index, replacing whatever was there.
    ///
    /// The row is the document's own row in `documents`, so this is a delete
    /// and an insert by rowid rather than by matching `document_id` — and that
    /// matters on the write path, which is every ingest: FTS5 answers
    /// `WHERE document_id = ?` by reading its whole content table, so an
    /// archive would pay for its own size on every document added, and a
    /// thousand-document import would pay for it a thousand times.
    ///
    /// The text is stored as [`text::indexed`] writes it, which is not the text
    /// anybody reads. Nothing in this table is ever shown: an excerpt is found
    /// in the original, by [`text::excerpt`].
    pub fn index_text(&self, id: &str, title: &str, body: &str) -> Result<(), StoreError> {
        let Some(row) = self.row_of(id)? else {
            return Ok(());
        };

        self.connection
            .execute("DELETE FROM text WHERE rowid = ?1", params![row])?;
        self.connection.execute(
            "INSERT INTO text(rowid, document_id, title, body) VALUES (?1, ?2, ?3, ?4)",
            params![row, id, text::indexed(title), text::indexed(body)],
        )?;

        Ok(())
    }

    /// Takes a document's text out of the index.
    ///
    /// Called by [`Manifest::forget`] rather than by whoever remembers to, so
    /// that a document leaving the index has one way of leaving and a search
    /// cannot return one the archive no longer holds.
    pub fn unindex(&self, id: &str) -> Result<(), StoreError> {
        let Some(row) = self.row_of(id)? else {
            return Ok(());
        };

        self.connection
            .execute("DELETE FROM text WHERE rowid = ?1", params![row])?;

        Ok(())
    }

    /// The documents holding every term, best match first.
    ///
    /// Ranked by `bm25`, which counts a word in the title eight times a word in
    /// the body: a document *called* 产品供销合同 is a better answer to 合同
    /// than one that merely mentions it. The two weights are the two numbers.
    ///
    /// `limit` is not a nicety. A term that appears in every document would
    /// otherwise return the archive, and what a list can show is twenty rows.
    pub fn search(&self, input: &str, limit: usize) -> Result<Vec<String>, StoreError> {
        let query = text::as_query(input);

        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let mut statement = self.connection.prepare(
            "SELECT document_id FROM text WHERE text MATCH ?1 \
             ORDER BY bm25(text, 8.0, 1.0) LIMIT ?2",
        )?;

        let rows = statement.query_map(params![query, limit as i64], |row| row.get(0))?;

        let mut found = Vec::new();
        for row in rows {
            found.push(row?);
        }

        Ok(found)
    }

    /// Empties the index, for a rebuild.
    pub fn clear_index(&self) -> Result<(), StoreError> {
        self.connection.execute("DELETE FROM text", [])?;
        Ok(())
    }

    /// How many documents the index holds.
    ///
    /// Asked beside the number of documents, because the two disagreeing is the
    /// one failure that makes search look broken rather than empty — and this
    /// index is a cache, so it can be behind.
    pub fn indexed_count(&self) -> Result<u64, StoreError> {
        let count: i64 = self
            .connection
            .query_row("SELECT count(*) FROM text", [], |row| row.get(0))?;

        Ok(count as u64)
    }

    /// The `documents` row a document occupies, which is the index's row too.
    fn row_of(&self, id: &str) -> Result<Option<i64>, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT rowid FROM documents WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn forget(&self, id: &str) -> Result<(), StoreError> {
        // The text goes with the row it belongs to. Leaving it would be worse
        // than a leak: the index is keyed by the row the document occupies, and
        // SQLite reuses a row id once the row holding it is gone — so an
        // orphaned entry is not merely stale, it is an entry the next document
        // to be given that row will be found under.
        self.unindex(id)?;

        self.connection
            .execute("DELETE FROM documents WHERE id = ?1", params![id])?;
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

    /// Finds a document by content id, source path, output path, or file name.
    ///
    /// All of these are accepted because a person thinks in file names while an
    /// agent holds a content hash, and neither should have to convert to the
    /// other's idea of an identifier. Exact matches are tried first so that a
    /// full path never loses to an unrelated document that shares its file name.
    pub fn find(&self, query: &str) -> Result<Option<DocumentRecord>, StoreError> {
        if let Some(record) = self.query_one("WHERE id = ?1", query)? {
            return Ok(Some(record));
        }
        // The recorded source path is matched directly as well as through the
        // alias table: the two normally agree, but a record that has no alias
        // would otherwise be unreachable by the path it came from.
        if let Some(record) = self.query_one("WHERE source_path = ?1", query)? {
            return Ok(Some(record));
        }
        if let Some(record) = self.query_one(
            "WHERE id = (SELECT document_id FROM aliases WHERE path = ?1 LIMIT 1)",
            query,
        )? {
            return Ok(Some(record));
        }
        if let Some(record) = self.query_one("WHERE output_path = ?1", query)? {
            return Ok(Some(record));
        }

        // Last resort: somebody typed `docfusion show report.pdf` rather than
        // the directory they imported it from. This scans, which is fine at the
        // scale a personal knowledge base reaches.
        Ok(self.all()?.into_iter().find(|record| {
            matches_name(&record.source_path, query) || matches_name(&record.output_path, query)
        }))
    }

    fn query_one(&self, filter: &str, value: &str) -> Result<Option<DocumentRecord>, StoreError> {
        let record = self
            .connection
            .query_row(
                &format!("SELECT {COLUMNS} FROM documents {filter}"),
                params![value],
                row_to_record,
            )
            .optional()?;

        Ok(record)
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
        output_hash: row.get(15)?,
        source_copy: row.get::<_, Option<String>>(16)?.map(PathBuf::from),
        edited_at: row.get(17)?,
    })
}

/// Paths are stored as text. On Windows a path is UTF-16 at the OS boundary, so
/// the only lossy case is a lone surrogate — which cannot name a real file.
fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn matches_name(path: &Path, query: &str) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy() == query)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two connections writing to one knowledge base is what two runs at once
    /// look like. Before the write-ahead log and the busy timeout, the second
    /// writer failed immediately — not corruptly, but for no reason a person
    /// running two commands could see.
    ///
    /// Asserted through the behaviour rather than through a pragma reading,
    /// because the pragma is the mechanism and the behaviour is the point.
    #[test]
    fn a_second_writer_waits_rather_than_failing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.db");

        let writer = Manifest::open(&path).unwrap();
        let waiting = Manifest::open(&path).unwrap();

        // The first writer takes the write lock and keeps it.
        writer.connection.execute_batch("BEGIN IMMEDIATE").unwrap();

        // The second may only succeed if it is willing to wait, and the first
        // must let go while it does.
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            writer.connection.execute_batch("COMMIT").unwrap();
        });

        waiting
            .connection
            .execute_batch("BEGIN IMMEDIATE")
            .expect("the second writer should wait for the first rather than give up");
        waiting.connection.execute_batch("COMMIT").unwrap();

        release.join().unwrap();
    }

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
            output_hash: Some("b".repeat(64)),
            source_copy: None,
            edited_at: None,
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
    fn find_matches_by_file_name() {
        let manifest = Manifest::open_in_memory().unwrap();
        let mut stored = record("hash-a", "a");
        stored.source_path = PathBuf::from("D:/in/reports/annual.pdf");
        stored.output_path = PathBuf::from("docs/annual.md");
        manifest.upsert(&stored).unwrap();

        // A person types the file they remember, not the path they imported
        // it from.
        assert!(manifest.find("annual.pdf").unwrap().is_some());
        assert!(manifest.find("annual.md").unwrap().is_some());
        assert!(manifest.find("missing.pdf").unwrap().is_none());
    }

    #[test]
    fn find_still_matches_content_ids() {
        let manifest = Manifest::open_in_memory().unwrap();
        manifest.upsert(&record("hash-a", "a")).unwrap();

        assert!(manifest.find("hash-a").unwrap().is_some());
    }

    /// An exact path must not lose to a different document that happens to
    /// share its file name.
    #[test]
    fn find_prefers_an_exact_match_over_a_file_name() {
        let manifest = Manifest::open_in_memory().unwrap();

        let mut wanted = record("hash-a", "a");
        wanted.source_path = PathBuf::from("D:/one/report.pdf");
        manifest.upsert(&wanted).unwrap();

        let mut decoy = record("hash-b", "b");
        decoy.source_path = PathBuf::from("D:/two/report.pdf");
        manifest.upsert(&decoy).unwrap();

        let found = manifest.find("D:/two/report.pdf").unwrap().unwrap();
        assert_eq!(found.id, "hash-b");
    }

    /// Sub-second precision carries nothing a knowledge base acts on, and makes
    /// timestamps taken seconds apart look as different as ones taken
    /// microseconds apart.
    #[test]
    fn timestamps_are_recorded_to_the_second() {
        let now = now_iso8601();

        assert!(now.ends_with('Z'), "{now}");
        assert!(!now.contains('.'), "{now}");
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
