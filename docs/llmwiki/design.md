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
                                     // | Quote | Figure | Formula | Code
    pub text:       String,
    pub bbox:       Option<Rect>,    // NORMALIZED to 0..1
    pub confidence: Option<f32>,     // 0..1
    pub engine:     EngineId,
    pub children:   Vec<Block>,      // rows, cells, list items
}

pub struct Conflict {
    pub page:     u32,
    pub bbox:     Option<Rect>,
    pub kind:     ConflictKind,      // TextMismatch | StructuralMismatch
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

### 5.3 Text inputs are read for what they are

A `.md` file is parsed as Markdown; a `.txt` file is read as plain text. Reading
plain text as Markdown would turn `a *b* c` into emphasis the author never wrote,
and turn a line of dashes into a list.

The file name is used for this and nowhere else. That is not a contradiction of
the rule in §5: the name decides **how to read text already known to be text**,
which is a different question from *what the file is* — that was settled by
sniffing the bytes.

Reading Markdown line by line instead of parsing it costs real fidelity, and the
benchmark is what caught it: a note containing a list was flattened into a
paragraph whose leading dash was then escaped, so the list came out as text that
merely looked like a list. The round trip now scores zero loss.

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
    pub mode:    VerifyMode,       // Off | OnDoubt | OnCriticalFields | Always
    pub below:   f32,              // confidence under which the primary is unsure
    pub trigger: FieldTrigger,     // regex set: amounts, dates, IDs, clause numbers
    pub max_verify_pages: u32,     // hard cost ceiling
}
```

Four policies rather than three. The earlier `mode`-plus-`trigger` split left
"verify on a critical field" expressible only as a confidence threshold that
happens never to fire, which is a way of writing a decision that nobody can
read back. The four are named after what a user would ask for.

Three hard rules:

1. **Default is single-engine.** The second engine is engaged only on low
   confidence or a critical-field hit. Dual-running every page doubles both cost
   and latency for no benefit on clean documents.
2. **Conflicts are never silently dropped.** They are written to the IR and
   marked in the output.
3. **`max_verify_pages` is enforced.** A 1000-page scan must not be able to run
   up an unbounded bill. It is counted per document, so one enormous scan cannot
   exhaust a budget the rest of the batch needed.

**The policy is part of the cache key.** It decides what the output says — which
pages carry conflicts, which are marked verified — so leaving it out would serve
a cached single-engine result to a run that asked for a second opinion.

**Arbitration is deferred.** `Resolution` and `ResolutionSource::Arbiter` are in
the IR for it, but a third engine has to be a third *lineage*, and the stack has
two. Shipping the field before there is anything that could fill it would be a
setting that silently does nothing.

### 6.3 Alignment algorithm

1. Both engines produce a `Page`.
2. Greedy-match blocks by **bbox IoU**, one to one.
3. Unmatched blocks on either side become `StructuralMismatch` — usually a
   reading-order error or a dropped region. **This is the most dangerous class
   of failure** and is reported first.
4. Matched blocks are normalized (whitespace, full/half-width) and compared;
   mismatches become `TextMismatch`.
5. Unresolved conflicts are surfaced in the output.

**When geometry is missing on either side, the comparison falls back to the
whole page.** This is not an edge case — it is the path today's stack always
takes, because a vision model answers with Markdown and no coordinates, and
alignment by position is impossible when only one engine has positions.
Inventing one would attribute a disagreement to the wrong region.

The fallback is coarser, not weaker. Two engines that produce the same
characters for a page have agreed about it, whatever they called the blocks, and
a dropped region still changes the character stream. What is lost is the ability
to say *where*, so those conflicts are recorded with no region rather than a
guessed one.

**Normalization folds whitespace away entirely, and nothing else.** Spacing is
much the largest source of noise between a recognition model and a language
model, and it is not a disagreement about the document. Case, punctuation and
character identity are exactly what the comparison is for; a tolerance wide
enough to hide a wrong digit would defeat the purpose.

### 6.4 What the reader sees

A conflict is worthless if it stays in the IR. The rendered Markdown carries it:

- Front matter gains `verified: <checked>/<total>`. A page nobody checked and a
  page that passed are different things, and the ceiling means a long scan can
  be partly one and partly the other.
- A `## Conflicts` section lists each disagreement with both readings verbatim,
  in an indented block so that extracted text cannot become structure.
- No reading is marked preferred. The tool has no basis for choosing, and a
  report that quietly picked would be the silent resolution this exists to
  prevent.

---

## 7. Capability, and what it now means

```rust
pub enum Capability {
    /// No vision endpoint is configured. Deterministic formats and OCR still work.
    OcrOnly,
    /// A vision endpoint answered.
    Vision,
}
```

Decided by **asking rather than detecting**. The configured endpoint is probed
with a cheap request, and the answer decides the tier.

Probing replaces inspecting, and that is the point of §8.2 arriving where it
did: GPU vendor, VRAM, whether a daemon is installed and which models it is
holding are all questions about *somebody else's program*, and that program
answers them better than any inspection we could write. What used to be an
environment-detection subsystem is now one request and an error message.

What is still worth checking locally, because no endpoint can report it:

- Free disk space for the knowledge base.
- The WebView2 runtime version, for the GUI.

**Both are deliberately not checked yet**, and each belongs with the thing that
needs it. Free space guards against a risk that markdown output — kilobytes per
document, against a 6.9 MB one-time extraction — does not pose, and would cost a
platform-specific dependency to report. WebView2 is for a GUI that does not
exist. A health check that reports everything it *could* measure is longer than
one that reports what matters.

**The `Capability` enum was not needed.** It was designed as a type, but nothing
branches on it: the answer is a report, and the tier is a field in it. `doctor`
reports `vision` only when a configured endpoint answered *and* listed the
configured model, and `ocr-only` otherwise — including when the endpoint is
configured but unconfirmed, because "vision" would then be a promise the run
just failed to verify. The reason is printed beside it either way.

**The user chooses the model; the tool reports what it found.** An earlier
version of this design had the tool ship one recommended stack and pick a tier
automatically. That is the right shape for a product that manages inference, and
the wrong one for a document pipeline that does not.

**A missing model listing is not a fault.** Nothing requires an endpoint to
implement `/v1/models`, and one that reads pages perfectly well without it must
not be reported as broken. `Probe` therefore separates "answered, has no
listing" from "did not answer": the first is a note, the second a problem and a
non-zero exit. An answer that always says "fine" is not an answer, and an answer
that always says "broken" is not one either.

---

## 8. Engine stack

### 8.1 Local OCR (all tiers) — RapidOCR on ONNX Runtime

- **Runtime:** ONNX Runtime. **No PaddlePaddle dependency at all.**
- **Models:** PP-OCRv6 tiny — detector, recogniser, angle classifier and
  dictionary. **Measured at 6.9 MB**, which is less than half the earlier
  estimate and settles the question of whether to ship them.
- **Why:** the reported Paddle problems (deployment pain, operator bugs, poor
  Windows support) live in the *PaddlePaddle runtime*, not in the recognition
  models, which are genuinely strong for Chinese. Swapping the runtime to ONNX
  retains the accuracy and removes the pain. CPU-only, no Python.
- Chosen over Windows.Media.Ocr (too weak on layout/tables, and locks the
  project to Windows) and over "use a tiny VLM for everything" (too slow on
  CPU, and it destroys verifier independence).

**This engine reports geometry and confidence**, unlike a general vision model,
which is what qualifies it to be the verifier in §6.1.

**The weights are embedded in the binary.** They are fetched and digest-verified
at build time and `include_bytes!`d, then written to a per-user location on the
first page that needs them. That makes "works with no setup" true in the strong
sense — offline from install, nothing to configure, nothing to download at run
time — and it is affordable precisely because the set measured small.

Two details that only matter because they were measured rather than assumed:

- **Extraction is lazy.** Loading the weights takes seconds; `list`, `show` and
  `schema` never read a page and so must not pay for a pipeline they do not use.
- **The pins live in two places** — the build script and `rapidocr-core` —
  because a build script cannot depend on the crate it is embedding for. A test
  compares them, so an upstream update fails loudly instead of silently
  embedding weights the crate no longer expects.

### 8.2 The vision backend is the user's endpoint, not ours

**DocFusion does not run models.** It speaks to an OpenAI-compatible HTTP
endpoint that the user points it at, and works with whatever is behind it.

This is a deliberate reduction, taken after two earlier designs — an Ollama
sidecar, then a llama.cpp sidecar — both made the same mistake. Each turned
DocFusion into a model runtime as well as a document pipeline, which meant
writing a downloader, a checksum verifier, a process supervisor, a port
allocator and Windows job objects. None of that is what this project is for, and
model runtimes are an entire product category with their own users.

The reduction is affordable because **`llama-server`, Ollama, LM Studio and vLLM
all expose the same OpenAI-compatible API**. One client covers every local
option and a cloud provider besides, so the choice of runtime stops being ours
to make and starts being a configuration value.

| Configured | What happens |
|---|---|
| Nothing | OCR only. Every deterministic format still works, and scans are read by the local OCR engine — so the tool is useful with no setup at all. |
| An endpoint | The vision backend joins, and dual verification becomes possible in the shape §6.1 describes. |

**Setup is a document, not a feature.** A guide for the user — or, more to the
point, for their *agent* — is smaller, more durable and more flexible than a
bundled installer that pins one model forever. It has to be executable rather
than suggestive: exact commands, exact file names, and a way to check it worked.
Models turn over every few months; a document can be updated in a paragraph.

**What the tool still owes the user**, given it no longer controls the backend:

- **Say what is missing, and how to fix it.** An unconfigured endpoint must
  produce a message naming the guide, not a connection error.
- **Never hang.** An endpoint that accepts a connection and then stalls has to
  time out, or one bad configuration freezes a batch.
- **Watch for the projector failure.** A GGUF model served without its `mmproj`
  does not fail: the image never becomes vision tokens and the model returns
  repeated garbage with a 200 status. An implausibly low prompt token count is
  an error, not a result.
- **Probe rather than detect.** Whether a backend is usable is answered by
  asking it, not by inspecting the GPU — which is another thing that stops being
  our problem.

> **Which model is best remains an open question, and it is now the user's to
> answer with our benchmark.** The candidates worth measuring, in roughly the
> order current evidence favours: PaddleOCR-VL (~0.9B, official GGUF, layout
> analysis), GLM-OCR (~0.9B, structured output with bounding boxes — the
> property dual verification wants most), OvisOCR2 (0.8B), dots.ocr (1.7B).
> `docfusion bench` exists so this can be settled on the user's own documents
> rather than on a leaderboard.

### 8.3 Cloud is the same client

A hosted provider is not a separate subsystem. It is the endpoint from §8.2 with
a different address and a key, which is why §8.2 made the endpoint the unit of
configuration rather than the runtime.

What differs is **what the endpoint can report**, and one property matters
enough to name:

- **Prefer a provider that returns typed blocks, bounding boxes and per-word
  confidence over one that returns only text.** Without geometry the verifier
  has nothing to align two engines' output on, and verification collapses into a
  whole-document text comparison that OCR noise makes useless. On an independent
  olmOCR-Bench run, Mistral's OCR models were the only leading system reporting
  it alongside top accuracy — but the property is what counts, not the vendor.
- **General hosted vision models report neither.** They are declared as
  `Caps { bounding_boxes: false, confidence: false }`, so the verifier degrades
  honestly instead of pretending to align on boxes that do not exist.

Neither is a build-time decision. Both are a URL and a key.

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

A corpus is **one directory per case**, each holding `expected.md` beside the
document it should come from:

```
corpus/
  annual-report/
    report.pdf          # any name, any extension
    expected.md         # what the output should be
```

A directory per case rather than a flat pile of same-stemmed files, because the
pairing has to survive a source that is itself Markdown: `notes.md` beside
`notes.md` is not a pairing, it is one file.

The corpus lives outside the repository (real documents are often private) and is
referenced by path via `docfusion bench --set <dir>`.

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

#### Two error rates, not one

**The single rate was wrong, and the first real corpus proved it.** Comparing the
output to the reference as one ordered string conflates two failures that need
different answers. A line-based recognition engine given a three-column newspaper
reads *across* the columns instead of *down* them: it produced 97.7% of the
reference's characters and scored a character error of 0.978. Read as "the engine
is broken", that number is false; read as "the engine cannot order a page", it is
correct and actionable.

`Score` therefore carries both:

- `character_error_rate` — reading order included. What a reader experiences.
- `content_error_rate` — the same characters, order-free. What survived.

**The gap between them is the damage ordering did**, and it is the first thing to
look at when a page reads well but scores badly. Both are reported; neither is
folded into the other.

#### A baseline, not a verdict

The first corpus is 24 pages sampled from OmniDocBench v1.5 — the base subset
only, since the `equation_hard`, `layout_hard` and `table_hard` subsets are
adversarial by construction and say little about ordinary documents.

Three limits are known and are **not** corrected for, because correcting them
would hide them:

1. **The reference is ours.** OmniDocBench ships per-region annotations, not
   Markdown, so `expected.md` is generated by a converter whose rules decide what
   counts as a correct reading. It was validated by looking at pages beside their
   references; it is still a converter, and the rules are in the script rather
   than implied.
2. **Formulas are contaminated.** The reference carries the dataset's own LaTeX,
   spaced as `0 . 1 6 6 \%`. No engine will reproduce that notation, so on
   math-heavy pages both rates measure notation rather than reading. 14 of the 24
   cases contain LaTeX; they are the worst-scoring ones and should be read with
   that in mind.
3. **The pages are born-digital, rendered to images.** No skew, no phone-camera
   noise, no uneven lighting — so every number here is optimistic about
   real-world scans.

#### The first baseline: local OCR against a cloud VLM

24 pages, both engines run over the same corpus. `order` is the character error
in reading order, `content` the same with order taken out.

| | local RapidOCR | DeepSeek-V4.1-Flash |
|---|---|---|
| mean error, in reading order | 0.466 | **0.245** |
| mean error, ignoring order | 0.203 | **0.171** |
| headings found | 0 / 64 | **9 / 64** |
| tables found with the right shape | 0 / 9 | **7 / 9** |
| slowest page | 56 s | 74 s |

**They are close on characters and not close on structure.** A line recogniser
has no concept of a heading or a table, so those two columns are structural
rather than measured — but a table read as a run of prose is genuinely worse for
a knowledge base than the same table read as a table, and that is the difference
the VLM buys.

**And they fail on different pages, which is the whole argument for two.**
Case 081, a Chinese newspaper page: the local engine recovered 97.7% of the
reference's characters; the VLM recovered **72.8%**, cleanly omitting the
masthead, the editor, the proofreader and the phone number. The cause is
arithmetic rather than quality — the provider converts a page to at most ~1024
image tokens, which downsizes a 2754×3939 scan to roughly 900×1280, and fine
print does not survive that. The local engine runs at full resolution and does
not care. On the same corpus the VLM's output on that page carries *more* of the
page's structure and *less* of its content, which is exactly the shape of
disagreement the verifier exists to catch.

**Two findings this produced in code, both from running rather than from tests:**

- **The VLM wrapped whole pages in a fenced block with a preamble** ("Here is
  the transcription of the page in GitHub-flavoured Markdown." followed by a
  fence). Left in place, the page became one verbatim code block and every
  heading and table was lost. `endpoint::strip_envelope` unwraps a single fenced
  block, with or without one introducing sentence, and only that.
- **The scorer could not see inside a code block.** A fenced block's content is
  a literal rather than a text node, so a correctly read page returned inside a
  fence scored a content error of **1.000** — read as "the engine returned
  nothing" when it had returned everything. Between the two, cases 011/012 went
  from 1.000 to 0.089 and the corpus mean from 0.292 to 0.245.

**Still open, raised by the data:** when the verifier recovers text the primary
dropped, the primary's reading is still what the document contains; the conflict
is recorded but nothing is resolved. Deferred arbitration covers this, but the
geometry-free comparison also cannot classify a dropped region as the dangerous
class it is, because it cannot localise it.

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
  updated_at     TEXT,
  output_hash    TEXT               -- BLAKE3 of the rendered Markdown
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

`reconcile` distinguishes three situations, and only two of them are drift:

| Situation | Meaning | Repair |
|---|---|---|
| The output file is gone | The index is stale | **Forget** the row; re-adding the source rebuilds it |
| The output is elsewhere, with the same content | The user moved the file | **Relink** the row to its new path |
| A file the index does not claim | Maybe a person's own notes | Nothing — the index claims nothing about it, so it cannot be wrong about it |

Relinking is why `output_hash` exists: a rename breaks the path link even though
the document itself never changed, and hashing the tree is the only way to
recognise it. A record written before that column existed is reported as missing
rather than guessed at.

**A cache hit also checks that its output still exists.** Reporting a hit for a
file somebody deleted would hand back a path to nothing and skip the write that
would put it back, which is a worse failure than redoing the work.

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

**The setup guide is `docs/setup.md`**, outside the knowledge base: it is a
product deliverable with a different reader from these notes. Every command in
it was run before it was written down, and it states which parts are a vendor's
documented commands rather than something that could be verified here.


### 10.1 Commands

```
docfusion add <paths...>       Process and ingest
docfusion show <id|path>       Emit Markdown to stdout
docfusion list [--status]      List documents
docfusion reconcile            Repair the index against the file tree
docfusion reindex --stale      Reprocess after an engine/model/prompt change
docfusion bench --set <dir>    Compare backends on a golden corpus
docfusion doctor               What this installation can read, and what is missing
docfusion export <id> --to <fmt>   md | html | docx | pdf | txt | json(IR)
docfusion serve                Local HTTP surface (agent-facing)
docfusion mcp                  MCP server (P4, thin wrapper)
```

### 10.2 Configuration

Settings that would otherwise be retyped on every invocation live in one file,
per user and outside any repository:

```
%LOCALAPPDATA%\DocFusion\config.toml      # or $HOME/.cache/docfusion on unix
```

```toml
endpoint = "https://api.deepseek.com"
model    = "deepseek-flash"
api_key  = "sk-..."
```

**Precedence: flag → environment variable → file → built-in default.** Clap
resolves the first two into one `Option`, so whatever it left unset is exactly
what the file is allowed to supply. That is why the settings with defaults
(`root`, `verify`, `max_verify_pages`) are `Option` in the parser rather than
carrying their defaults in an attribute — a default applied by the parser could
never be overridden by the file.

**Three deliberate choices:**

- **Per user, not per knowledge base.** It holds an API key. A file beside the
  documents is a file that eventually gets committed. It also shares a directory
  with the extracted weights, so "where does DocFusion put things" has one
  answer.
- **A misspelt key is an error, not a shrug.** `deny_unknown_fields` makes
  `api_ky = "..."` fail with the file, the line and the list of valid keys. A
  setting silently ignored means running against the wrong endpoint with nothing
  to say why — the quiet wrong answer this project exists to avoid.
- **`--json` and `--dry-run` are not settable here.** They describe what *this
  run* should do rather than how the tool is set up, and a file that quietly
  made every run a dry run would be a trap rather than a convenience.

`docfusion schema` publishes the path along with the keys, because an agent
should not have to guess where the settings are.

**The window writes this same file — see §11.3.** There is no second place for
settings to live, because two places are two answers and the command line reads
this one.

### 10.3 Agent contract

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
5. **Configuration is flags or environment variables**, and both are documented
   by `docfusion schema`: `--endpoint`, `--model`, `--api-key`, and
   `DOCFUSION_ENDPOINT`, `DOCFUSION_MODEL`, `DOCFUSION_API_KEY` for the same.
   An agent driving the tool should never have to read a file to find out how it
   is configured, because it cannot guess where that file is.

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
Document tab                   Records tab
┌──────────────────────┐      ┌──────────────────────┐
│ DocFusion            │      │ DocFusion            │
│ [Document] Records   │      │ Document [Records]   │
│        [+ Add]       │      │        [+ Add]       │
├──────────────────────┤      ├──────────────────────┤
│ # 2024 Annual Report │      │ annual report.pdf    │
│ Revenue 1.2B, +12%   │      │ done · 2 to review   │
│ ┌────┬──────┬─────┐  │      │                      │
│ │Year│ Rev  │ YoY │  │      │ scan-0007.png        │
│ └────┴──────┴─────┘  │      │ done                 │
├──────────────────────┤      │                      │
│ rapidocr · ⚠ 3  [Copy]│     │                      │
└──────────────────────┘      └──────────────────────┘
```

- **The two views are tabs, not a drawer.** A drawer is an overlay: it covers the
  document, it has to be dismissed before the document can be read, and on this
  window it turned out to be the single most broken thing in it (see below). A
  tab is a sibling. Switching costs one `hidden` attribute and the layout does
  not move.
- The GUI is a shell over the same `docfusion-core`; feature work belongs in core
  and CLI. The GUI must not become the only place a capability lives.

**`hidden` loses to any `display` you write.** The drawer shipped unable to
close: `aside { display: flex }` overrides the user-agent rule that `hidden`
relies on, so the attribute was set and the panel stayed pinned open. This is not
a mistake about Tauri or about CSS in general — it is a mistake about *which*
rule wins, and the fix is one line for every element rather than a rule per
element: `[hidden] { display: none !important; }`. Anything hidden in this
frontend is hidden through that.

**The window draws its own waiting state.** The other side sets a `Working` state
the moment a read starts, but that state only reaches the frontend when the call
returns — and the call is the thing that takes minutes. So the frontend draws the
waiting screen before it asks for the work, with a clock that counts up. A window
that says nothing for the length of a scan is indistinguishable from one that has
stopped, which is exactly the complaint this fixed. The clock is not decoration:
it is the evidence that the process is alive.

**`docfusion-gui` is Tauri v2 with a static frontend and no build chain.** The
`ui/` folder is HTML, CSS and one JavaScript file, loaded directly — no bundler,
no `node_modules`, no npm step in the build. The design chose that before there
was any code, and building it confirmed the choice: the only genuinely hard part
of this interface is rendering Markdown, and that is `comrak` in Rust
(`render::html`), because the command line needs it anyway. What is left for the
frontend is a view over a JSON value and two things a webview is better at than
Rust — the clipboard and knowing what is selected.

**`state.rs` is plain data with no window types in it**, and it is where the
tests are. `bridge.rs`'s job — the two directions between the window and the
pipeline — lives in `lib.rs`. This is the shape `verse-app` uses, and it is the
right one: the part that decides what the interface looks like can be checked
without starting a window.

**The engines come from `docfusion_store::engines::standard`**, the same call
the command line makes. Two lists that agree today are two lists that disagree
after the next change, and a window that reads a document differently from the
command line is two products with one name.

**The GUI links the crates, and there is no server behind it.** Tauri is a Rust
application whose webview is a view over the same process: the interface calls
into `docfusion-core` and `docfusion-store` directly, as the command line does.
There is no HTTP surface between the two halves.

**Which means there is no port, and so nothing to collide.** Port conflicts are
the tax a desktop application pays for being two programs rather than one —
a fixed port that a second copy wants, a chosen port somebody else already has,
a firewall rule for a listener the user never asked for. None of it arises here,
and it is worth naming as a *reason* rather than an accident: a `docfusion
serve` would have reintroduced all three for a capability the CLI already
provides, which is why §10.2 can call the command line the complete agent
surface.

Should a long-running mode ever be needed — watching a directory, say, which is
P4 — that is a service in the true sense, and it would need a port, a port
policy, and a story for a second copy. It would not be this GUI growing a
backend.

### 11.2 Settings

A third tab, three fields, one button: endpoint, model, key. It exists so that
pointing the tool at a model does not mean opening a TOML file — which is what
it meant until now, and is why the endpoint was configured once and never
changed.

**It writes the same file the command line reads** (§10.2). A settings page with
its own store would be two answers to one question, and the one that loses is
the one the CLI acts on.

**Edited in place, not rewritten.** Most of that file is the comments explaining
it, and a save that deleted them would delete them silently. `toml_edit` changes
the three keys and leaves everything else — including the note somebody wrote at
the end of the line being changed, which has to be carried over by hand because
assigning a value takes the trivia around it as well.

**The key goes one way.** It is typed in the window and written to the file, and
nothing reads it back: the view carries `has_key` and never the key, so there is
nothing for a form to show and nothing for a screenshot to keep. That has one
consequence worth stating — an empty key box means *I did not touch this*, not
*delete it*, because a form that re-sent the key on every save would be a form
that needs the key.

**Turning the model off is clearing the endpoint.** `endpoint` and `model` are
set together or not at all, since one endpoint may serve several models and
guessing would silently read documents with whatever it happened to be serving.
Clearing both puts the tool back on the bundled engine, and the key that stays
behind in the file does nothing.

**Saving takes effect immediately**, which is why the endpoint is behind a lock
in the window's state. A settings page whose settings apply after a restart is
not a settings page.

### 11.3 Language

The interface is in Chinese and English, and it opens in whichever one the
machine is set to. Nothing has to be configured: `navigator.language` on a
Chinese Windows is already `zh-CN`. A button in the header switches, and the
choice is remembered.

**The window sends the kind of a failure, not the sentence about it.** This is
the whole reason the interface can be translated at all, and it is the part
worth writing down. A `thiserror` message is English, and it is the string the
command line prints. A window that had to show it in Chinese could only match on
the sentence — which comes apart the moment somebody rewords it, silently, in a
place nobody looks. So a failure crosses the boundary in two halves:

- **`cause`** — the kind: nothing can read this, an engine failed, a file could
  not be read, the index is not usable, and so on. `Cause` in `state.rs` is a
  closed enum, `cause_of` in `lib.rs` maps the library's error to it with an
  exhaustive `match`, and the frontend holds one sentence per kind. A new way
  for the library to fail stops `cause_of` compiling, which is the window being
  made to decide how to say it rather than finding out later that it has nothing
  to say.
- **`detail`** — the account the engine or the operating system gave, carried
  through untouched.

**Which leaves one thing in English, and it is not laziness.** `detail` is
somebody else's words: `reqwest` said the connection was refused, the OS said
`os error 10061`, SQLite said what was wrong with the query. Translating those
would mean guessing at what a third-party crate meant and writing the guess in
Chinese — inventing an explanation instead of reporting one. It is shown as it
arrived, set apart so it reads as a quotation rather than as our own sentence.

The command line is untouched by all of this and stays English. It is the agent
surface (§10.2), and an agent reading a `Display` message is exactly the case
the message is written for.

### 11.4 Appearance

**Dark, and light if asked.** Not "follow the system": this interface has one
appearance decision to make and dark is the answer to it, so there is no third
state to explain, no query to the system, and no way for the window to open in
one theme and change its mind a moment later.

**The button is a sun or a moon, and it says what pressing it gives you** — a
sun while the window is dark — which is the rule the language button follows
too, because the thing worth saying is the one you cannot see. Both are drawn
inline as stroked `<svg>`: no icon font, no library, no emoji. Emoji were ruled
out by eye before anything else — `☀` is one codepoint away from being rendered
in colour by the system font, which is not a thing a button can decide.

**A wordless button still needs words.** They go in `title` and `aria-label`,
translated like everything else, because an icon has no accessible name of its
own and "sun" is not the name — "switch to the light theme" is.

**`hidden` does not work on an `<svg>`.** This cost an afternoon and belongs
here rather than in a commit message. `hidden` is an IDL property of
`HTMLElement`; `SVGElement` has no such property, so `svg.hidden = true` sets an
ordinary JavaScript field on the object and never writes the attribute. The
stylesheet's `[hidden]` therefore never matches, both shapes stay on screen, and
reading `svg.hidden` back says `true` — so a check that asks the code what it
thinks it did is told that everything is fine. Use `toggleAttribute("hidden",
…)`, and check the attribute rather than the property when looking.

**One palette, both themes, side by side.** Every colour is declared once as
`light-dark(light, dark)`, and a single `color-scheme` on the root element
decides which of the pair is used. The alternative — two blocks of declarations,
one inside a media query — is the arrangement where somebody changes a colour in
one and not the other and the themes quietly stop matching. Written this way the
two values are on the same line, which is the only place they can be compared.

**`color-scheme` is doing a second job.** It tells the webview what kind of
controls to draw, and the scrollbars, the text cursor, the password reveal
button and the autofill background are all the browser's rather than ours. A
dark page that claims to be light gets all of them wrong, and no amount of
`background-color` fixes it.

**The window is darker than the page it is about to draw.** `backgroundColor` in
`tauri.conf.json` is the same value as `--paper` in the dark theme, which is a
duplication — the window exists for a moment before the webview paints, and that
moment is white unless it is told otherwise. It is written down here because it
is the one colour in two places.

**Links in a document were the one thing that did not survive.** They had no
rule at all until now, so they kept the browser's default, which is a dark blue
chosen for a white page. On a dark one it is very nearly invisible.

Renumbering follows: the settings tab is §11.2, language §11.3, appearance
§11.4, and copy behaviour §11.5.

### 11.5 Copy behavior

The displayed Markdown is **rendered** (headings, tables, emphasis). A dedicated
**Copy** action yields **plain text with formatting stripped** — what users
actually paste into other tools. (Rendering is a view concern; copying is a
content concern. They must not be the same code path.)

### 11.6 Conflict review

The conflict panel is the GUI's main reason to exist. The CLI can report that
three blocks disagree, but only a GUI can put the two engines' outputs beside
each other for a person to adjudicate.

**In the file they are one thing; on the screen they are two.** The Markdown
ends with a `## Conflicts` section, and that is right for a file: a disagreement
has to travel with the document it is about, and an agent reading the document
should see the doubt as well as the text. On a screen it is wrong — a report
about a document, sitting inside the document, reads as though somebody had
written it into the page. So the window splits the Markdown at that heading and
draws the two parts apart.

**Nothing in the body points at the conflict.** That is still true and is still
a gap: the window can say "one place" and take you to the report, but it cannot
show you *where on the page* the engines parted company, because the Markdown
carries no marker at that position. Closing it means the renderer emitting
something at the block, which is a change to the artifact rather than to the
window — the marker would end up in the text a pipeline indexes. Worth doing,
not done.

**Neither reading is presented as the right one.** The tool has no basis for
preferring either, which is exactly why it reports the disagreement rather than
settling it, and a panel that quietly picked a side would be the silent
resolution this feature exists to prevent. Both are shown as each engine wrote
them, labelled with what the engine is called here and what it is called
everywhere else — `openai-compatible` is a protocol, not a name for anything a
person can picture.

**Each reading is capped in height and scrolls inside itself.** A disagreement
about a whole page is two pages of text, and nobody can compare two pages on a
screen. The cap is what keeps the panel usable; the scroll is what keeps the
whole reading there for whoever wants it.

**The two are side by side, which is the entire point.** A reading-order failure
— the common one, and the one the local engine makes on multi-column pages — is
indistinguishable from a good reading until the two are next to each other.

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
| Vision backend | The user's OpenAI-compatible endpoint | `llama-server`, Ollama, LM Studio, vLLM or a cloud API — one client covers all of them. Not ours to run. |
| HTTP client | `ureq` | Blocking and small. The pipeline is synchronous, so an async runtime would be weight carried for nothing. |
| OCR pipeline | `rapidocr-core` on ONNX Runtime | Detection, classification, recognition and their post-processing. Writing DBNet contour finding and CTC decoding by hand would be well-trodden work containing no decision of ours. |
| Hashing | BLAKE3 | Fast, content addressing |
| Manifest | SQLite (`rusqlite`) | Single file, FTS5 available for P4 |
| CLI | `clap` | Derive API, generated help |
| Field trigger | `regex` | The critical-field patterns. Four hand-rolled matchers would be more code, less capable, and harder to extend; the comparison itself folds width and case by hand, because a regex per character would be absurd. |
| GUI | Tauri v2 + static frontend | No Node build chain. The `ui/` folder is loaded as it is; Markdown becomes HTML in Rust, so there is nothing for a bundler to do. |
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
| 2 | No vision endpoint is configured | Not an error: OCR-only is a supported mode, and the message points at the setup guide |
| 12 | The endpoint accepts a connection and then stalls | Every request carries a timeout; one bad configuration must not freeze a batch |
| 3 | Tauri + WebView2 packaging size and CI | Validate early, before P2 is scoped |
| 4 | Dual verification cost blowout | On-demand triggering + `max_verify_pages` |
| 5 | Benchmark claims unreliable across sources | `docfusion bench` against the user's own corpus |
| 6 | Local VLM VRAM footprint | 0.9B Q4 ≈ 600 MB weights, but Ollama's real footprint must be measured |
| 7 | Models turn over every ~6 months | Everything behind traits; `bench` re-evaluates, and the setup guide is a paragraph to update rather than a release to cut |
| 13 | A model served without its multimodal projector answers 200 with garbage | An implausible prompt token count is treated as an error; see §8.2 |
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
