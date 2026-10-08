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
- [x] `OcrBackend` and `VisionBackend`, with `Caps` declaring bounding-box and
      confidence support
- [x] `ScanExtractor`: renders or decodes each page and hands it to the
      configured backends
- [x] A composite engine stamp covering every backend, so pointing the tool at a
      different model invalidates previous output
- [x] A router that reports what is missing when nothing can read an input

**Verify:** a stubbed backend produces a Document from a fixture image, and a
PDF that has a text layer is parsed rather than rendered.

> **Deliberately absent: a fallback from vision to OCR.** A silent downgrade
> would produce unverified output indistinguishable from verified output, which
> is the failure this project is organised around.
>
> **Not yet wired into the CLI.** `ScanExtractor` refuses every input until a
> backend is configured, so registering one now would add an engine that can
> only ever decline. Step 5 supplies the configuration it is waiting for.

### Step 4 — Local OCR backend
- [ ] RapidOCR models on ONNX Runtime; no PaddlePaddle
- [ ] Emits per-line bounding boxes

**Verify:** a generated image of known text is read back exactly.

### Step 5 — Vision backend: the user's OpenAI-compatible endpoint
- [x] One HTTP client against `/v1/chat/completions`, covering llama-server,
      Ollama, LM Studio, vLLM and cloud providers alike
- [x] Configuration: base URL, model name, optional key, timeout, prompt
- [x] A request timeout that names the likely cause, since a model still loading
      and a model too large for the machine look identical from outside
- [x] An implausible prompt token count is an error: a GGUF model served without
      its multimodal projector answers 200 with repeated garbage
- [x] `probe()`, which asks the endpoint what it serves rather than inspecting
      the machine
- [x] Wire the endpoint into the CLI: `--endpoint`, `--model`, `--api-key`, the
      matching environment variables, and `ScanExtractor` registered once a
      backend exists

**Verify:** a stub HTTP server, not a mocked transport — the client is the thing
under test, and mocking below it would test the mock.

> Verified end to end against a stub endpoint: without one, a scan exits 3 with
> a message naming the setup guide; with one, the page is read and written. That
> run also caught a rendering fault the benchmark had missed — adjacent list
> items were being separated by a blank line, which makes a *loose* list and
> renders each item wrapped in a paragraph. The benchmark could not have caught
> it: its scoring compares headings, tables and text, and a loose list has all
> three right. Metric design has gaps, and the gap is worth naming.

**Verify:** against a stub server, a fixture page returns Markdown; with no
endpoint configured the error names the guide; a server that accepts and then
stalls fails on the timeout rather than hanging.

> **Changed from Ollama, then from a llama.cpp sidecar.** Both made DocFusion a
> model runtime as well as a document pipeline, which meant owning a downloader,
> a checksum verifier, a process supervisor, a port allocator and Windows job
> objects. See design §8.2. The tool now runs no models at all.

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

### Step 8 — Probing, not detecting
- [ ] Probe the configured endpoint rather than inspecting the machine
- [ ] Report what was found, and what to do about what was not
- [ ] Check the two things no endpoint can report: free disk space, and the
      WebView2 runtime for the GUI

**Verify:** with no endpoint, the tool reports `OcrOnly` and still processes
text-layer documents and scans through OCR.

> **This step shrinks because of §8.2.** GPU vendor, VRAM, whether a daemon is
> running and which models it holds are questions about somebody else's program,
> and that program answers them better than any inspection we could write.

### Step 9 — The setup guide
- [ ] A guide the user's *agent* can execute: exact commands, exact file names,
      and a check that it worked
- [ ] Cover `llama-server`, Ollama and LM Studio as interchangeable, since all
      three expose the same API
- [ ] State the projector requirement and the download sizes, because a model
      served without its projector fails silently rather than loudly

**Verify:** following the guide on a clean machine produces an endpoint that
`docfusion` reports as working, without any step being guessed at.

---

## Out of scope for P1
The GUI, search, cloud provider implementations beyond what is needed to make
the verifier work, and any semantic enrichment of the output.

**Out of scope permanently, as of this revision:** downloading, verifying,
installing, supervising or versioning models and inference binaries. That is a
product category, and it is not this one.
