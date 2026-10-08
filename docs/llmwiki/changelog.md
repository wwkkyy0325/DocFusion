# Changelog

Append-only record of meaningful changes. Newest entries at the top.

---

## 2026-10-08 — DocFusion stops running models

**What:** The vision backend is now the user's OpenAI-compatible HTTP endpoint.
Sections 7, 8.2, 10 and 14 rewritten; P1 Steps 5 and 8 restated, and a new Step
9 added for the setup guide.

**Why:** Two successive designs — an Ollama sidecar, then a llama.cpp sidecar —
made the same mistake. Each turned DocFusion into a model runtime as well as a
document pipeline, which meant owning a downloader, a checksum verifier, a
process supervisor, a port allocator and Windows job objects. None of that is
what this project is for, and model runtimes are a product category with their
own users and their own maintainers.

The reduction is affordable because **`llama-server`, Ollama, LM Studio and vLLM
all expose the same OpenAI-compatible API**. One client covers every local
option and a cloud provider besides, so the choice of runtime stops being ours
to make and becomes a configuration value.

**What it removes:** binary and model acquisition, checksum verification,
process lifecycle, orphan prevention, the job object, and GPU detection. An
entire subsystem and four named risks are gone.

**What it adds:** the tool owes the user clear reporting instead. An
unconfigured endpoint must name the guide rather than emit a connection error; a
request must carry a timeout so one bad configuration cannot freeze a batch; and
an implausibly low prompt token count must be an error, because a model served
without its multimodal projector answers 200 with repeated garbage rather than
failing.

**Corrected along the way:** a suggestion that the Doubao desktop application
could host the model. It is a chat client and exposes no local endpoint. The
runtimes that do are the three named above.

**What this makes the user's problem, deliberately:** which model is best.
`docfusion bench` already exists for it, and it now matters more than before —
the tool will no longer pretend to have an opinion. Setup is a document rather
than a feature, which is also a smaller thing to keep current when models turn
over every few months.

**The OCR path is unaffected and still bundled.** Deterministic formats and
scans work with no configuration at all, which is what makes OCR-only a
supported mode rather than a degraded one.


## 2026-10-08 — Local inference moves from Ollama to llama.cpp

**What:** Replaced the Ollama sidecar with llama.cpp, managed by the tool.
Design §8.2 rewritten; P1 Step 5 restated.

**Why:** The earlier decision optimised for reaching a validated end-to-end
chain fastest, which traded a product decision for engineering convenience.
Ollama is a separate application with its own daemon, its own login autostart,
its own model store and its own uninstall path. The requirement is that the tool
leave nothing on a user's machine beyond its own directory, and Ollama cannot
meet it.

**This is not a capability regression.** llama.cpp's multimodal documentation
lists dedicated support for exactly the models this project wants — PaddleOCR-VL,
GLM-OCR, DeepSeek-OCR, dots.ocr and HunyuanOCR — and PaddleOCR-VL has an official
GGUF release.

**What the change costs:**

- A subprocess exists while documents are being processed. It is spawned on
  demand on a free localhost port and terminated when the run ends.
- **A crashed parent would orphan that subprocess**, which a destructor does not
  prevent. The child is therefore placed in a Windows job object with
  `KILL_ON_JOB_CLOSE`, so it dies with the parent. This is the specific failure
  the requirement is about, and it is now a named risk in §14.
- The download is ~290 MB of model beside ~840 MB of multimodal projector on
  first use. The projector is not optional and cannot be inferred from a
  successful HTTP response: without it the image never becomes vision tokens and
  the model emits repeated garbage rather than failing.

**Considered and rejected:** linking llama.cpp into the binary, which removes the
subprocess but needs a C++ toolchain and CMake, fixes the GPU backend at compile
time, and adds hundreds of megabytes to the executable; and `mistral.rs`, which
is pure Rust and in-process but runs only general vision models, not the
document-OCR specialists, and has no Vulkan backend.

**Two open questions that only measurement can settle**, both recorded in §8.2:
whether PaddleOCR-VL needs a separate layout-detection pass before pages reach
it, and how much quantization costs accuracy in body text.


## 2026-10-08 — Benchmark harness, and the bug it found first

**What:** `docfusion bench --set <dir> [--engine <id>]`, scoring any engine
against a corpus of reference documents. P1 Step 1.

**Decisions and why:**

- **Scored with a Markdown parser, not string comparison.** Both sides are read
  with `comrak`, so what is compared is the structure a reader would see. The
  components are reported separately as well as combined, because a model that
  reads every character and loses the section headings is not the same as one
  that reads most characters and keeps them.
- **Structural loss is weighted above character noise.** A missing heading or a
  table of the wrong shape is worse than a scattering of character errors,
  because the text that survives still looks correct — which is exactly how a
  silent failure gets believed.
- **Heading agreement is order-sensitive.** A document whose sections are all
  present but shuffled is not the same document, and again the text reads fine.
  The count is the longest run still in order.
- **A corpus is one directory per case.** Pairing by file stem cannot express a
  Markdown source, because the source and the reference would share a name. This
  was found by trying to write that case.
- **Cost is omitted, not zeroed.** No configured backend has a price, and a
  zero would read as free.

**The harness paid for itself on its first run.** A document byte-identical to
its reference scored 0.047 characters of error, which turned out to be real:
`TextPassthrough` read Markdown line by line, so a list flattened into a
paragraph whose leading dash was then escaped to stop it becoming a list. Notes
went in as Markdown and came out as text that merely looked like it. Markdown is
now parsed with `comrak`, which was already a dependency and is now a real one,
and the same case scores zero — including nested lists, tables, quotes and code
blocks.

Two things followed from that fix: `BlockKind::Quote` joined the IR, and list
nesting moved into `children`, so the renderer indents by recursing rather than
by a depth field that a reader would have to reconstruct the tree from.

A test caught a further mistake: list items live inside a `List` node, and
handling `Item` alone produced no blocks at all.


## 2026-10-08 — Reconcile, and the end of P0

**What:** `docfusion reconcile [--fix]`, exit code 4, and a schema migration that
records the output content hash. **P0 is complete**: a document goes in, a
structured Markdown file comes out, and re-processing is skipped.

**Decisions and why:**

- **The output hash exists so a moved file can be recognised.** A rename breaks
  the path link even though nothing about the document changed, so repairing by
  deleting the row would throw away the index entry for a document that is
  sitting right there under a new name. Schema version 2 adds the column; the
  migration is additive, because an index that cannot be opened is an index
  somebody deletes, and deleting it throws away what was already processed.
- **An unclaimed file is not drift.** A person may keep their own notes beside
  the extracted documents. The index says nothing about such a file, so it
  cannot be wrong about it — it is reported, and `--fix` leaves it alone.
- **`--dry-run` outranks `--fix`.** A global switch that says "change nothing"
  must not be overridden by a command-specific one that says "change this".

**Two bugs found by running the command, not by testing it:**

1. **A cache hit did not check that its output still existed.** After deleting
   an extracted file, re-adding the source reported `cached` and handed back a
   path to nothing, skipping the write that would have put it back. A cache hit
   now confirms the file is there, so `add` heals itself and `reconcile` remains
   the tool for repairing in bulk.
2. **Reconcile returned exit 4 for a tree containing only an unclaimed file**,
   contradicting the rule written in the same function — it tested
   `drift.is_empty()` rather than `needs_repair()`.

Also worth recording: a path-frame mismatch inside `reconcile` itself. Output
paths are relative to the knowledge root (`docs/notes.md`) while the directory
walk originally produced paths relative to the document directory
(`notes.md`), so nothing ever matched. The first attempt to fix it introduced a
second instance of the same mistake — hashing `docs/docs/...` — which
`filter_map` swallowed silently. Both are now pinned by tests.


## 2026-10-08 — The command line

**What:** `docfusion add`, `show`, `list` and `schema`, with `--json`,
`--dry-run` and semantic exit codes. P0 is now usable end to end.

**Decisions and why:**

- **stdout carries results and nothing else.** NDJSON under `--json`, human
  lines otherwise; progress, warnings and errors always go to stderr. A caller
  parsing stdout should never have to defend against a log line, which is the
  most common way a command line tool breaks the agent driving it.
- **Exit code 3 is separate from 1.** "Nothing can read this input" is a
  different situation from "the run broke": the file is fine and the tool is
  missing a backend. A batch that reports 3 says install one; a batch that
  reports 1 says something is wrong.
- **One bad file does not abandon the batch.** A run of thirty documents should
  not be lost to one unreadable scan, so failures are reported per file and the
  batch exits 2 when only some succeeded.
- **`--dry-run` does everything except write.** Reporting the real result rather
  than a prediction of one is worth the extraction cost, and it keeps the dry
  run from being a second, differently-behaving code path.
- **`reconcile` and exit code 4 were held back to Step 8.** Publishing a code
  the tool cannot emit is a contract that lies.

**Three bugs found by running the binary rather than only testing it:**

1. `show` failed on a bare file name — the index stored the absolute path a
   document was imported from, but a person types the name they remember. Fixed
   by falling back to matching on the file name, with exact matches tried first
   so a full path never loses to an unrelated document with the same name.
2. Timestamps carried nanosecond precision, which distinguishes nothing anyone
   acts on and made two runs a second apart look as different as two runs a
   microsecond apart.
3. **Output files were named after the source file rather than the document's own
   title**, contradicting the design's own example. `report.pdf` containing
   "2024 Annual Report" now produces `2024-annual-report.md`.


## 2026-10-08 — Markdown rendering

**What:** IR → GFM Markdown with YAML front matter, filename slugs, and text
escaping. Verified by reading the output back with a real CommonMark parser.

**Decisions and why:**

- **Extracted text is escaped, but only where it changes meaning.** A paragraph
  lifted from a Word file is plain text that was never Markdown; if it begins
  with `#` or `-` it would silently become a heading or a list. Only the start
  of a line is escaped, though — escaping inline punctuation too would turn
  `5 * 3` into `5 \* 3`, noise that changes nothing because an unpaired `*`
  already renders literally. A leading `2024.` is handled by escaping the
  period, since a backslash only escapes punctuation and `\1` would render as
  `\1`.
- **The slug allows only alphanumeric characters and collapses everything else
  into `-`.** An earlier attempt excluded the characters Windows forbids, which
  got the platform question right and the slug wrong: a dash is legal in a
  filename but is a separator here, so `Q1 - 2024` came out as `q1---2024`. The
  allow-list makes platform safety fall out for free.
- **CJK is kept rather than transliterated.** Those characters are alphanumeric
  and legal in filenames, and a transliterated Chinese title is unrecognisable
  to the person who wrote it.
- **YAML scalars are quoted conservatively.** YAML's plain-scalar rules are
  subtle, and over-quoting costs two characters while under-quoting produces a
  field a parser reads as something else. A quoted value also keeps a stable
  type: a bare timestamp may be read back as a date rather than the string it
  was written as.
- **No page separators between pages.** A knowledge base is read as continuous
  prose; a horizontal rule between every page of a long PDF is noise. Page
  structure stays in the IR.
- **Front-matter timestamps are supplied by the caller, not read from the
  clock.** A renderer that reads the clock cannot produce the same bytes for the
  same input, so every re-render would look like a change to anything comparing
  the output.

**Verification:** four round-trip tests parse the rendered Markdown with
`comrak` and assert on the resulting tree — heading levels, list items, and
table dimensions — rather than on substrings. The one that matters most checks
that text which *looks* like structure does not become structure, and that front
matter is not read as document body.


## 2026-10-08 — Spreadsheets and presentations

**What:** `XlsxParser` and `PptxParser` complete the deterministic engine set.
P0 Step 5 is now done apart from HTML, which stays deferred.

**Decisions and why:**

- **XLSX delegates parsing to `calamine`.** Shared string tables, cell types and
  date serials are more trouble than they look, and `calamine` is purpose-built
  for reading. DOCX and PPTX stay hand-rolled over the shared OOXML layer
  because their structure is simple and the markdown shape is easier to control
  directly.
- **`calamine` cannot distinguish "not a spreadsheet" from "corrupt
  spreadsheet".** It reports both as a read error, and only one of those should
  let routing continue. The extractor checks for `xl/workbook.xml` in the
  archive first, so a pptx handed to it declines instead of failing the run.
- **Slides are ordered numerically, not by name.** A ten-slide deck contains
  `slide10.xml`, which sorts before `slide2.xml` as text — ordering by name
  would silently reshuffle the presentation. There is a test for exactly this.
- **Slide geometry is converted from EMU to points.** DrawingML measures in
  English Metric Units; reporting raw EMU would make every downstream geometry
  comparison meaningless.
- **Whole numbers are not written with a decimal point.** Spreadsheets store
  every number as a float, so an integer cell arrives as `42.0`; echoing that
  would be a formatting artefact rather than data.

**A test that was wrong rather than the code:** an assertion expected a sheet's
short row to stay short, but `calamine` normalises the range to a rectangle.
The real behaviour is the better one — it is what keeps a value under its own
column heading — so the test was corrected to assert it.


## 2026-10-08 — Text-layer PDF extraction

**What:** `PdfTextLayer`, backed by PDFium via `pdfium-render` + `pdfium-bundled`.
Extracts text, font sizes and per-line bounding boxes, infers headings from font
size, and declines (rather than fails) when a PDF has no text layer so the router
can fall through to a future OCR/vision backend.

**Decisions and why:**

- **PDFium over pure Rust.** Chosen for CJK and CID font handling — the decisive
  requirement for this project's documents — plus per-character bounds and font
  names, which heading inference needs. MIT OR Apache-2.0 keeps it
  commercially safe. `mupdf-rs` was ruled out as AGPL-3.0 and `poppler-rs` for
  its GLib/Cairo stack on Windows.
- **`pdfium-bundled` rather than `pdfium-render` alone.** `pdfium-render` ships
  no library at all. `pdfium-bundled` downloads the pinned build at build time,
  verifies it against a SHA-256 digest, and embeds it, so end users install
  nothing and need no network. Measured cost: 7.4 MB, not the ~30 MB the crate
  documentation claims.
- **PDFium binds only once per process.** Discovered by a failing test, not by
  reading docs: a second `bind_to_library` returns
  `PdfiumLibraryBindingsAlreadyInitialized` instead of the loaded library. The
  binding is now a process-wide singleton. This is a real architectural
  constraint — two engine instances, or one instance used twice, would have
  broken in production exactly as it broke in the test.
- **The PDFium build is part of the cache key.** Upgrading the engine changes
  extraction output, so the build number is recorded in `EngineStamp.model`
  rather than left to someone remembering to bump a version constant.
- **The engine declines when there is no text layer.** `Fit::Maybe` exists
  precisely for this: sixteen magic bytes cannot reveal whether a PDF carries
  text, and a scanned PDF must hand over to the next engine rather than be
  reported as a hard failure.

**Bugs caught by tests during this step:**

- Coordinates were converted twice — negated when collecting runs *and* flipped
  again when normalising. Reading order still passed, which is exactly how this
  class of bug survives: the ordering was right and only the geometry was wrong.
  The conversion now happens once, at the boundary, where the page height is
  known.
- The PDFium singleton issue above.

**Testing note:** PDF fixtures are generated by a small hand-written builder
rather than committed as binaries, so the repository carries no opaque files and
each test states exactly what the page contains.


## 2026-10-08 — Storage layer

**What:** BLAKE3 hashing (streamed, so large documents are never buffered whole),
a SQLite manifest with a versioned schema, and the dedup lookups the pipeline
depends on.

**Decisions and why:**

- **A cache hit requires the whole engine stamp to match, not just the content.**
  `model` and `prompt_version` participate in the comparison via SQL `IS`, so a
  NULL never compares equal to a value. Leaving `prompt_version` out would mean
  that editing a prompt silently serves stale output — the failure would look
  like a mysterious quality regression rather than a bug.
- **`aliases` records every path a content has been seen at, including the
  first.** A single complete table lets `reconcile` answer "which document does
  this file belong to?" without unioning two sources.
- **Editing a source file leaves a stale row behind.** Rows are keyed by content
  hash, so changed content produces a new row while the old row keeps pointing at
  an output path that now holds different content. This is now an explicit
  `reconcile` responsibility (design §9.4), safe because `runs` keeps history.
- **`runs` is created but unused.** Nothing writes pipeline history until the CLI
  lands in Step 7; adding accessors before a caller exists would be speculation.

**Timestamps** are RFC 3339 text so the database stays readable when inspected
directly, with the clock owned by the caller rather than the manifest inventing
times.


## 2026-10-08 — Model selection becomes measurement-driven

**What:** Added design §8.5 (evaluation corpus) and a sequencing requirement in
§8.4: the bench harness and golden corpus are built **before** the local VLM
default is fixed. P1 was reordered so the harness comes first.

**Why:** Review feedback — the model choice should be decided by testing on a
suitable corpus and then tuned, not taken from public leaderboards. This matches
the project's own finding that leaderboard numbers come from different benchmark
versions and are not directly comparable. The candidates listed in §8.2 are now
explicitly starting points for measurement rather than a decision.

**Consequence:** No model name may be hard-coded outside a `VlmBackend` impl, and
each bench run records its corpus hash so results remain comparable over time.

## 2026-10-08 — Initial design

**What:** Created the repository and wrote the initial design for DocFusion: a
local-first document ingestion pipeline producing structured Markdown.

**Why:** Greenfield project. The design was reached through a structured
requirements discussion; the decisions below are the outcome.

**Key decisions:**

- **Extraction backend is pluggable.** A default deterministic offline chain,
  with LLM/VLM as an optional enhancement layer. `Extractor` and `VlmBackend`
  traits keep every engine swappable.
- **Storage: plain file tree + sidecar index.** The Markdown tree is the external
  contract (Obsidian/editor/git/agent compatible); `.docfusion/manifest.db`
  (SQLite) holds dedup and provenance state. Chosen over a content-addressed
  store (nothing to browse) and over a DB-primary store (locks the user in).
- **Dual verification, with roles that depend on tier.** Local-only setups verify
  OCR against a local VLM; once a cloud VLM is configured, the local OCR becomes
  the verifier. Verifier lineage must differ from the primary engine.
- **Local engine stack is fixed for the user.** RapidOCR (ONNX Runtime, no
  PaddlePaddle) for OCR; a ~0.9B document VLM served via Ollama. Users do not
  pick models — only the cloud provider is configurable.
- **IR is block-structured with normalized bbox and confidence.** Without this,
  cross-engine block alignment is impossible and dual verification degenerates
  into an unusable whole-document text diff.
- **Conflicts are first-class IR data, not logs.** Rationale: specialized parsers
  fail loudly, general MLLMs fail silently; silent failure is unacceptable for
  invoices and contracts.
- **Agent surface is the full CLI; the GUI is deliberately minimal.** One page,
  progressive single column, no model selection, no knowledge-base management.
- **Tauri v2 with a static frontend and no Node build chain.**

**Notes:** Model sizes and benchmark figures cited in the design came from web
sources of mixed reliability and are marked for re-verification at implementation
time.
