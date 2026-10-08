# Task: P0 — Foundation

**Goal:** A working end-to-end pipeline for text-layer PDFs and Office documents,
producing a clean Markdown file tree with a SQLite manifest that guarantees
content is never processed twice. Agents must be able to use the output directly.

**Success criteria (verifiable):**

1. `docfusion add report.pdf` produces `knowledge/docs/<slug>.md` with valid YAML
   frontmatter and extracted assets under `knowledge/docs/_assets/`.
2. Running the same command twice does not reprocess — second run is a cache hit
   and the output file's mtime is unchanged.
3. Reprocessing a *modified* file overwrites the output without error (this is
   the Windows `rename` hazard).
4. `docfusion add --json` emits parseable NDJSON on stdout with clean stderr.
5. `docfusion reconcile` restores a manifest after the file tree is edited by hand.
6. A round-trip test asserts `parse(fixture) == expected_ir` for a golden corpus.

---

## Steps

### Step 1 — Workspace scaffolding
- [ ] `cargo new --lib crates/docfusion-core`
- [ ] `cargo new --lib crates/docfusion-store`
- [ ] `cargo new crates/docfusion-cli`
- [ ] Root `Cargo.toml` workspace with shared dependency versions
- [ ] `cargo build` succeeds from a clean checkout

**Verify:** `cargo build --workspace` exits 0; `cargo clippy -- -D warnings` clean.

### Step 2 — Core IR
- [ ] Define `Document`, `Page`, `Block`, `BlockKind`, `Rect`, `SourceRef`,
      `Conflict`, `EngineId` in `docfusion-core/src/ir.rs`
- [ ] `Rect` normalizes to 0..1 with `#[serde]` support
- [ ] Serialize/deserialize round-trip test
- [ ] `Rect::iou()` with unit tests (disjoint, contained, partial overlap)

**Verify:** `cargo test -p docfusion-core` passes; IoU unit tests cover the three
overlap cases plus the degenerate zero-area case.

### Step 3 — Atomic write (do this early — it is a Windows landmine)
- [ ] `write_atomic(path, bytes)` helper: write `.tmp` → fsync → replace
- [ ] Windows path uses `ReplaceFileW` (fallback `MoveFileEx`)
- [ ] Unix path uses `rename`
- [ ] Test: writing over an existing file succeeds on Windows

**Verify:** On Windows, `write_atomic` over an existing file returns `Ok` and the
content is the new content. This test must run on Windows, not just Linux CI.

### Step 4 — Store: hashing, manifest, dedup
- [ ] BLAKE3 hashing of input content
- [ ] SQLite schema for `documents`, `aliases`, `runs` (see design §9.2)
- [ ] `lookup(content_hash, engine_id, engine_version, model, prompt_version)`
- [ ] Cache-hit path: return existing record without re-extraction
- [ ] Alias handling for the same content at multiple paths
- [ ] `rusqlite` migrations or versioned schema setup

**Verify:** Integration test — ingest the same file from two paths; assert one
`documents` row, two `aliases` rows, and that the extractor was called once
(count invocations via a test double).

### Step 5 — Deterministic extractors
- [ ] Content-based MIME sniffing (not extension-based)
- [ ] `Extractor` trait + `Fit` routing
- [ ] `PdfTextLayer` via `pdfium-render`; returns `Fit::No` when no text layer
- [ ] `DocxParser` via `docx-rs`
- [ ] `XlsxParser` via `calamine`
- [ ] `PptxParser` (zip + XML, hand-rolled)
- [ ] `TextPassthrough` for `.md` / `.html` / `.txt`
- [ ] Legacy formats (`.doc`/`.xls`/`.ppt`) detected and rejected with a clear
      "convert first" error

**Verify:** Golden-corpus round-trip test — each fixture produces the expected IR.
Fixtures live in `tests/fixtures/`, expected IR as committed snapshots.

### Step 6 — Renderer: IR → Markdown
- [ ] IR → GFM Markdown with frontmatter (`source`, `sha256`, `title`, `mime`,
      `created_at`, `engines`, `conflict_count`)
- [ ] Assets extracted and written content-addressed under `_assets/`
- [ ] Slug generation for output filenames, collision handling
- [ ] Escaping of Markdown metacharacters in extracted text

**Verify:** For each fixture, rendered Markdown re-parses to an equivalent IR
(heading levels, table dimensions, list nesting preserved).

### Step 7 — CLI
- [ ] `clap`-based command surface: `add`, `show`, `list`, `reconcile`, `schema`
- [ ] Global `--json` → NDJSON on stdout, logs on stderr
- [ ] Semantic exit codes 0/1/2/3/4
- [ ] `--dry-run` on all mutating commands
- [ ] `docfusion schema` emits JSON Schema

**Verify:** Scripted test asserting stdout is valid NDJSON with `--json` while
log lines appear only on stderr; exit code assertions for each failure mode.

### Step 8 — Reconcile
- [ ] Rehash the file tree and diff against the manifest
- [ ] Report orphaned outputs (manifest row, no file) and untracked files
      (file, no manifest row)
- [ ] `--fix` repairs the manifest

**Verify:** Move a produced `.md` by hand, run `reconcile`, assert drift is
reported; run with `--fix`, assert the manifest matches the tree again.

### Step 9 — Documentation
- [ ] `README.md` with install and quickstart
- [ ] Update `docs/llmwiki/design.md` if anything shifted during implementation
- [ ] Append a dated entry to `docs/llmwiki/changelog.md`

**Verify:** A reader who has never seen the repo can go from clone to
`docfusion add` using only the README.

---

## Out of scope for P0
OCR, VLM backends, verification, GUI, search, environment detection, `bench`.
