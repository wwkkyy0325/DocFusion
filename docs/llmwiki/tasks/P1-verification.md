# Task: P1 — Engines and verification

**Goal:** read documents that have no text layer, and never let an engine be
wrong quietly. Every model choice is made by measurement against a corpus, not
by reading a leaderboard.

**Success criteria (verifiable):**

1. `docfusion bench --set <dir>` scores any engine against a corpus of
   reference documents, reporting accuracy, latency and cost.
2. A scanned PDF and a photograph of a page both produce Markdown.
3. When two engines disagree, the disagreement is recorded in the IR and
   marked in the output — never silently resolved.
4. The engines that run are chosen from the machine's detected capability, not
   from a configuration file the user has to understand.
5. The default local model is selected by benchmark results, and no model name
   appears outside a backend implementation.

---

## Steps

The order matters: the harness comes before the models it will choose between.

### Step 1 — Benchmark harness and corpus format
- [x] Corpus layout: one directory per case, holding `expected.md` beside the
      source document
- [x] `score(expected, actual)` — character error rate, heading agreement in
      reading order, table shape agreement
- [x] `docfusion bench --set <dir> [--engine <id>]`, human and NDJSON output
- [x] Engines report latency; cost is reported when a backend knows it
- [ ] Record the corpus hash on each run, so results stay comparable as the
      corpus grows

**Verify:** a corpus built from the existing fixtures scores the deterministic
engines at zero error, and a deliberately corrupted reference scores worse.
This proves the harness before it is used to judge anything.

> **The harness paid for itself immediately.** Its first run scored a document
> that was byte-identical to its reference at 0.047 characters of error, which
> turned out to be a real fidelity bug: `TextPassthrough` read Markdown line by
> line, so a list became a paragraph whose leading dash was then escaped to stop
> it becoming a list. Notes went in as Markdown and came out as text that merely
> looked like it. Fixed by parsing Markdown with `comrak`, which was already a
> dependency; the same case now scores zero.
>
> It also exposed a flaw in the corpus layout itself: pairing by file stem cannot
> express a Markdown source, because the source and the reference would share a
> name.
>
> **Not yet done:** cost. No configured backend has a price, so the field is
> omitted rather than filled with a zero that would read as free.

### Step 2 — Page images
- [ ] `PageImage` type: pixels, dimensions, source page
- [ ] PDF page rendering through PDFium
- [ ] Image decoding for standalone image files

**Verify:** a fixture PDF renders to the expected pixel dimensions at a chosen DPI.

### Step 3 — Backend traits
- [ ] `OcrBackend` and `VlmBackend`, with `Caps` declaring bounding-box and
      confidence support
- [ ] An `Extractor` adapter that routes images and text-layer-less PDF pages
      to the configured backends
- [ ] A router that reports which backend is missing when nothing can read an
      input

**Verify:** a stubbed backend produces a Document from a fixture image, and the
router prefers the text-layer engine for a PDF that has one.

### Step 4 — Local OCR backend
- [ ] RapidOCR models on ONNX Runtime; no PaddlePaddle
- [ ] Emits per-line bounding boxes

**Verify:** a generated image of known text is read back exactly.

### Step 5 — Local VLM backend
- [ ] Ollama client, with the embedded-library style of failure reporting
- [ ] Model pulled on demand, with clear progress and a clear error when the
      daemon is absent

**Verify:** against a running Ollama, a fixture page returns Markdown; with the
daemon stopped, the error names Ollama and says what to do.

### Step 6 — Verification
- [ ] Alignment by bounding-box overlap; text comparison after normalization
- [ ] `VerifyPolicy`: off, on low confidence, on critical fields, always
- [ ] Conflicts recorded in the IR and marked in the rendered output
- [ ] A cost ceiling, so a large scan cannot run away

**Verify:** two engines given deliberately different text produce a recorded
conflict and a rendered marker; a policy of `Off` produces neither.

### Step 7 — Model selection by measurement
- [ ] Build the corpus from real documents
- [ ] Run every candidate through `bench`
- [ ] Fix the default, and record the numbers that chose it

**Verify:** the chosen default is stated with the scores behind it, and
swapping it is a one-line change behind the trait.

### Step 8 — Capability detection
- [ ] GPU, memory, disk, Ollama presence, cloud reachability
- [ ] `Capability` tiers: `OcrOnly`, `LocalDual`, `CloudEnhanced`
- [ ] The chosen tier is reported by `docfusion doctor` — hmm, name TBD

**Verify:** on a machine with no GPU the tier is `OcrOnly` and the tool still
processes text-layer documents.

---

## Out of scope for P1
The GUI, search, cloud provider implementations beyond what is needed to make
the verifier work, and any semantic enrichment of the output.
