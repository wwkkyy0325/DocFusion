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
- [x] `cargo new --lib crates/docfusion-core`
- [x] `cargo new --lib crates/docfusion-store`
- [x] `cargo new crates/docfusion-cli`
- [x] Root `Cargo.toml` workspace with shared dependency versions
- [x] `cargo build` succeeds from a clean checkout

**Verify:** `cargo build --workspace` exits 0; `cargo clippy -- -D warnings` clean.

> The `docfusion-gui` crate is **not** created here. It is deferred to P2 so the
> Tauri dependency tree does not tax CLI build times during P0/P1.

### Step 2 — Core IR
- [x] Define `Document`, `Page`, `Block`, `BlockKind`, `Rect`, `SourceRef`,
      `Conflict`, `EngineId` in `docfusion-core/src/ir.rs`
- [x] `Rect` normalizes to 0..1 with `#[serde]` support
- [x] Serialize/deserialize round-trip test
- [x] `Rect::iou()` with unit tests (disjoint, contained, partial overlap)

**Verify:** `cargo test -p docfusion-core` passes; IoU unit tests cover the three
overlap cases plus the degenerate zero-area case.

### Step 3 — Atomic write (do this early — it is a Windows landmine)
- [x] `write_atomic(path, bytes)` helper: write `.tmp` → fsync → replace
- [x] Windows path uses `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`
- [x] Unix path uses `rename`
- [x] Test: writing over an existing file succeeds on Windows
- [x] Test: a failed write leaves no temporary file behind

**Verify:** On Windows, `write_atomic` over an existing file returns `Ok` and the
content is the new content. This test must run on Windows, not just Linux CI.

> `ReplaceFileW` was dropped in favour of `MoveFileExW`: it requires the
> destination to exist, so it needs a fallback path for new files, while
> `MoveFileExW` handles both cases in one step.

### Step 4 — Store: hashing, manifest, dedup
- [x] BLAKE3 hashing of input content
- [x] SQLite schema for `documents`, `aliases`, `runs` (see design §9.2)
- [x] `lookup(content_hash, engine_id, engine_version, model, prompt_version)`
- [x] Cache-hit path: return existing record without re-extraction
- [x] Alias handling for the same content at multiple paths
- [x] `rusqlite` migrations or versioned schema setup

> `runs` is created but has no accessors yet — nothing writes to it until the
> CLI records pipeline history in Step 7. Hashing streams through a fixed buffer
> rather than loading the file, since documents routinely run to hundreds of
> megabytes.

**Verify:** Integration test — ingest the same file from two paths; assert one
`documents` row, two `aliases` rows, and that the extractor was called once
(count invocations via a test double).

### Step 5 — Deterministic extractors
- [x] Content-based MIME sniffing (not extension-based)
- [x] `Extractor` trait + `Fit` routing, with fall-through when an engine declines
- [x] `TextPassthrough` for `.md` / `.txt`
- [x] Legacy formats (`.doc`/`.xls`/`.ppt`) detected and rejected with a clear
      "convert first" error
- [x] `PdfTextLayer`; returns `Unsupported` when there is no text layer
- [x] `DocxParser` (hand-rolled over a shared OOXML layer)
- [x] `XlsxParser` via `calamine`
- [x] `PptxParser` (hand-rolled over the shared OOXML layer)
- [ ] `HtmlParser` for `.html`

> **Deferred from this step:** HTML needs a real HTML→Markdown conversion
> (link targets, image sources, base URLs); a regex-based strip would produce
> output that looks right and is subtly wrong. It is a separate decision.
>
> **PDF engine decided:** `pdfium-render` + `pdfium-bundled` (Chromium's engine,
> MIT/Apache). The library is downloaded and digest-verified at build time, then
> embedded, so users install nothing and need no network at run time. See design
> §5.2, including the constraint that PDFium may be bound only once per process.

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
