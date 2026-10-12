# DocFusion

**English** | [简体中文](README.zh-CN.md)

[![CI](https://github.com/wwkkyy0325/DocFusion/actions/workflows/ci.yml/badge.svg)](https://github.com/wwkkyy0325/DocFusion/actions/workflows/ci.yml)

**Turn PDFs, images and Office documents into structured Markdown — locally, exactly once, into a file tree you already know how to use.**

DocFusion is a local-first document ingestion pipeline written in Rust. It reads
a document, decides for itself which engine can read it, writes the result as
Markdown with provenance front matter, and indexes it so it never has to do the
work twice. It ships two surfaces over one core: a command line that is designed
to be driven by an agent, and a small desktop window for converting and reviewing
a single document.

There is no server, no account, and no vector database. The output is files.

```
                 ┌──────────────────────────────────────────────┐
   a document ──▶│ mime sniff → BLAKE3 → seen it before?        │
                 └──────────────────────────────────────────────┘
                        │ miss
                        ▼
   text-layer PDF ──▶ PDFium          (deterministic, no model)
   scan / image  ──▶ OCR + layout     (local, ONNX Runtime)
   docx/xlsx/pptx ─▶ pure-Rust parsers
                        │
                        ▼
              block IR (text + bbox + confidence + engine)
                        │
                        ▼
        verify (optional second engine, disagreements kept)
                        │
                        ▼
     Markdown + YAML front matter + assets → atomic write
```

---

## Contents

- [What it is](#what-it-is)
- [Principles](#principles)
- [Status](#status)
- [Quick start](#quick-start)
- [Command line](#command-line)
- [Configuration](#configuration)
- [How a document is read](#how-a-document-is-read)
- [The knowledge base on disk](#the-knowledge-base-on-disk)
- [Search, and Chinese](#search-and-chinese)
- [Desktop app](#desktop-app)
- [Export](#export)
- [Engines and models](#engines-and-models)
- [Evaluation](#evaluation)
- [Engineering notes](#engineering-notes)
- [Repository layout](#repository-layout)
- [Building and testing](#building-and-testing)
- [Third-party components](#third-party-components)
- [License](#license)

---

## What it is

Most document pipelines answer the question *"how do I get text out of a PDF?"*.
DocFusion answers a narrower and harder one: **how do I put a document into an
archive and find it again, without ever silently lying about what it said?**

That framing decides the features:

| | |
|---|---|
| **Content addressing** | Every document is identified by the BLAKE3 hash of its bytes. The same file arriving from three folders is stored once and known by three names. |
| **A block IR, not a string** | Extraction produces `Vec<Block>` with a bounding box, a confidence and the engine that produced it. Two engines cannot be compared, and a disagreement cannot be located, against a string. |
| **Verification is a first-class step** | When a reading is uncertain — or when it contains an amount, a date or an identifier — a second engine is asked. Disagreements are recorded in the document rather than smoothed away. |
| **The knowledge base is plain Markdown** | `docs/` is a normal directory of `.md` files with YAML front matter. Obsidian opens it, `git` versions it, an agent `grep`s it. There is no lock-in to escape from. |
| **No model to choose** | Local OCR runs from a fixed, digest-pinned set of weights. A vision model is optional and is *yours* — any OpenAI-compatible endpoint, local or hosted. |

### What it is not

Non-goals, stated so they don't creep in:

- **No embeddings, no rerankers, ever.** Search is one SQL query against one
  SQLite file (FTS5), and it is meant to stay that way. Summaries, entity
  extraction, auto-tagging, graphs and backlinks are a *different product* that
  would consume this one's output — which is why the output is plain Markdown.
- **No multi-user, no server, no auth, no quotas.**
- **No legacy Office formats** (`.doc`, `.xls`, `.ppt`) — detected and refused
  with a conversion hint rather than half-read.
- **No handwriting OCR** as a first-class goal.

The priority this buys is measurable: **how fast a document goes in, and how fast
it comes back out.**

---

## Principles

Three rules the code is actually held to, because each one was broken once and
caught:

1. **Never process the same bytes twice.** A cache hit is decided by content,
   plus the engine, model and prompt version that produced the reading — so a
   document is re-read when the *engines* change and not otherwise.
2. **Never silently lie.** A reading that was uncertain says so. A conflict
   between two engines is written into the document. A claim that cannot be
   supported is reported as a problem rather than as an empty success.
3. **Never make the user guess.** Misspelt settings are errors with the file and
   the line, not silently-ignored keys. `docfusion schema` publishes the whole
   contract — commands, fields, exit codes — so an agent never has to parse help
   text.

---

## Status

**v0.1.0 — working, and honest about its edges.** All four crates build, the
suite is green, and the CLI and the window share one core.

| Area | State |
|---|---|
| Deterministic extraction (text PDF, DOCX, XLSX, PPTX, TXT, MD) | done |
| Local OCR (RapidOCR PP-OCRv6 on ONNX Runtime) | done |
| Layout / structure model (PP-DocLayout_plus-L) | done, downloaded on demand |
| Vision endpoint (any OpenAI-compatible server or API) | done |
| Dual verification, conflict records | done |
| Content-addressed store, dedup, atomic writes | done |
| Full-text search (FTS5, Chinese-aware) | done |
| Block-structured Markdown editor + source comparison (GUI) | done |
| Export: Markdown, HTML, text, CSV, DOCX, XLSX | done |
| Benchmark harness (`docfusion bench`) | done |
| `serve` (local HTTP), `mcp` (MCP server) | not built |

The design document is `docs/llmwiki/design.md`; the setup guide is
`docs/setup.md`; every change is recorded with its reasoning in
`docs/llmwiki/changelog.md`.

---

## Quick start

Requires a Rust toolchain (**1.88+** — edition 2024 alone would run on 1.85, but
PDFium and ONNX Runtime's bindings each ask for 1.88).

```bash
git clone https://github.com/wwkkyy0325/DocFusion.git
cd DocFusion
cargo build --release
```

The binary is `target/release/docfusion`.

```bash
# Put a document in. Text-layer PDFs need nothing else installed.
docfusion --root kb add report.pdf

# See what it became.
docfusion --root kb show report.pdf

# Or read it as a file.
cat kb/docs/report.md
```

The first `add` of a scan or an image fetches the recognition weights — 7 MB,
once, verified by SHA-256 before anything is trusted. Nothing downloads at
startup and nothing refreshes in the background.

```bash
docfusion doctor        # what this installation can read, and what is missing
```

Run the desktop window instead, if you would rather click:

```bash
cargo run --release -p docfusion-gui
```

---

## Command line

```
docfusion add <paths...>            Process and ingest
docfusion show <id|path>            Emit the Markdown
docfusion list [--status S]         List the documents
docfusion export <id> --to <fmt>    Write a document, or one block, as a file
docfusion search <words...>         Find documents by what is in them
docfusion index                     Rebuild the search index from the Markdown
docfusion reindex [id|path]         Read documents again with the engines here now
docfusion forget <id|path>          Take it out of the index; the Markdown stays
docfusion delete <id|path>          Delete it, its Markdown and its copy
docfusion reconcile [--fix]         Repair the index against the file tree
docfusion bench --set <dir>         Score engines against a reference corpus
docfusion doctor                    What can be read, and what is missing
docfusion schema                    The output contract, as JSON
```

A document can be named by its content id, its first few characters, its source
path, or its file name — whichever is in front of you.

### Built to be driven by a program

Agents get the complete feature set, and four rules make that safe:

1. **`--json` puts NDJSON on stdout and sends every log to stderr.** Log output
   polluting stdout is the most common way an agent integration breaks.
2. **Exit codes carry meaning:** `0` success · `1` error · `2` partial failure ·
   `3` no text layer, an engine must be named · `4` index inconsistent, run
   `reconcile`.
3. **`docfusion schema` is self-describing**, so nothing has to be inferred from
   help text.
4. **Every mutating command supports `--dry-run`.**

Configuration can be given entirely as flags or environment variables
(`DOCFUSION_ROOT`, `DOCFUSION_ENDPOINT`, `DOCFUSION_MODEL`, `DOCFUSION_API_KEY`,
`DOCFUSION_VERIFY`, …), so an agent never has to read a file to find out how the
tool is configured.

---

## Configuration

One file, per user, **outside any repository** — it holds an API key, and a file
beside the documents is a file that eventually gets committed:

```
%LOCALAPPDATA%\DocFusion\config.toml      Windows
$HOME/.cache/docfusion/config.toml        unix
```

```toml
endpoint = "https://api.deepseek.com"
model    = "deepseek-flash"
api_key  = "sk-..."
```

**Precedence: flag → environment variable → file → built-in default.**

A vision endpoint is entirely optional. With none configured, DocFusion still
reads every text-layer PDF, every Office file and every scan through local OCR —
the endpoint buys *structure* (headings, tables, reading order across columns),
not the ability to read at all.

Any endpoint speaking `POST {base_url}/v1/chat/completions` with an image in a
`user` message works: `llama-server`, Ollama, LM Studio, vLLM, or a hosted API.
One client covers all of them. There is a **Test** button in the window's
settings that shows the endpoint a drawn page with a number on it and says
whether the number came back — because an address that answers is not the same as
a model that can see.

---

## How a document is read

**Routing is by content, not by extension**, and the first rule is that
**a PDF with a text layer never touches a model.** That path is deterministic,
instant and free; sending it to a recogniser would be slower and less accurate.

| Input | Path |
|---|---|
| PDF with a text layer | PDFium. No model, no network, no cost. |
| PDF that is a scan, images, scans | Local OCR, plus the layout model when it is installed |
| DOCX, XLSX, PPTX, TXT, MD | Pure-Rust parsers, straight into the IR |

### Verification

A second engine is asked only when it is worth asking — policies are `doubt`
(where the first engine was unsure, or read a value whose exactness matters),
`fields` (only where the text holds an amount, a date or an identifier), `always`,
and `off`.

When the two disagree, the disagreement is **kept**: the document carries the
reading, the second opinion, and the pages they differ on, so a human can decide.
Arbitrating automatically would be a guess presented as an answer.

---

## The knowledge base on disk

```
knowledge/
  docs/
    2024-annual-report.md         # YAML front matter + body
    _assets/
      a3f9c2.png                  # extracted figures, content-addressed
  .docfusion/
    manifest.db                   # SQLite manifest + the FTS5 index
    sources/
      a3f9c2….pdf                 # a copy of the bytes that were read
    cache/                        # engine result cache
```

**Deduplication is by content.** Same bytes, same engine, same model, same
prompt version ⇒ a cache hit, and the second path is recorded as an alias of the
first rather than as a second document.

**`.docfusion/sources/` keeps a copy of everything that was read**, and the
reason is not provenance. It is that *a document whose original moved can never
be read again* — and `reindex` exists to re-read documents after an engine or a
model changes, which needs the bytes. Keeping the copies under `.docfusion/`
rather than beside `docs/` is deliberate: a `docs/` directory that is synced or
committed should not drag the contracts and bank statements that were read into
it along with the Markdown.

The cost is disk — the knowledge base is about as large as what has been put into
it. `docfusion delete` is how something stops taking up room; `forget` is the
reversible half and deliberately does not.

**Writes are atomic** (temp file + `ReplaceFileW`/`MoveFileExW` on Windows), and
`reconcile` repairs the index against the file tree when something has moved
underneath it.

---

## Search, and Chinese

Search is SQLite FTS5 and nothing on top of it — no embeddings, no reranker.

**FTS5 has no Chinese tokenizer, so the text is tokenized before it is stored:
one token per character.** This is the decision with the largest effect on a
Chinese corpus, and it was made by measuring rather than by preference. With
default tokenizing, a whole Chinese clause is alphanumeric as far as FTS5 is
concerned and becomes a single token, so `合同` matches nothing. Storing one token
per character makes every character addressable, and a two-character word becomes
two adjacent tokens, which a phrase query finds.

The obvious alternative — FTS5's `trigram` tokenizer, which handles all scripts —
was measured and rejected: it silently fails queries shorter than three
characters, which is most two-character Chinese words. Per-character tokens are
also *smaller* than trigram, not larger.

Queries are quoted internally, so `合同` asks for the two adjacent tokens `合` and
`同` rather than for two independent characters.

---

## Desktop app

A Tauri v2 window over the same core. The frontend is hand-written HTML/CSS/JS
served as-is — **there is no Node or npm step in the build**; `cargo build` is the
whole build, and the Markdown-to-HTML conversion happens in Rust. The one
vendored dependency (CodeMirror 6, MIT) is committed as a single ES module
because the content policy is `default-src 'self'` and an offline machine must
still work.

Three tabs:

| Tab | What it does |
|---|---|
| **Document** | Convert one file, read the result, review conflicts between engines, then copy or export it |
| **Records** | Everything in the library: browse, search, open, forget, delete |
| **Settings** | The model and endpoint, the theme, and the state of the local model set |

On the document page:

- **Blocks are addressable.** Hovering one washes it, and a single `⋯` button
  opens its actions — copy, or export that block on its own. Blocks are numbered
  in the order an export takes them, so "block 7" means the same thing in the
  window and on the command line.
- **The Markdown can be edited in place**, with the rendered result beside it,
  and the edit is written back into the document with its own timestamp so an
  edited document is never mistaken for an engine's reading.
- **The original can be looked at beside the reading.** For a PDF or an image the
  left half shows the page that was read, and for a file with no pages it says so
  rather than showing an empty half — the one thing Markdown cannot show anybody
  is what it was looking at.

---

## Export

The rule: **an export keeps what the words alone do not.**

| Format | For |
|---|---|
| `md` | The document as it is stored |
| `html` | A page with its emphasis and its heading levels |
| `txt` | The words, for a whole document (a single block's words are what `show` and the copy button are for) |
| `docx` | A Word file — a real WordprocessingML document, built from the parsed Markdown |
| `xlsx` | A spreadsheet, one sheet per table |
| `csv` | A single table's rows |

`docx` and `xlsx` are zips and therefore need `--out`; neither can be written to
a terminal. As always, the difference between formats is the point: a table
exported to CSV and the same table exported to Markdown do not have the same
answer to "what is this".

---

## Engines and models

| Concern | Choice |
|---|---|
| PDF text layer and rendering | **PDFium** (Chromium's engine), embedded in the binary. CJK-safe. |
| Local OCR | **RapidOCR** — PP-OCRv6 tiny detector, recogniser, angle classifier — on **ONNX Runtime** |
| Page structure | **PP-DocLayout_plus-L** — finds headings, tables, headers, footers, formulas |
| Vision | **The user's endpoint.** Not ours to run. |
| DOCX/XLSX | `zip` + XML, `calamine` |
| Markdown → HTML | `comrak` (GFM) |
| Store | SQLite (`rusqlite`, bundled) with FTS5 |
| HTTP | `ureq` — blocking and small; the pipeline is synchronous, so an async runtime would be weight carried for nothing |

**The model set is six files, 144 MB, and none of them are inside the binary.**
That was a trade, and the cost is worth stating plainly: a fresh installation
reads no image and no scanned PDF until the weights arrive. What it buys is a
single place that defines what "complete" means, digests checked before anything
is trusted, resumable downloads and mirrors tried in order. Nothing is fetched
unless somebody asks for it.

The parts are not equal, and the split is deliberate. The **recognition set is
6.9 MB and is not optional** — without it, a page of text that is a picture
cannot be read at all. The **table model is 7.8 MB** and the **layout model is
124 MB**, and both are improvements rather than capabilities: every page this
tool ever read, it read without them.

Everything runs locally. Nothing is uploaded anywhere unless a vision endpoint
has been configured, and then only pages that need one.

---

## Evaluation

`docfusion bench --set <dir>` scores engines against a corpus of reference
documents — one directory per case, holding `expected.md` beside the source
document. The pairing is a directory rather than a filename convention, because
`notes.md` beside `notes.md` is not a pairing, it is one file.

### Two error rates, not one

The first real corpus proved that comparing output to a reference as one ordered
string conflates two different failures. A line-recognition engine given a
three-column newspaper reads *across* the columns instead of *down* them: it
recovered 97.7% of the reference's characters while scoring a character error of
**0.978**. Read as "the engine is broken" that number is false; read as "the
engine cannot order a page" it is correct and actionable.

So `Score` reports both:

- `character_error_rate` — reading order included. What a reader experiences.
- `content_error_rate` — the same characters, order-free. What survived.

**The gap between them is the damage that ordering did**, and it is the first
thing to look at when a page reads well but scores badly.

### The baseline

24 pages, both engines over the same corpus:

| | local RapidOCR | vision endpoint (DeepSeek-V4.1-Flash) |
|---|---|---|
| mean error, in reading order | 0.466 | **0.245** |
| mean error, ignoring order | 0.203 | **0.171** |
| headings found | 0 / 64 | **9 / 64** |
| tables found with the right shape | 0 / 9 | **7 / 9** |

**Close on characters, not close on structure.** A line recogniser has no
concept of a heading or a table. Reading tables with the layout model moved that
last row from 1 of 9 to 7 of 9 — the single largest quality gain in the project,
and it came from running a model rather than from tuning one.

The two engines also **fail on different pages**, which is the entire argument for
having two. On one Chinese newspaper page the local engine recovered 97.7% of the
reference's characters while the cloud model recovered 72.8%, cleanly omitting
the masthead, the editor and the phone number — because that provider downsizes a
2754×3939 scan to roughly 900×1280 before reading it, and fine print does not
survive that. The local engine runs at full resolution and does not care.

### The corpus

The first corpus is **24 pages sampled from the base subset of
[OmniDocBench v1.6](https://github.com/opendatalab/OmniDocBench)** — the current
release, downloaded from the dataset's `master` revision. v1.6 keeps the base
subset labelled `v1.5` in the data, which is a **subset name rather than a
version number**, and the corpus builder filters on exactly that; the full
release is 1,651 pages. The `equation_hard`, `layout_hard` and `table_hard`
subsets were deliberately left out: they are adversarial by construction and say
little about ordinary documents.

Three limits are known and are **not** corrected for, because correcting them
would hide them:

1. **The reference is ours.** OmniDocBench ships per-region annotations rather
   than Markdown, so the `expected.md` files were generated by a converter whose
   rules decide what counts as a correct reading. It was validated by looking at
   pages beside their references. It is still a converter.
2. **Formulas are contaminated.** The reference carries the dataset's own LaTeX,
   spaced as `0 . 1 6 6 \%`, which no engine will reproduce. 14 of the 24 cases
   contain LaTeX, and they are the worst-scoring ones.
3. **The pages are born-digital, rendered to images.** No skew, no camera noise,
   no uneven lighting — so every number above is optimistic about real scans.

> **The corpus is not in this repository.** Real documents are often private, and
> this one is the benchmark's. The corpus is referenced by path
> (`docfusion bench --set <dir>`), and what ships here is the harness and the
> scoring, not the data.

### Citing the dataset

If you use these numbers, or build on this harness, cite the benchmark:

```bibtex
@misc{ouyang2024omnidocbenchbenchmarkingdiversepdf,
  title={OmniDocBench: Benchmarking Diverse PDF Document Parsing with Comprehensive Annotations},
  author={Linke Ouyang and Yuan Qu and Hongbin Zhou and Jiawei Zhu and Rui Zhang and
          Qunshu Lin and Bin Wang and Zhiyuan Zhao and Man Jiang and Xiaomeng Zhao and
          Jin Shi and Fan Wu and Pei Chu and Minghao Liu and Zhenxiang Li and Chao Xu and
          Bo Zhang and Botian Shi and Zhongying Tu and Conghui He},
  year={2024},
  eprint={2412.07626},
  archivePrefix={arXiv},
  primaryClass={cs.CV},
  url={https://arxiv.org/abs/2412.07626}
}
```

**Note the licence, because it is not this project's.** OmniDocBench is released
**for research purposes only, not for commercial use**. Nothing from the dataset
is redistributed here — no page images, no annotations, no references — and the
MIT licence below covers this repository's code only. Anyone reproducing the
benchmark is responsible for their own use of that dataset, and any commercial
use of these code paths should be re-measured on a corpus of their own.

---

## Engineering notes

Things that took more than one attempt, recorded because the *reasons* are the
interesting part.

**Windows file replacement is not `rename`.** Overwriting an existing file needs
`ReplaceFileW`/`MoveFileExW`; a plain rename fails or loses the target. Every
write in the store goes through the one atomic path that got this right.

**The scorer could not see inside a code block.** A vision model wrapped whole
pages in a fenced block with a one-line preamble ("Here is the transcription…").
Left in place, a page became one verbatim code block and every heading and table
was lost — and because a fence's content is a literal rather than a text node,
the scorer read a *correctly* transcribed page as a content error of **1.000**.
Unwrapping that envelope and teaching the scorer to look inside a fence moved two
cases from 1.000 to 0.089 and the corpus mean from 0.292 to 0.245. Both bugs were
found by running on real documents, not by tests.

**Asking for thinking to be off has to be explicit.** A provider's default is on
at high effort, and it dominated output: asked for a single word, the model spent
24 of its 26 output tokens thinking. It also makes `temperature` meaningless —
the setting that keeps a model from paraphrasing a page it is supposed to be
transcribing.

**The GPU was measured before it was believed in.** DirectML execution providers
are implemented and off by default, because on real pages they mostly did not
help: the cost is in the pipeline, not in the arithmetic.

**Two headings from one region.** A heading region can legitimately contain a
title and a subtitle; emitting one heading per region loses one, and emitting one
per line invents a heading where a wrapped line was. The rule is written down and
tested, rather than tuned by eye.

**This project has a knowledge base of its own.** `docs/llmwiki/` records the
design as a living document, one task doc per piece of work with a verification
step for each, and an append-only changelog where every entry says what changed
*and why* — including the measurements that changed a decision.

---

## Repository layout

```
DocFusion/
  crates/
    docfusion-core/     IR, extractors, engines, verification, rendering, export
    docfusion-store/    manifest, content addressing, cache, config, reconcile
    docfusion-cli/      the `docfusion` command
    docfusion-gui/      the Tauri window (Rust backend + static `ui/`)
  docs/
    setup.md            the setup guide, as a product deliverable
    llmwiki/            design.md, changelog.md, tasks/
```

The split into crates rather than feature flags is deliberate: **the CLI must not
compile Tauri.** Tauri's dependency tree is hundreds of crates and a CLI user
should never pay that build cost. It also keeps `docfusion-core` testable in
isolation, against a corpus of golden IR rather than through a user interface.

---

## Building and testing

```bash
cargo build --release                  # the CLI
cargo run --release -p docfusion-gui   # the window
cargo test --workspace                 # 603 tests
cargo clippy --workspace --all-targets
```

The build needs a network connection the first time: `pdfium-bundled` downloads
and links Chromium's PDF engine, and `ort` fetches ONNX Runtime. Neither needs
anything installed on the machine.

CI runs clippy and the tests on Windows, and builds the three portable crates on
Linux and macOS. **Formatting is not gated** — the tree predates `rustfmt`
having an opinion about it, and a whitespace commit across 34 files would bury
more history than it cleans.

### Platforms

**Windows is what this is built and released for.** Linux and macOS build, and
both native dependencies ship binaries for them, so the pipeline itself is not
Windows-specific. What has never happened is somebody running the *window*
anywhere but Windows — so the CI canary deliberately excludes it rather than
putting a green tick next to something untested.

Two things stand between here and a release elsewhere. macOS needs signing and
notarization or Gatekeeper refuses the download, and Intel macOS cannot be built
at all: ONNX Runtime publishes no binaries for `x86_64-apple-darwin`, and neither
does Microsoft.

### Releases

A `v*` tag produces two things:

| | |
|---|---|
| `docfusion-<tag>-windows-x86_64.zip` | the command line, portable, with `README.md`, `LICENSE` and `THIRD-PARTY.md` |
| `DocFusion_<version>_x64-setup.exe` | an NSIS installer for the window, per-user, no administrator needed |

The binaries are **not code-signed**, so SmartScreen warns on first run. The
model weights are not in either artifact — 144 MB arrives on first use.

**Uninstalling offers to clean up after itself**, and asks two separate
questions because they are two different sizes of decision: one for the models
and configuration, and one for the library of converted documents, which defaults
to **No**. A library kept somewhere other than the default is not found and not
touched. See [Uninstalling](docs/setup.md#10-uninstalling).

---

## Third-party components

See [THIRD-PARTY.md](THIRD-PARTY.md). The short version: PDFium (BSD-3-Clause),
ONNX Runtime (MIT), the PaddleOCR model weights (Apache-2.0), CodeMirror (MIT),
and the Rust crates in `Cargo.lock` (permissive throughout).

---

## License

[MIT](LICENSE) — this repository's code, and only its code. The benchmark dataset
described under [Evaluation](#evaluation) is separate and is **not** covered by
it. See that section before reproducing any of the numbers here.
