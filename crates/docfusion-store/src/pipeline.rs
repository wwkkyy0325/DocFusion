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
use docfusion_core::text;

use crate::atomic::write_atomic;
use crate::hash::content_hash;
use crate::manifest::{DocumentRecord, Manifest, Status, StoreError, now_iso8601};

/// Directory holding the sidecar index, kept out of the document tree so that
/// `.md` files stay the only thing in it.
pub const STATE_DIR: &str = ".docfusion";
pub const MANIFEST_FILE: &str = "manifest.db";
/// Extracted documents are written here, under the knowledge root.
pub const DOCS_DIR: &str = "docs";

/// The bytes that were read are kept here, under the state directory.
///
/// Inside `.docfusion/` rather than beside `docs/`, and that placement is the
/// decision: a knowledge base whose `docs/` is synced or committed should not
/// drag along the contracts and bank statements that were read into it. The
/// Markdown is the thing worth syncing; the sources are the thing worth having
/// when the original is gone, and those are different jobs.
///
/// Named after the document's content hash, so the same file arriving from two
/// folders is stored once — the aliases table already models the two names.
pub const SOURCES_DIR: &str = "sources";

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

    #[error(
        "the source of {id} is not where the index says it is, so there is nothing to copy. \
         It was read from a file that has since moved or gone, and this knowledge base kept \
         no copy at the time."
    )]
    SourceGone { id: String },
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

/// The first bytes of a file, and how long the whole of it is.
///
/// Enough to sniff what a document is, which is all a probe needs. Reading the
/// whole of every file in the archive to ask "would this be read differently
/// now" is a scan that costs the archive's own size in I/O to answer a question
/// about its first few hundred bytes.
fn head_of(path: &Path) -> Option<(Vec<u8>, u64)> {
    use std::io::Read as _;

    let mut file = std::fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();

    let mut head = vec![0u8; SNIFF_LENGTH];
    let mut read = 0;

    while read < head.len() {
        match file.read(&mut head[read..]) {
            Ok(0) => break,
            Ok(filled) => read += filled,
            Err(_) => return None,
        }
    }

    head.truncate(read);
    Some((head, size))
}

/// What routing needs to know about a document: the first bytes, the name, and
/// how long the whole of it is.
///
/// A function because three things build one — reading something new, re-reading
/// something old, and asking whether a document would be read differently now —
/// and a probe that disagreed with itself between them would route the same
/// document two ways.
fn probe_of(source: &Path, head: &[u8], size: u64) -> InputProbe {
    InputProbe::new(
        head,
        source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
        size,
    )
}

/// The last part of a path, as somebody would recognise the file by.
///
/// Falls back to the whole path, because a path with no file name is a path
/// somebody typed and a blank line is worse than a long one.
fn source_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// One document a search found.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub id: String,
    /// The file this document was read from, as somebody would recognise it.
    pub name: String,
    /// The passage the match is in, with it marked, when the document can still
    /// be read. See [`Pipeline::search`] for when it cannot.
    pub excerpt: Option<String>,
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
        create_dir(&state.join(SOURCES_DIR))?;

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

    /// Writes somebody's own words into a document's Markdown.
    ///
    /// **The body only.** The front matter and the conflict report are this
    /// program's — one is the record of where the document came from and the
    /// other is what the engines disagreed about — and somebody editing the
    /// words of a document has no business in either. `render::parts` is what
    /// keeps the three apart and `joined` is what puts them back, so an editor
    /// cannot damage what it was never handed.
    ///
    /// **The record is marked**, because the file stops being what the engines
    /// read the moment somebody types in it: it is what they read with a
    /// person's own words on top, and a re-read will take the difference away.
    /// The mark is what lets the window say so before that happens.
    ///
    /// **The output hash moves with it**, since the file changed — `reconcile`
    /// recognises a document's output by that hash, and leaving it stale would
    /// make this file look like somebody else's.
    ///
    /// The clock is passed in rather than read here, like `created_at` in
    /// [`Pipeline::ingest`], so that the same edit saved twice is the same
    /// bytes twice.
    pub fn save_body(
        &self,
        query: &str,
        body: &str,
        at: &str,
    ) -> Result<DocumentRecord, PipelineError> {
        let record = self.resolve(query)?;
        let path = self.root.join(&record.output_path);

        let markdown = std::fs::read_to_string(&path).map_err(|source| PipelineError::Read {
            path: path.clone(),
            source,
        })?;

        let split = render::parts(&markdown);
        let written = render::edited(&split.with_body(body).joined(), at);

        std::fs::write(&path, written.as_bytes()).map_err(|source| PipelineError::Write {
            path: path.clone(),
            source,
        })?;

        let saved = DocumentRecord {
            updated_at: at.to_owned(),
            output_hash: Some(content_hash(written.as_bytes())),
            edited_at: Some(at.to_owned()),
            ..record
        };

        self.manifest.upsert(&saved)?;

        // The words that are now on disk are the words a search should find.
        self.manifest.index_text(
            &saved.id,
            &source_name(&saved.source_path),
            render::body_of(&written),
        )?;

        Ok(saved)
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

    /// Deletes a document, the Markdown it was written to, and the copy of
    /// what it was read from.
    ///
    /// **The files go first.** A row left behind by a file that would not
    /// delete is a row that opens onto nothing, which every reader of the index
    /// already knows how to show; a file left behind by a row that was removed
    /// is a document nobody can find and nothing that would report it. The
    /// order is chosen so that a failure leaves something somebody can act on.
    ///
    /// **The document somebody chose is not here to delete.** It was read where
    /// it lay and what was kept is a copy, so destroying the record destroys
    /// what this program holds and not the original — which is what both front
    /// ends promise, in those words.
    ///
    /// This is the difference from [`Pipeline::forget`]: that one leaves the
    /// copy and this one takes it. Taking the copy away is what "delete this
    /// document" means, and leaving it would mean a delete that does not give
    /// the room back. A copy left by *forget* is a different thing — nothing
    /// points at it any more, and `reconcile` reports it rather than guessing
    /// that it is rubbish.
    pub fn delete(&self, query: &str) -> Result<DocumentRecord, PipelineError> {
        let record = self.resolve(query)?;
        let path = self.output_of(&record)?;

        fs::remove_file(&path).map_err(|source| PipelineError::Delete {
            path: path.clone(),
            source,
        })?;

        // A copy that cannot be removed does not fail the delete: the document
        // is out of the knowledge base either way, and a file left in the state
        // directory is something `reconcile` will report and somebody can
        // remove by hand. Failing here would leave the Markdown already gone
        // and say nothing had happened.
        if let Some(copy) = record.source_copy.as_ref()
            && let Ok(copy) = self.inside(copy)
        {
            let _ = fs::remove_file(copy);
        }

        self.manifest.forget(&record.id)?;

        Ok(record)
    }

    /// The documents whose stored reading is not what this machine would
    /// produce now.
    ///
    /// Asked by comparing what the record says produced it — an engine, a
    /// version, a model, a prompt — against what the engines in this router
    /// would say about the same bytes. Any difference is a difference in the
    /// output, which is the whole reason the stamp is stored.
    ///
    /// Only the first few hundred bytes of a document are read, because the
    /// question is about routing and a scan that read every file in the archive
    /// would cost its size in I/O to answer it. A document whose bytes cannot be
    /// found at all is **not** reported as stale: it cannot be re-read, and
    /// saying it needs re-reading would be a promise this cannot keep. The
    /// caller asks [`Pipeline::unreachable`] for those.
    pub fn stale(&self) -> Result<Vec<DocumentRecord>, PipelineError> {
        let mut stale = Vec::new();

        for record in self.manifest.all()? {
            let Some(path) = self.bytes_whereabouts(&record) else {
                continue;
            };
            let Some((head, size)) = head_of(&path) else {
                continue;
            };

            let probe = probe_of(&record.source_path, &head, size);

            if !self
                .router
                .candidates(&probe)
                .iter()
                .any(|engine| engine.stamp() == record.engine)
            {
                stale.push(record);
            }
        }

        Ok(stale)
    }

    /// The documents nothing can find the bytes of any more.
    ///
    /// A record with no copy and no source — the state every record written
    /// before copies existed is in, as soon as the file it came from moves. Its
    /// Markdown is intact and it can be read, searched and deleted as usual;
    /// what it cannot do is be read again, which is worth saying rather than
    /// silently doing nothing about.
    pub fn unreachable(&self) -> Result<Vec<DocumentRecord>, PipelineError> {
        Ok(self
            .manifest
            .all()?
            .into_iter()
            .filter(|record| self.bytes_whereabouts(record).is_none())
            .collect())
    }

    /// Re-reads a document from the bytes it was read from the first time.
    ///
    /// **What the copy is for.** A document is read by whatever this machine
    /// had at the time, and that stamp is what the manifest remembers. When the
    /// models, the endpoint, the prompt or an engine's own version changes, the
    /// Markdown on disk is the old engine's answer and nothing would ever ask
    /// again — `ingest` reuses on a matching stamp, and a document nobody drags
    /// in twice is never looked at again.
    ///
    /// Read from the copy rather than from where the document came from: the
    /// original may have moved, and a re-read that skipped exactly the documents
    /// whose sources had moved would skip the ones that need it most.
    ///
    /// **What is not changed.** Where the record says the document came from,
    /// when it was first read, and every name it is known by. Those are about
    /// the document rather than about the reading, and a re-read that replaced
    /// the path somebody recognises with one inside the state directory would
    /// be a re-read that lost the document.
    pub fn reindex(&self, record: &DocumentRecord, mode: Mode) -> Result<Outcome, PipelineError> {
        let copy = self.keep_source_of(record)?;
        let bytes = fs::read(&copy).map_err(|source| PipelineError::Read {
            path: copy.clone(),
            source,
        })?;

        // Checked rather than assumed. Everything else here is about to write a
        // document under this record's id, and bytes that do not hash to it
        // would write a different document under the name of this one — the
        // failure that is discovered years later, if ever.
        if content_hash(&bytes) != record.id {
            return Err(PipelineError::SourceGone {
                id: record.id.clone(),
            });
        }

        let probe = probe_of(&record.source_path, &bytes[..bytes.len().min(SNIFF_LENGTH)], bytes.len() as u64);
        let (document, stamp, markdown) = self.produce(
            &record.source_path,
            &record.id,
            &bytes,
            &probe,
            &record.created_at,
        )?;

        let output = self.output_path(&document, &record.id)?;
        let conflicts = document.conflicts.len();

        if mode == Mode::Apply {
            write_atomic(&output, markdown.as_bytes()).map_err(|source| PipelineError::Write {
                path: output.clone(),
                source,
            })?;

            self.manifest.upsert(&DocumentRecord {
                engine: stamp,
                mime: document.source.mime.clone(),
                output_path: self.relative(&output),
                status: Status::Done,
                verified: document.pages.iter().all(|page| page.provenance.verified),
                conflict_count: conflicts as u32,
                updated_at: now_iso8601(),
                output_hash: Some(content_hash(markdown.as_bytes())),
                // The copy is where it was: the name is the content, the
                // content is the same, so `keep_source_of` above has already
                // put it back if it had gone.
                source_copy: Some(self.relative(&copy)),
                // **The file was just written by the engines**, so whatever
                // somebody had typed into it is gone, and a mark saying they
                // had is a mark about a file that no longer exists. Carried over
                // from the old record by the spread below until this line was
                // added, which made `reindex` the one re-read that said the
                // document was still edited while overwriting the edit.
                edited_at: None,
                ..record.clone()
            })?;

            self.manifest.index_text(
                &record.id,
                &source_name(&record.source_path),
                render::body_of(&markdown),
            )?;
        }

        Ok(Outcome {
            id: record.id.clone(),
            source: record.source_path.clone(),
            output,
            reused: false,
            conflicts,
            status: Status::Done,
        })
    }

    /// Reads a document and renders it, which is the part reading something
    /// new and re-reading something old have in common.
    ///
    /// The same code for both on purpose: a re-read that took a slightly
    /// different path through extraction than a first read would produce a
    /// slightly different document, and the whole point of a re-read is that
    /// the only thing that changed is the engine.
    fn produce(
        &self,
        source: &Path,
        id: &str,
        bytes: &[u8],
        probe: &InputProbe,
        created_at: &str,
    ) -> Result<(Document, EngineStamp, String), PipelineError> {
        let document =
            self.router
                .extract(probe, &source_ref(source, id, probe, bytes), bytes)?;
        let stamp = self.stamp_of(&document)?;

        let markdown = render::markdown(&document, created_at);

        Ok((document, stamp, markdown))
    }

    /// Where a document's bytes are, preferring the copy.
    ///
    /// A guess rather than an answer: nothing here checks that the bytes are the
    /// ones the record describes, which costs reading the whole file and is
    /// [`Pipeline::keep_source_of`]'s job. This is for asking questions *about*
    /// a document — whether it would be read differently now — rather than for
    /// reading it.
    fn bytes_whereabouts(&self, record: &DocumentRecord) -> Option<PathBuf> {
        if let Some(copy) = record.source_copy.as_ref()
            && let Ok(path) = self.inside(copy)
            && path.is_file()
        {
            return Some(path);
        }

        if record.source_path.is_file() {
            return Some(record.source_path.clone());
        }

        self.manifest
            .aliases_of(&record.id)
            .ok()?
            .into_iter()
            .find(|path| path.is_file())
    }

    /// How many documents are indexed, against how many there are.
    ///
    /// The two disagreeing is the one failure that makes search look broken
    /// rather than empty, so both callers ask before answering a search rather
    /// than after. It is two counts and no reading.
    pub fn index_coverage(&self) -> Result<(u64, u64), PipelineError> {
        Ok((
            self.manifest.indexed_count()?,
            self.manifest.document_count()?,
        ))
    }

    /// Rebuilds the search index from the Markdown files, and says how many
    /// documents went in.
    ///
    /// The index is a cache over the tree in the same way the manifest is, so
    /// this is what to run when it is empty or behind: a knowledge base made
    /// before search existed, or one whose database was replaced. Nothing is
    /// read out of the sources — the Markdown is what is indexed, and it is
    /// right there, which also means a document whose source has gone can still
    /// be found.
    ///
    /// A document whose Markdown cannot be read is skipped rather than failing
    /// the rebuild. It is drift, and `reconcile` is what reports drift.
    pub fn rebuild_index(&self) -> Result<usize, PipelineError> {
        self.manifest.clear_index()?;

        let mut indexed = 0;
        for record in self.manifest.all()? {
            let Ok(markdown) = fs::read_to_string(self.root.join(&record.output_path)) else {
                continue;
            };

            self.manifest.index_text(
                &record.id,
                &source_name(&record.source_path),
                render::body_of(&markdown),
            )?;
            indexed += 1;
        }

        Ok(indexed)
    }

    /// The documents holding every term in `query`, best match first.
    ///
    /// The excerpt is found in the Markdown rather than asked of the index,
    /// because the index does not hold the text anybody wrote — see
    /// [`docfusion_core::text::indexed`]. Reading the file is a few kilobytes
    /// per hit and the operating system has it in hand from the write.
    ///
    /// A document whose Markdown has gone since it was indexed comes back
    /// without an excerpt rather than being dropped: it matched, and a list
    /// that quietly loses rows is worse than one with a gap explained by the
    /// row itself.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Found>, PipelineError> {
        let terms = text::terms(query);
        let mut found = Vec::with_capacity(limit);

        for id in self.manifest.search(query, limit)? {
            let Some(record) = self.manifest.find(&id)? else {
                continue;
            };

            let excerpt = fs::read_to_string(self.root.join(&record.output_path))
                .ok()
                .and_then(|markdown| text::excerpt(render::body_of(&markdown), &terms));

            found.push(Found {
                id,
                name: source_name(&record.source_path),
                excerpt,
            });
        }

        Ok(found)
    }

    /// Keeps a copy of the bytes that were read, and answers with where.
    ///
    /// **Why there is a copy at all.** The knowledge base used to be Markdown
    /// plus a pointer at a file somebody else owns, which made a document whose
    /// original moved a dead pointer that could never be read again — including
    /// after the engine, the model or the prompt changed, which is the one
    /// thing `reindex` exists to do. The bytes are the only way back.
    ///
    /// **Content-addressed, so it is idempotent.** The name is the document's
    /// own hash, which is also its id, so calling this twice writes once, and
    /// re-adding a source whose copy has been deleted puts it back. That is why
    /// both the fresh path and the reuse path call it.
    ///
    /// The extension is the one the file arrived with, kept so that somebody
    /// browsing the directory can tell a PNG from a PDF. Nothing in the program
    /// reads it: where a record's copy is, is recorded in the index rather than
    /// recomputed.
    fn keep_source(&self, id: &str, source: &Path, bytes: &[u8]) -> Result<PathBuf, PipelineError> {
        let path = self.source_copy_path(id, source);

        if path.is_file() {
            return Ok(path);
        }

        // Written through a temporary file, like the Markdown, so that a copy
        // interrupted half way is not a file that looks complete to the next
        // run and is not the bytes anybody asked for.
        write_atomic(&path, bytes).map_err(|error| PipelineError::Write {
            path: path.clone(),
            source: error,
        })?;

        Ok(path)
    }

    /// Puts back the copy of a record's source, from wherever the index says
    /// that source is.
    ///
    /// Looked for where the record was made from and then at every name the
    /// index remembers for it, because a document that has been moved is
    /// recognised by its content and the aliases are where the other names
    /// live. Every candidate is read and hashed rather than trusted: putting
    /// something at `<hash>` that the hash does not describe would be worse
    /// than having no copy at all, and it would be discovered years later, by
    /// a re-read producing a different document under the same id.
    pub fn keep_source_of(&self, record: &DocumentRecord) -> Result<PathBuf, PipelineError> {
        // Already kept, which is the ordinary case and the one that has to be
        // cheap: `reconcile` asks about every record whose copy is missing, but
        // nothing stops anybody asking about one whose copy is here.
        if let Some(copy) = record.source_copy.as_ref()
            && let Ok(path) = self.inside(copy)
            && path.is_file()
        {
            return Ok(path);
        }

        let named = std::iter::once(record.source_path.clone())
            .chain(self.manifest.aliases_of(&record.id)?);

        for path in named {
            // Unreadable and unreadable-for-this-document are the same thing
            // here: the next name is tried either way, and the answer at the
            // end says what happened to all of them.
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };

            if content_hash(&bytes) != record.id {
                continue;
            }

            return self.keep_source(&record.id, &path, &bytes);
        }

        Err(PipelineError::SourceGone {
            id: record.id.clone(),
        })
    }

    /// Where the copy of a document's source goes.
    fn source_copy_path(&self, id: &str, source: &Path) -> PathBuf {
        let extension = source
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase());

        let name = match extension {
            Some(extension) if !extension.is_empty() => format!("{id}.{extension}"),
            _ => id.to_owned(),
        };

        self.root.join(STATE_DIR).join(SOURCES_DIR).join(name)
    }

    /// Where a record's Markdown is, once it is known to be somewhere this
    /// pipeline is allowed to reach.
    fn output_of(&self, record: &DocumentRecord) -> Result<PathBuf, PipelineError> {
        self.inside(&record.output_path)
    }

    /// Resolves a path the index recorded against the knowledge root, or
    /// refuses it.
    ///
    /// Both paths that come back out of the database — the Markdown and the
    /// copy of the source — go through here, because both are joined onto the
    /// root and then handed to `remove_file`. The database is a file somebody
    /// could have edited by hand, and this is the only thing in the program
    /// that destroys anything.
    ///
    /// `starts_with(root)` is not the check, and looks like it is: it walks
    /// components and `..` is one, so `root.join("../../x")` starts with `root`
    /// and the guard somebody reaches for first passes. A leading slash and a
    /// drive letter have the same problem, which is why all three are refused
    /// before the join rather than after it.
    fn inside(&self, recorded: &Path) -> Result<PathBuf, PipelineError> {
        let outside = recorded.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        });

        if outside {
            return Err(PipelineError::Outside {
                path: recorded.to_path_buf(),
            });
        }

        Ok(self.root.join(recorded))
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

        if let Some(outcome) = self.reuse(&id, source, &probe, &bytes, mode)? {
            return Ok(outcome);
        }

        let created_at = now_iso8601();
        let (document, stamp, markdown) = self.produce(source, &id, &bytes, &probe, &created_at)?;
        let output = self.output_path(&document, &id)?;

        if mode == Mode::Apply {
            // The bytes first. A copy that will not write is a failed ingest
            // rather than a silent one, and doing it before the Markdown means
            // a failure leaves nothing half done — no Markdown without a
            // record, and no record pointing at bytes nobody kept.
            let copy = self.keep_source(&id, source, &bytes)?;

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
                source_copy: Some(self.relative(&copy)),
                // The file was just written by the engines, so whatever anybody
                // had typed into it before this is gone with it. A document
                // re-added without being read keeps its mark, because this is
                // the path that does not run: `reuse` returns long before here.
                edited_at: None,
            })?;
            self.manifest.add_alias(&id, source)?;

            // And the text, so that a search can find this document again.
            // Written from the rendering that was just made rather than read
            // back out of the file: the bytes are in hand, the report is in
            // them, and `body_of` is what takes it back out.
            self.manifest.index_text(
                &id,
                &source
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                render::body_of(&markdown),
            )?;
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
        bytes: &[u8],
        mode: Mode,
    ) -> Result<Option<Outcome>, PipelineError> {
        for engine in self.router.candidates(probe) {
            let Some(mut record) = self.manifest.lookup(id, &engine.stamp())? else {
                continue;
            };

            let output = self.root.join(&record.output_path);

            // A record whose output has been deleted is not a usable cache
            // entry. Reporting a hit would hand back a path to a file that is
            // not there and skip the write that would put it back, so the
            // lookup is treated as a miss and the document is produced again.
            if !output.exists() {
                continue;
            }

            if mode == Mode::Apply {
                self.manifest.add_alias(id, source)?;

                // The bytes as well, which is not redundant work: a record
                // written before copies existed has none, and one whose copy
                // somebody deleted has a path that leads nowhere. Either way
                // re-adding the source is the moment to put it back, and the
                // copy is named after the content, so this is one `stat` when
                // it is already there.
                let copy = self.keep_source(id, source, bytes)?;
                let kept = self.relative(&copy);

                if record.source_copy.as_deref() != Some(kept.as_path()) {
                    let mut kept_record = record;
                    kept_record.source_copy = Some(kept);
                    self.manifest.upsert(&kept_record)?;
                    record = kept_record;
                }
            }

            return Ok(Some(Outcome {
                id: id.to_owned(),
                source: source.to_path_buf(),
                output,
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

    /// A document is findable by a word in it, and by a two-character Chinese
    /// word in particular.
    ///
    /// Written as two characters on purpose. It is the case that made this
    /// index what it is: FTS5's `trigram` tokenizer matches nothing shorter
    /// than three characters, which on a corpus test meant 合同, 供销, 学生 and
    /// 报到 all returned zero. Most Chinese words are two characters, so a
    /// search built on trigram would have looked like it worked, on the English
    /// documents, and found nothing in the ones it was for.
    #[test]
    fn a_document_is_found_by_a_two_character_word() {
        let (dir, mut pipeline) = setup();
        pipeline
            .ingest(
                &write_source(dir.path(), "合同.txt", "产品供销合同 甲方与乙方"),
                Mode::Apply,
            )
            .unwrap();
        pipeline
            .ingest(
                &write_source(dir.path(), "注册.txt", "学生报到注册流程说明"),
                Mode::Apply,
            )
            .unwrap();

        for (query, expected) in [("合同", "合同.txt"), ("供销", "合同.txt"), ("报到", "注册.txt")] {
            let found = pipeline.search(query, 10).unwrap();

            assert_eq!(found.len(), 1, "{query} found {found:?}");
            assert_eq!(found[0].name, expected, "{query}");
        }
    }

    /// Every term has to be in the document, not any of them.
    #[test]
    fn a_search_for_two_words_wants_both() {
        let (dir, mut pipeline) = setup();
        pipeline
            .ingest(
                &write_source(dir.path(), "one.txt", "甲方在第一条约定"),
                Mode::Apply,
            )
            .unwrap();
        pipeline
            .ingest(
                &write_source(dir.path(), "two.txt", "乙方在第二条约定"),
                Mode::Apply,
            )
            .unwrap();

        assert_eq!(pipeline.search("甲方 约定", 10).unwrap().len(), 1);
        assert!(pipeline.search("甲方 乙方", 10).unwrap().is_empty());
    }

    /// The passage comes back as the document has it, with the match marked —
    /// not as the index stores it, which has a space between every pair of
    /// Chinese characters.
    #[test]
    fn the_passage_is_the_document_s_own_text() {
        let (dir, mut pipeline) = setup();
        pipeline
            .ingest(
                &write_source(dir.path(), "合同.txt", "本合同由甲方与乙方于二〇二四年签订"),
                Mode::Apply,
            )
            .unwrap();

        let found = pipeline.search("甲方", 10).unwrap();
        let excerpt = found[0].excerpt.as_deref().expect("a passage");

        assert!(
            excerpt.contains(&format!("{}甲方{}", text::HIT_START, text::HIT_END)),
            "{excerpt}"
        );
        assert!(
            !excerpt.contains("甲 方"),
            "the passage has the index's spacing in it: {excerpt}"
        );
    }

    /// Latin works the same way, as whole words.
    #[test]
    fn a_latin_word_is_found_whole() {
        let (dir, mut pipeline) = setup();
        pipeline
            .ingest(
                &write_source(dir.path(), "notes.txt", "the openEuler tutorial"),
                Mode::Apply,
            )
            .unwrap();

        assert_eq!(pipeline.search("openEuler", 10).unwrap().len(), 1);
        assert_eq!(pipeline.search("OPENEULER", 10).unwrap().len(), 1);

        // And a fragment of a word is not a word: this is a lexical index, and
        // pretending otherwise is how a search box gets slower for nothing.
        assert!(pipeline.search("Euler", 10).unwrap().is_empty());
    }

    /// The index says how far behind it is, and rebuilding catches it up.
    ///
    /// This is the state a knowledge base made before search existed is in, and
    /// the state a replaced database leaves behind. Both front ends ask before
    /// answering a search, because an index that is behind is *the* reason a
    /// search finds nothing while the archive plainly holds it.
    #[test]
    fn the_index_can_be_behind_and_can_be_caught_up() {
        let (dir, mut pipeline) = setup();
        // Different bodies, or they are one document: the id is the content
        // hash, and the same words from two files is the same document twice.
        for (name, body) in [("one.txt", "the first words"), ("two.txt", "the second words")] {
            pipeline
                .ingest(&write_source(dir.path(), name, body), Mode::Apply)
                .unwrap();
        }

        pipeline.manifest().clear_index().unwrap();
        assert_eq!(pipeline.index_coverage().unwrap(), (0, 2));
        assert!(pipeline.search("words", 10).unwrap().is_empty());

        assert_eq!(pipeline.rebuild_index().unwrap(), 2);
        assert_eq!(pipeline.index_coverage().unwrap(), (2, 2));
        assert_eq!(pipeline.search("words", 10).unwrap().len(), 2);
    }

    /// A document that is taken out of the archive stops being found.
    #[test]
    fn a_document_that_went_is_no_longer_found() {
        let (dir, mut pipeline) = setup();
        let wrote = pipeline
            .ingest(
                &write_source(dir.path(), "notes.txt", "the words in it"),
                Mode::Apply,
            )
            .unwrap();

        assert_eq!(pipeline.search("words", 10).unwrap().len(), 1);

        pipeline.forget(&wrote.id).unwrap();
        assert!(pipeline.search("words", 10).unwrap().is_empty());

        // And the row it occupied is free for the next document, which is the
        // reason the index is keyed by it and the reason forgetting has to
        // clear it: a longer text under the same row would be found by the
        // words of the document before it.
        pipeline
            .ingest(
                &write_source(dir.path(), "other.txt", "something else entirely"),
                Mode::Apply,
            )
            .unwrap();

        assert!(pipeline.search("words", 10).unwrap().is_empty());
        assert_eq!(pipeline.search("else", 10).unwrap().len(), 1);
    }

    /// What is indexed is the document, not what this program wrote about it.
    ///
    /// The conflict report carries the extracted text of the regions two
    /// engines disagreed about, so indexing it would make a document findable
    /// by a passage that appears once in the document and once in the note
    /// about it — and would rank it by the wording of a disagreement. The
    /// front matter goes too: a title, a hash and an engine are about the
    /// document rather than in it.
    #[test]
    fn neither_the_report_nor_the_front_matter_is_indexed() {
        let (dir, mut pipeline) = setup();
        let wrote = pipeline
            .ingest(
                &write_source(dir.path(), "notes.txt", "the words in the body"),
                Mode::Apply,
            )
            .unwrap();

        assert_eq!(pipeline.search("body", 10).unwrap().len(), 1);
        assert!(pipeline.search("sha256", 10).unwrap().is_empty(), "front matter");
        assert!(pipeline.search("engines", 10).unwrap().is_empty(), "front matter");
        assert!(pipeline.search("created_at", 10).unwrap().is_empty(), "front matter");

        // And what a report would add: appended the way the renderer appends
        // one, then indexed the way the pipeline indexes — which is what
        // `body_of` is for.
        let mut markdown = fs::read_to_string(&wrote.output).unwrap();
        markdown.push_str("\n## Conflicts\n\n- **page 1** — a disagreement\n");

        pipeline
            .manifest()
            .index_text(&wrote.id, "notes.txt", render::body_of(&markdown))
            .unwrap();

        assert!(
            pipeline.search("disagreement", 10).unwrap().is_empty(),
            "the report about the document was indexed"
        );
        assert_eq!(pipeline.search("body", 10).unwrap().len(), 1);
    }

    /// **The whole reason the copy exists.**
    ///
    /// A document read by one engine, the engine then replaced, and the source
    /// file then *moved away* — which is the state that used to be permanent.
    /// The copy is what lets it be read again, and the record keeps saying where
    /// the document came from rather than where the copy is.
    #[test]
    fn a_document_is_read_again_after_the_engine_changed_and_the_source_moved() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "the words");

        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();
        let record = pipeline.manifest().find(&wrote.id).unwrap().unwrap();

        // As a machine that has since been upgraded looks: the record says an
        // engine a version older than the one installed, which is exactly what
        // a changed model, endpoint or engine leaves behind.
        pipeline
            .manifest()
            .upsert(&DocumentRecord {
                engine: EngineStamp::new(record.engine.id.clone(), "0.9.0"),
                ..record.clone()
            })
            .unwrap();

        // And the file it came from is where the index does not know to look.
        let moved = dir.path().join("moved-away.txt");
        fs::rename(&source, &moved).unwrap();

        let stale = pipeline.stale().unwrap();
        assert_eq!(stale.len(), 1, "the installed engine should find this out of date");
        assert_eq!(stale[0].id, wrote.id);

        pipeline.reindex(&stale[0], Mode::Apply).unwrap();

        let after = pipeline.manifest().find(&wrote.id).unwrap().unwrap();
        assert_eq!(after.engine.version, "1.0.0", "it was not read by what is installed");
        assert_eq!(
            after.source_path, source,
            "the document's own path was replaced by the copy's"
        );
        assert_eq!(after.created_at, record.created_at, "it was re-dated");
        assert!(after.output_path.is_file() || pipeline.root().join(&after.output_path).is_file());

        // And it is no longer out of date, so running it again does nothing.
        assert!(pipeline.stale().unwrap().is_empty());
    }

    /// A document whose bytes cannot be found is not called out of date.
    ///
    /// It cannot be re-read, and saying it needs re-reading would be a promise
    /// this cannot keep. It is reported as what it is, which is a separate
    /// question with a separate answer.
    #[test]
    fn a_document_with_no_bytes_is_unreachable_rather_than_stale() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "the words");
        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();

        let record = pipeline.manifest().find(&wrote.id).unwrap().unwrap();
        let copy = pipeline.root().join(record.source_copy.clone().expect("a copy"));

        // As a record written before copies existed, whose source has since
        // moved: no copy, and nothing where the index says the document is.
        pipeline
            .manifest()
            .upsert(&DocumentRecord {
                source_copy: None,
                edited_at: None,
                ..record
            })
            .unwrap();
        fs::remove_file(&copy).unwrap();
        fs::remove_file(&source).unwrap();

        assert!(pipeline.stale().unwrap().is_empty());
        assert_eq!(pipeline.unreachable().unwrap().len(), 1);
    }

    /// A re-read that was asked for by name happens whether or not it looks
    /// needed, and produces the same document when nothing has changed.
    #[test]
    fn reading_a_document_again_that_has_not_changed_produces_the_same_thing() {
        let (dir, mut pipeline) = setup();
        let wrote = pipeline
            .ingest(&write_source(dir.path(), "notes.txt", "the words"), Mode::Apply)
            .unwrap();

        let before = fs::read_to_string(&wrote.output).unwrap();
        let record = pipeline.manifest().find(&wrote.id).unwrap().unwrap();

        pipeline.reindex(&record, Mode::Apply).unwrap();

        assert_eq!(fs::read_to_string(&wrote.output).unwrap(), before);
        assert!(pipeline.stale().unwrap().is_empty());
        assert_eq!(pipeline.search("words", 10).unwrap().len(), 1);
    }

    /// A copy that is not the document it claims to be stops the re-read.
    ///
    /// Everything else here is about to write a document under this record's
    /// id, and bytes that do not hash to it would write a *different* document
    /// under the name of this one — which is a thing discovered years later, by
    /// a search returning the wrong page.
    #[test]
    fn a_copy_that_is_not_what_it_claims_stops_the_re_read() {
        let (dir, mut pipeline) = setup();
        let wrote = pipeline
            .ingest(&write_source(dir.path(), "notes.txt", "the words"), Mode::Apply)
            .unwrap();

        let record = pipeline.manifest().find(&wrote.id).unwrap().unwrap();
        let copy = pipeline.root().join(record.source_copy.clone().expect("a copy"));

        fs::write(&copy, "not the bytes anybody read").unwrap();
        fs::remove_file(dir.path().join("notes.txt")).unwrap();

        let error = pipeline.reindex(&record, Mode::Apply).expect_err("should refuse");

        assert!(matches!(error, PipelineError::SourceGone { .. }), "{error}");
    }

    /// A dry run changes nothing, which on the one command that can re-read a
    /// whole archive is the difference between a look and an afternoon.
    #[test]
    fn a_dry_run_re_reads_nothing() {
        let (dir, mut pipeline) = setup();
        let wrote = pipeline
            .ingest(&write_source(dir.path(), "notes.txt", "the words"), Mode::Apply)
            .unwrap();

        let before = fs::read_to_string(&wrote.output).unwrap();
        let record = pipeline.manifest().find(&wrote.id).unwrap().unwrap();

        pipeline.reindex(&record, Mode::DryRun).unwrap();

        assert_eq!(fs::read_to_string(&wrote.output).unwrap(), before);
    }

    /// The bytes that were read are kept, so the document can be read again
    /// after the file it came from has moved or gone.
    #[test]
    fn ingesting_keeps_a_copy_of_what_was_read() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "the words");
        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();

        let record = pipeline.manifest().find(&wrote.id).unwrap().unwrap();
        let copy = pipeline.root().join(record.source_copy.expect("a copy"));

        assert!(copy.is_file(), "nothing was kept");
        assert_eq!(fs::read(&copy).unwrap(), b"the words");

        // And the point of it: the source can go, and re-reading the document
        // still has something to read. Without a copy this is the state a
        // document stays in for ever — readable, and never re-readable.
        fs::remove_file(&source).unwrap();

        let record = pipeline.manifest().find(&wrote.id).unwrap().unwrap();
        assert_eq!(pipeline.keep_source_of(&record).unwrap(), copy);
    }

    /// Two folders holding the same file are one copy, because the name is the
    /// content and not the path.
    #[test]
    fn the_same_bytes_from_two_places_are_kept_once() {
        let (dir, mut pipeline) = setup();
        let one = dir.path().join("one");
        let two = dir.path().join("two");
        fs::create_dir_all(&one).unwrap();
        fs::create_dir_all(&two).unwrap();

        let first = write_source(&one, "report.txt", "the same words");
        let second = write_source(&two, "report.txt", "the same words");

        pipeline.ingest(&first, Mode::Apply).unwrap();
        pipeline.ingest(&second, Mode::Apply).unwrap();

        let kept = fs::read_dir(pipeline.root().join(STATE_DIR).join(SOURCES_DIR))
            .unwrap()
            .count();

        assert_eq!(kept, 1, "the same bytes were kept twice");
    }

    /// A copy somebody deleted comes back the next time the source is read,
    /// which is the only self-healing this has.
    #[test]
    fn re_reading_a_source_puts_back_a_copy_that_went() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "the words");
        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();

        let record = pipeline.manifest().find(&wrote.id).unwrap().unwrap();
        let copy = pipeline.root().join(record.source_copy.expect("a copy"));
        fs::remove_file(&copy).unwrap();

        pipeline.ingest(&source, Mode::Apply).unwrap();

        assert!(copy.is_file(), "the copy was not put back");
    }

    /// The copy is named after the content, not after where it was found.
    ///
    /// Two files with the same name and different contents are two documents,
    /// and neither can be allowed to overwrite the other.
    #[test]
    fn copies_are_named_after_the_content() {
        let (dir, mut pipeline) = setup();
        let one = write_source(dir.path(), "notes.txt", "first");
        let two = dir.path().join("notes-again.txt");
        fs::write(&two, "second").unwrap();

        let a = pipeline.ingest(&one, Mode::Apply).unwrap();
        let b = pipeline.ingest(&two, Mode::Apply).unwrap();

        let copy_of = |id: &str| {
            pipeline.root().join(
                pipeline
                    .manifest()
                    .find(id)
                    .unwrap()
                    .unwrap()
                    .source_copy
                    .unwrap(),
            )
        };

        assert_ne!(copy_of(&a.id), copy_of(&b.id));
        assert_eq!(fs::read(copy_of(&a.id)).unwrap(), b"first");
        assert_eq!(fs::read(copy_of(&b.id)).unwrap(), b"second");
    }

    /// Putting a copy back means reading, not trusting.
    ///
    /// The file still named by the record is not necessarily the file that was
    /// read: somebody may have edited it, or replaced it with something else of
    /// the same name. Copying it in on the strength of its path would put
    /// different bytes under a hash that says otherwise — and that would be
    /// found years later, as a re-read producing a different document under the
    /// same id.
    #[test]
    fn a_source_that_is_not_the_bytes_of_record_is_refused() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "the words");
        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();

        let record = pipeline.manifest().find(&wrote.id).unwrap().unwrap();
        let copy = pipeline.root().join(record.source_copy.clone().expect("a copy"));

        // Both ways back are broken: the copy is gone and the file the index
        // names is no longer what was read from it.
        fs::remove_file(&copy).unwrap();
        fs::write(&source, "something else entirely").unwrap();

        let error = pipeline.keep_source_of(&record).expect_err("should refuse");

        assert!(matches!(error, PipelineError::SourceGone { .. }), "{error}");
    }

    /// Taking a document out of the index leaves everything where it is —
    /// including the copy, which is the half that is a decision rather than an
    /// oversight.
    ///
    /// The whole difference between the two ways out, and the reason one of
    /// them is offered without a confirmation: nothing is destroyed, so there
    /// is nothing to be sure about. A copy with nothing pointing at it is
    /// `reconcile`'s to report, not this's to judge.
    #[test]
    fn forgetting_leaves_the_markdown_and_the_copy_alone() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "the words");
        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();

        let record = pipeline.manifest().find(&wrote.id).unwrap().unwrap();
        let copy = pipeline.root().join(record.source_copy.clone().expect("a copy"));

        let forgotten = pipeline.forget(&wrote.id).expect("forget");

        assert_eq!(forgotten.id, wrote.id);
        assert!(wrote.output.is_file(), "the Markdown should still be here");
        assert!(copy.is_file(), "and so should the copy");
        assert!(pipeline.manifest().find(&wrote.id).unwrap().is_none());
    }

    /// The other way out takes the files with it — the Markdown **and** the
    /// copy, which is the difference between the two.
    #[test]
    fn deleting_takes_the_markdown_and_the_copy_with_it() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "the words");
        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();

        let record = pipeline.manifest().find(&wrote.id).unwrap().unwrap();
        let copy = pipeline.root().join(record.source_copy.clone().expect("a copy"));

        pipeline.delete(&wrote.id).expect("delete");

        assert!(!wrote.output.exists(), "the Markdown should be gone");
        assert!(!copy.exists(), "and so should the copy of the source");
        assert!(pipeline.manifest().find(&wrote.id).unwrap().is_none());
        assert!(source.is_file(), "but not the file somebody chose");
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
            source_copy: None,
            edited_at: None,
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

    /// The clock the edits below are saved with, so that the bytes are not a
    /// function of when the test ran.
    const AT: &str = "2026-10-12T09:00:00Z";

    /// **An edit reaches the body and nothing else.**
    ///
    /// The front matter is where the document says what it came from and the
    /// report is what the engines disagreed about. Neither is anything a person
    /// editing the words of a document should be able to damage, and neither is
    /// anywhere an editor is handed.
    #[test]
    fn an_edit_reaches_the_body_and_nothing_else() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "the words");
        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();

        let before = render::parts(&markdown_at(&wrote.output)).front.to_owned();

        pipeline
            .save_body(&wrote.id, "# somebody's heading\n\nnew words\n", AT)
            .unwrap();

        let after = markdown_at(&wrote.output);
        let split = render::parts(&after);

        assert_eq!(split.body, "# somebody's heading\n\nnew words\n");
        assert!(split.front.contains("source:"), "the provenance went with it: {after}");
        assert!(split.front.contains("sha256:"), "{after}");
        assert_eq!(split.report, "", "there was no report to damage");

        // And the only thing that changed about the front matter is the note,
        // which is not the same as the front matter it had.
        assert_eq!(
            render::edited(&before, AT).replace("edited_at: ", ""),
            split.front.replace("edited_at: ", "")
        );
    }

    /// **Saving a document nobody typed in writes the note and nothing else.**
    /// The alternative is an editor that rewrites the whole file on every save,
    /// which is a diff on a document that did not change.
    #[test]
    fn saving_the_same_words_changes_only_the_note() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "the words");
        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();

        let before = markdown_at(&wrote.output);
        let body = render::parts(&before).body.to_owned();

        pipeline.save_body(&wrote.id, &body, AT).unwrap();

        assert_eq!(
            markdown_at(&wrote.output),
            render::edited(&before, AT),
            "a save that changed nothing changed something"
        );
    }

    /// **The record says the file is no longer what the engines read**, and the
    /// words on disk are the words a search finds.
    #[test]
    fn an_edit_is_recorded_and_is_what_a_search_finds() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "the words");
        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();

        assert!(pipeline.resolve(&wrote.id).unwrap().edited_at.is_none());

        let saved = pipeline
            .save_body(&wrote.id, "somebody's own words\n", AT)
            .unwrap();

        assert_eq!(saved.edited_at.as_deref(), Some(AT));
        assert_eq!(
            pipeline.resolve(&wrote.id).unwrap().edited_at.as_deref(),
            Some(AT)
        );

        assert_eq!(pipeline.search("somebody", 10).unwrap().len(), 1, "not indexed");
        assert!(pipeline.search("the words", 10).unwrap().is_empty(), "the old words are still indexed");
    }

    /// **Reading the document again takes the mark away with the edit**, which
    /// is the whole reason the mark exists: it is what the window asks about
    /// before letting this happen.
    #[test]
    fn a_reread_writes_the_engines_words_back_and_clears_the_mark() {
        let (dir, mut pipeline) = setup();
        let source = write_source(dir.path(), "notes.txt", "the words");
        let wrote = pipeline.ingest(&source, Mode::Apply).unwrap();

        let saved = pipeline
            .save_body(&wrote.id, "somebody's own words\n", AT)
            .unwrap();

        pipeline.reindex(&saved, Mode::Apply).unwrap();

        let after = pipeline.resolve(&wrote.id).unwrap();

        assert!(after.edited_at.is_none(), "the mark outlived the edit");
        assert!(!markdown_at(&wrote.output).contains("somebody"), "the edit outlived the mark");
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
