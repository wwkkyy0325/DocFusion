# DocFusion — Design

> Status: **Draft v1** (2026-10-08)
> This is the living design document. Update it whenever the design shifts.

---

## 1. Overview

DocFusion is a local-first document ingestion pipeline. It takes arbitrary input
(PDF, images, Office files) and produces **structured Markdown** with provenance
metadata, backed by a content-addressed index that guarantees no document is
processed twice.

It ships two surfaces over one core:

| Surface | Audience | Scope |
|---|---|---|
| **CLI** | Humans (power users) + **agents** | Full capability. Every feature lives here. |
| **GUI** (Tauri) | End users | Deliberately minimal: one page. Convert one file, review, export. |

The primary consumer model is **agent-first**: the output is a clean file tree
that an LLM agent can `grep` and `read` directly. The GUI is a convenience shell
for single-file conversion, not a knowledge-base manager.

### Core value proposition

1. **Never process the same bytes twice** — BLAKE3 content addressing.
2. **Never silently lie** — every extraction is dual-verified when the input
   warrants it, and disagreements are surfaced rather than smoothed over.
3. **Never lock you in** — the knowledge base is a plain Markdown file tree.
4. **Never make you choose a model** — the engine stack is auto-selected from a
   detected hardware capability tier.

---

## 2. Non-Goals (v1)

Explicitly out of scope, so they don't creep in:

- Legacy Office formats (`.doc`, `.xls`, `.ppt`) — detect and error with a
  conversion hint.
- Web page scraping, email (`.eml`/`.msg`), EPUB, archive batch import.
- Vector / semantic search. FTS5 only, and only in P4.
- Multi-user, server deployment, auth, quotas.
- LLM semantic enrichment (summaries, entity extraction, auto-tagging).
  That is a *different project* that would consume DocFusion's output.
- OCR of handwritten text as a first-class goal.

---

## 3. Architecture

### 3.1 Workspace layout

```
DocFusion/
  crates/
    docfusion-core/     # IR, extractors, VLM backends, verification, rendering
    docfusion-store/    # manifest.sqlite, content hashing, cache, reconcile
    docfusion-cli/      # full-featured CLI (human + agent surface)
    docfusion-gui/      # Tauri shell (Windows-first, static frontend, no Node)
```

The GUI crate is created in P2, not upfront: its dependency tree is heavy and
would slow every CLI build during P0 and P1.

Rationale for a split workspace rather than feature flags in one crate: the Tauri
dependency tree is heavy (hundreds of crates). CLI users must not pay that
compile-time cost, and `docfusion-core` must stay testable in isolation against
a golden IR corpus.

A `Job` abstraction is reserved inside `docfusion-core` so that a future
daemon/queue model (single-writer, concurrent workers) can be introduced without
rewriting the pipeline or the store.

### 3.2 Data flow

```
input file
   │
   ├─> mime sniff (content-based, NOT extension)
   ├─> BLAKE3 hash
   ├─> manifest lookup ── hit (same content + same engine/model/prompt version) ──> return existing
   │        │ miss
   │        ▼
   ├─> route to Extractor
   │     ├── text-layer PDF ...... pdfium (deterministic, no model)
   │     ├── scanned/image ....... OCR engine +/or VLM engine
   │     └── docx/xlsx/pptx ...... pure-Rust parsers
   │
   ├─> IR (Vec<Block> with bbox + confidence)
   ├─> verify (on-demand second engine + arbitration)
   ├─> render ──> Markdown + YAML frontmatter + extracted assets
   ├─> atomic write (temp + ReplaceFileW on Windows)
   └─> manifest + cache update
```

---

## 4. Core IR

The IR is the project's central contract. **It is block-structured, not a
string.** Dual verification is impossible without this.

```rust
pub struct Document {
    pub source:    SourceRef,        // path, blake3, mime, size, mtime
    pub pages:     Vec<Page>,
    pub meta:      DocMeta,          // title, author, created, language
    pub conflicts: Vec<Conflict>,    // verification disagreements — first-class
}

pub struct Page {
    pub index:      u32,
    pub size:       Option<Size>,    // None for formats without pages
    pub blocks:     Vec<Block>,
    pub provenance: PageProvenance,  // engines used, whether verified
}

pub struct Block {
    pub kind:       BlockKind,       // Heading{level} | Paragraph | ListItem
                                     // | Table > TableRow > TableCell
                                     // | Figure | Formula | Code
    pub text:       String,
    pub bbox:       Option<Rect>,    // NORMALIZED to 0..1
    pub confidence: Option<f32>,     // 0..1
    pub engine:     EngineId,
    pub children:   Vec<Block>,      // rows, cells, list items
}

pub struct Conflict {
    pub page:     u32,
    pub bbox:     Option<Rect>,
    pub kind:     ConflictKind,      // TextMismatch | StructuralMismatch | Missing
    pub variants: Vec<Variant>,      // Variant { engine, text }
    pub resolved: Option<Resolution>,
}
```

### Design notes

- **`bbox` is normalized to 0..1.** Engines see different resolutions (pdfium
  works in points, a VLM in pixels). Without normalization, cross-engine
  alignment is impossible.
- **`conflicts` lives in the IR, not in a log file.** Disagreements travel with
  the artifact. This implements the governing principle below.
- **Tables nest three levels: Table → TableRow → TableCell.** An earlier
  flat cell list was not enough: a renderer cannot lay out a grid without
  knowing where rows end.

> **Governing principle:** *Specialized parsers fail loudly; general MLLMs fail
> silently.* For invoices, contracts, and anything where one wrong digit is a
> real loss, silent failure is unacceptable. DocFusion must make failure loud.

---

## 5. Extraction layer

```rust
pub trait Extractor: Send + Sync {
    fn id(&self) -> EngineId;
    fn accepts(&self, probe: &InputProbe) -> Fit;   // Yes | No | Maybe
    fn extract(&self, input: &Input, ctx: &ExtractCtx) -> Result<Document>;
}

pub trait VlmBackend: Send + Sync {
    fn id(&self) -> EngineId;
    fn capabilities(&self) -> Caps;                 // has_bbox, has_confidence, max_pages
    fn parse_page(&self, img: &Image, ctx: &PageCtx) -> Result<Page>;
}
```

`Fit` drives routing: the text-layer PDF extractor returns `No` for a scan, so
the router falls through to OCR/VLM.

`Caps` is a prerequisite check for verification. If the second engine cannot emit
bounding boxes, block-level alignment is impossible; the verifier **degrades to
text-only comparison and lowers the resulting confidence** rather than failing.
An engine that lacks a capability must never break the chain.

### 5.1 Routing rule: text-layer PDFs never touch a model

A PDF with a usable text layer is extracted deterministically via pdfium — zero
model, zero cost, zero latency, exactly reproducible. OCR/VLM is engaged **only**
for scans, images, and pathological layouts.

### 5.2 PDF engine: PDFium, and the constraint it imposes

`pdfium-render` + `pdfium-bundled`. PDFium is Chromium's PDF engine, chosen
because CJK and CID font handling is the decisive requirement for this project's
documents, and because it reports per-character bounds, font names and font
sizes — the metadata heading detection and later region alignment depend on.
Licence is MIT OR Apache-2.0, so it is safe for commercial use.

`pdfium-bundled` (not `pdfium-render`) provides the embedding: its build script
downloads the pinned Chromium build, verifies it against a SHA-256 digest, and
`include_bytes!`es it into the binary. At first use the library is extracted to a
cache directory. **The user never installs anything, and no network access is
needed at run time.** Measured cost: the embedded `pdfium.dll` is 7.4 MB.

Rejected: `mupdf-rs` is **AGPL-3.0**, which would force a closed-source
application to be open-sourced; `poppler-rs` needs a native GLib/Cairo stack that
is painful on Windows.

> **Architectural constraint — PDFium binds only once per process.**
> A second `bind_to_library` call returns
> `PdfiumLibraryBindingsAlreadyInitialized` rather than handing back the loaded
> library. The binding is therefore a process-wide singleton, not a field on an
> engine instance: two engine instances, or one instance used twice, would
> otherwise fail. Callers must also lock around extraction, because PDFium gives
> no thread-safety guarantee and pdfium-render only serialises internally when
> its optional `thread_safe` feature is enabled.

**Note on core purity.** `docfusion-core` performs no document I/O, but loading
PDFium does touch the filesystem once, to extract the embedded library. That is
a deliberate, single, bounded exception rather than a slip: it happens at the
native-library boundary, not per document.

---

## 6. Verification

### 6.1 Role matrix

The verifier is whichever engine did *not* produce the primary result. Which
engine is primary depends on the detected capability tier and configuration:

| Tier | Primary | Verifier |
|---|---|---|
| `OcrOnly` | Local OCR | — (none available) |
| `LocalDual` | Local VLM | Local OCR |
| `CloudEnhanced` | **Cloud VLM** | **Local OCR** |

**Iron rule: the verifier must be a different engine lineage.** Self-checking
with the same model is worthless — its errors are correlated and it cannot
detect its own mistakes. This is why the local stack pairs an OCR engine with a
VLM engine, rather than running one VLM twice.

**Both engines run independently.** The OCR result is never fed into the VLM as a
draft, because doing so anchors the VLM to the OCR's mistakes and destroys the
independence of the check.

### 6.2 Policy

```rust
pub struct VerifyPolicy {
    pub mode:    VerifyMode,       // Off | OnLowConfidence(f32) | Always
    pub trigger: FieldTrigger,     // regex set: amounts, dates, IDs, clause numbers
    pub arbiter: Option<EngineId>, // third engine for unresolved disagreements
    pub max_verify_pages: u32,     // hard cost ceiling
}
```

Three hard rules:

1. **Default is single-engine.** The second engine is engaged only on low
   confidence or a critical-field hit. Dual-running every page doubles both cost
   and latency for no benefit on clean documents.
2. **Conflicts are never silently dropped.** They are written to the IR and
   marked in the output.
3. **`max_verify_pages` is enforced.** A 1000-page scan must not be able to run
   up an unbounded bill.

### 6.3 Alignment algorithm

1. Both engines produce a `Page`.
2. Greedy-match blocks by **bbox IoU**.
3. Unmatched blocks on either side become `StructuralMismatch` — usually a
   reading-order error or a dropped region. **This is the most dangerous class
   of failure** and is reported first.
4. Matched blocks are normalized (whitespace, full/half-width, digit form) and
   compared; mismatches become `TextMismatch`.
5. Unresolved conflicts are surfaced to the user or routed to `arbiter`.

---

## 7. Environment detection & capability tiers

```rust
pub enum Capability {
    OcrOnly,        // cannot run a VLM
    LocalDual,      // can run the ~0.9B VLM locally
    CloudEnhanced,  // cloud API configured
}
```

Detected at install and on demand:

- GPU vendor / model / VRAM (DXGI + WMI on Windows)
- System RAM, CPU core count
- Free disk space
- Whether the Ollama sidecar is installed, running, and which models are pulled
- Reachability of the configured cloud endpoint
- WebView2 runtime version

**The user never chooses a model.** DocFusion ships exactly one recommended
local stack and selects the tier automatically. Only the cloud provider is a
configuration item. See §8.

---

## 8. Engine stack

### 8.1 Local OCR (all tiers) — RapidOCR

- **Runtime:** ONNX Runtime. **No PaddlePaddle dependency at all.**
- **Models:** PP-OCRv5 detection + recognition (~15 MB total).
- **Why:** the reported Paddle problems (deployment pain, operator bugs, poor
  Windows support) live in the *PaddlePaddle runtime*, not in the recognition
  models, which are genuinely strong for Chinese. Swapping the runtime to ONNX
  retains the accuracy and removes the pain. CPU-only, no Python.
- Chosen over Windows.Media.Ocr (too weak on layout/tables, and locks the project
  to Windows) and over "use a tiny VLM for everything" (too slow on CPU, and it
  destroys verifier independence).

### 8.2 Local VLM (mid/high tier) — via Ollama sidecar

- **Primary candidate: GLM-OCR (~0.9B, Apache-2.0 code / MIT weights).**
  Decisive property: it emits **structured output with bounding boxes**, which
  dual verification requires. Also has ready-made Ollama support and runs on
  consumer GPUs.
- **Benchmark alternative: OvisOCR2 (0.8B, Apache-2.0)** — currently top of
  OmniDocBench v1.6 (~96.6). Output interface (bbox support) still to be verified.
- **Serving:** Ollama, started as a sidecar. Chosen for Windows-native operation,
  model download/GPU allocation/VRAM management for free, and the fastest path to
  a validated end-to-end chain. Bundling llama.cpp or pure-Rust inference
  (`mistral.rs`/`candle`) remains possible later because both are just
  `VlmBackend` impls.

> **Model identity, sizes, and benchmark claims in this section were gathered
> from web sources of varying reliability and must be re-verified at
> implementation time.** Benchmark numbers across sources use different
> OmniDocBench versions and are not directly comparable.

### 8.3 Cloud (optional, configured by the user)

Cloud is a config item, not a build-time decision. Providers sit behind the same
`VlmBackend` trait.

- **Preferred cloud OCR/parse: Mistral OCR 4.** On an independent olmOCR-Bench
  run (40 hard documents) it ranked first overall (74.9%) and first among paid
  APIs, leading on tables (94.3) and reading order (73.3). **The decisive factor
  is not its rank but that it is the only leading system that returns typed
  blocks, bounding boxes, and per-word confidence.** Without bbox and confidence
  the verifier cannot align blocks, which collapses dual verification into a
  whole-document text diff — unusable given OCR noise.
- **Generic cloud VLMs** (Claude, GPT, Gemini class) remain usable as primary
  extractors or arbiters, but must be treated as `Caps { has_bbox: false }`, with
  verification degrading to text-only comparison.

### 8.4 Benchmark harness — selection is a *feature*, not a constant

`docfusion bench --set <dir>` runs every configured backend over a golden corpus
of real documents and reports accuracy / latency / cost.

Model generations turn over roughly every six months. Hard-coding a selection
means this design decays within two quarters. Because every backend is already a
trait impl, adding a new model is one `impl` block — and the choice is
re-evaluated against *the user's own documents* rather than a public leaderboard.

**Sequencing requirement:** the harness and the golden corpus are built *before*
the local VLM default is fixed. The candidates in §8.2 are starting points for a
measurement, not a decision. Nothing in P1 may hard-code a model name outside a
backend impl.

### 8.5 Evaluation corpus

The corpus lives outside the repository (real documents are often private) and is
referenced by path via `docfusion bench --set <dir>`. Because the corpus is not
versioned, **each evaluated run records its corpus hash** so that results stay
comparable across time.

Coverage the corpus must include, since these are where engines actually diverge:

- clean text-layer PDFs (baseline — note the deterministic path should win here
  at zero cost, so this measures whether the router is correct, not model quality)
- scanned documents, clean and skewed
- multi-column academic / report layouts
- tables with merged cells and multi-line cells
- Chinese documents: mixed CJK/Latin, vertical text, seals/stamps
- formulas
- low-quality scans (fax-grade, uneven lighting)

Ground truth is human-corrected Markdown. Metrics: per-block text accuracy after
normalization, table TEDS, reading-order correctness, latency, and cost per 1000
pages. Weighting is deliberate: **reading-order and table errors are weighted
above character-level noise**, because they are the failures that silently
corrupt downstream use.

---

## 9. Storage & deduplication

### 9.1 External contract: a plain file tree

```
knowledge/
  docs/
    2024-annual-report.md          # YAML frontmatter + body
    _assets/
      a3f9c2.png                   # extracted figures, content-addressed
  .docfusion/
    manifest.db                    # SQLite
    cache/                         # engine result cache
```

The knowledge base is directly consumable by Obsidian, any editor, and `git`, and
is immediately usable by an agent with no adapter.

`_assets/` is empty until P1: no deterministic engine produces figure blocks, so
there is nothing to write there yet.

### 9.2 Manifest schema

```sql
CREATE TABLE documents (
  id             TEXT PRIMARY KEY,   -- BLAKE3(content)
  source_path    TEXT,
  source_size    INTEGER,
  source_mtime   INTEGER,
  mime           TEXT,
  output_path    TEXT,               -- relative to knowledge root
  engine_id      TEXT,
  engine_version TEXT,
  model          TEXT,               -- VLM identity, nullable
  prompt_version TEXT,               -- nullable
  status         TEXT,               -- done | partial | failed
  verified       INTEGER,
  conflict_count INTEGER,
  created_at     TEXT,
  updated_at     TEXT
);

CREATE TABLE aliases (               -- same content imported from several paths
  document_id TEXT REFERENCES documents(id),
  path        TEXT,
  PRIMARY KEY (document_id, path)
);

CREATE TABLE runs (                  -- append-only audit log
  id INTEGER PRIMARY KEY,
  document_id TEXT, started_at TEXT, finished_at TEXT,
  engines TEXT, outcome TEXT, error TEXT
);
```

### 9.3 Deduplication semantics

| Situation | Behavior |
|---|---|
| Same content, different path | Same `id`; processed **once**; both paths recorded in `aliases` |
| Same path, content changed | mtime/size differ → reprocess; output overwritten; `runs` keeps history |
| Engine / model / prompt version changed | `docfusion reindex --stale` reprocesses affected documents |

`aliases` holds **every** path a given content has been seen at, including the
one in `documents.source_path`. Keeping one complete table is what lets
`reconcile` answer "which document does this file belong to?" with a single
lookup, instead of unioning two sources.

**Cache key = `blake3 + engine_id + engine_version + model + prompt_version`.**
`prompt_version` must be in the key: without it, editing a prompt silently reuses
stale results.

### 9.4 Index drift

Because the file tree is authoritative and users may edit or move files, the
index can drift. `docfusion reconcile` rehashes the tree and repairs the
manifest. This is a designed-for condition, not an error state.

Drift includes one case that is easy to miss: **editing a source file leaves the
old row behind.** Because rows are keyed by content hash, a changed file gets a
new row while the previous row keeps pointing at an output path that now holds
different content. `reconcile` must prune those rows, which is safe precisely
because `runs` retains the history.

### 9.5 Atomic writes — Windows hazard

`std::fs::rename` **fails on Windows when the destination exists** (Unix
semantics differ). Every write goes: temporary file in the destination directory
→ `sync_all` → **`MoveFileExW`** with `MOVEFILE_REPLACE_EXISTING`. Without this,
reprocessing any existing document fails on the project's primary platform.

`MoveFileExW` is used rather than `ReplaceFileW` because it covers both the
create and overwrite cases in one path; `ReplaceFileW` requires the destination
to already exist, which would add a second failure path for no benefit until
preserving destination ACLs actually matters.

The temporary file must live in the destination directory so the replace stays
within one filesystem. Parent-directory `fsync` is deliberately not performed on
Unix: index drift is already a designed-for condition (§9.4), so `reconcile`
covers a crash between replace and directory flush.

---

## 10. CLI contract

### 10.1 Commands

```
docfusion add <paths...>       Process and ingest
docfusion show <id|path>       Emit Markdown to stdout
docfusion list [--status]      List documents
docfusion reconcile            Repair the index against the file tree
docfusion reindex --stale      Reprocess after an engine/model/prompt change
docfusion bench --set <dir>    Compare backends on a golden corpus
docfusion export <id> --to <fmt>   md | html | docx | pdf | txt | json(IR)
docfusion serve                Local HTTP surface (agent-facing)
docfusion mcp                  MCP server (P4, thin wrapper)
```

### 10.2 Agent contract

Agents get the **complete** feature set. Four rules make the CLI safe to drive
programmatically:

1. **With `--json`, stdout carries NDJSON only; all logs go to stderr.** Log
   output polluting stdout is the single most common agent-integration failure.
2. **Semantic exit codes:**
   `0` success · `1` general error · `2` partial failure · `3` no text layer,
   backend must be specified · `4` index inconsistent, run `reconcile`
3. **`docfusion schema`** emits a self-describing JSON Schema so an agent does
   not need to parse help text.
4. **Every mutating command supports `--dry-run`.**

---

## 11. GUI

Tauri v2. Windows-first; Linux/macOS only if they prove cheap. The frontend is a
static directory of hand-written HTML/CSS/JS — **no Node/npm build chain**, so the
build stays `cargo build` only.

Rendering split: Rust (comrak) produces HTML; the webview does CSS + KaTeX +
Mermaid. All logic stays in Rust.

### 11.1 One page, progressive single column

Exactly one page plus a settings page. One screen state at a time:

```
Empty state                    After processing
┌──────────────────────┐      ┌──────────────────────┐
│ DocFusion        [⚙] │      │ DocFusion        [⚙] │
├──────────────────────┤      ├──────────────────────┤
│                      │      │ # 2024 Annual Report │
│   ┌──────────────┐   │      │ Revenue 1.2B, +12%   │
│   │ Drop a file  │   │      │ ┌────┬──────┬─────┐  │
│   │ or click     │   │      │ │Year│ Rev  │ YoY │  │
│   └──────────────┘   │      │ └────┴──────┴─────┘  │
│                      │      │                      │
│                      │      │ ⚠ 3 items to review  │
│ [Records]            │      │ [Copy] [Export ▾]    │
└──────────────────────┘      └──────────────────────┘
```

- **Records** and **Settings** open as drawers/modals, never as new pages.
- The GUI is a shell over the same `docfusion-core`; feature work belongs in core
  and CLI. The GUI must not become the only place a capability lives.

### 11.2 Copy behavior

The displayed Markdown is **rendered** (headings, tables, emphasis). A dedicated
**Copy** action yields **plain text with formatting stripped** — what users
actually paste into other tools. (Rendering is a view concern; copying is a
content concern. They must not be the same code path.)

### 11.3 Conflict review

The conflict panel is the GUI's main reason to exist. The CLI can report that
three blocks disagree, but only a GUI can present the two engines' outputs
side-by-side for a human to adjudicate. This is why the GUI complements rather
than duplicates the CLI.

---

## 12. Technology choices

| Concern | Choice | Notes |
|---|---|---|
| Workspace | Cargo workspace, 4 crates | CLI must not compile Tauri |
| Markdown render | `comrak` | GFM + tables; output HTML for both CLI and GUI |
| PDF text layer | `pdfium-render` + `pdfium-bundled` | Chromium engine, MIT/Apache, CJK-safe; DLL embedded, extracted to cache on first use (7.4 MB) |
| DOCX / XLSX | `docx-rs` / `calamine` | Pure Rust |
| PPTX | hand-rolled (zip + XML) | No mature Rust crate |
| Local OCR | RapidOCR models + ONNX Runtime | No PaddlePaddle |
| Local VLM | Ollama sidecar | `VlmBackend` impl |
| Hashing | BLAKE3 | Fast, content addressing |
| Manifest | SQLite (`rusqlite`) | Single file, FTS5 available for P4 |
| CLI | `clap` | Derive API, generated help |
| GUI | Tauri v2 + static frontend | No Node build chain |
| Async | `tokio` | Needed for concurrent backend calls |

---

## 13. Phased plan

Each phase is independently usable — no phase requires the next to be useful.

| Phase | Contents | Deliverable |
|---|---|---|
| **P0 Foundation** | IR, store, deterministic PDF/Office extractors, CLI (`add`/`show`/`list`/`reconcile`), atomic write | Clean file tree; **agents can use it immediately** |
| **P1 Dual verification** | **Bench harness + golden corpus first**, then `VlmBackend`, Ollama backend, RapidOCR backend, alignment, conflicts, model selection | Scans handled; disagreements visible; default local VLM chosen from measurement |
| **P2 GUI** | Tauri shell, rendering, copy-as-plain-text, conflict panel | Users can review conflicts |
| **P3 Retrieval** | SQLite FTS5 + Chinese tokenization, search UI, tags | Knowledge base for humans |
| **P4 Optional** | Vector search, MCP server, environment detection UI, daemon/queue | — |

Environment detection and capability tiers land with P1, since they determine
which engines are available.

---

## 14. Risks

| # | Risk | Mitigation |
|---|---|---|
| 1 | Windows `rename` overwrite semantics | `MoveFileExW` — fixed in P0 Step 3 |
| 2 | Ollama not installed / model not pulled | First-run guidance; explicit error; optional degrade to cloud or OCR-only |
| 3 | Tauri + WebView2 packaging size and CI | Validate early, before P2 is scoped |
| 4 | Dual verification cost blowout | On-demand triggering + `max_verify_pages` |
| 5 | Benchmark claims unreliable across sources | `docfusion bench` against the user's own corpus |
| 6 | Local VLM VRAM footprint | 0.9B Q4 ≈ 600 MB weights, but Ollama's real footprint must be measured |
| 7 | Models turn over every ~6 months | Everything behind traits; selection re-evaluated by bench |
| 8 | Verifier correlation (same lineage on both sides) | Enforced by the role matrix in §6.1 |
| 9 | PDFium binds only once per process | Process-wide singleton; see §5.2 |
| 10 | First run extracts 7.4 MB to a cache directory | Bounded and one-off; a read-only cache dir fails with a clear message rather than silently |
| 11 | The embedded PDFium build is pinned; upgrading it changes output | The PDFium build participates in the cache key, so an upgrade invalidates affected documents automatically |

---

## 15. Open questions

- Whether OvisOCR2 exposes bounding boxes (decides GLM-OCR vs OvisOCR2 as the
  local VLM default).
- Whether the local VLM should be bundled in the installer or downloaded on
  first use.
- Export format priority and whether `pdf` export needs a full layout engine
  (likely: render HTML → print path in WebView, no extra dependency).
- Chinese tokenization choice for FTS5 (`jieba-rs` vs character bigrams) — P3.
- Whether `docfusion serve` and the GUI should share one long-lived process.
