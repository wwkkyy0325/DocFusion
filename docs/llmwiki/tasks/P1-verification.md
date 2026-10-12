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
- [x] RapidOCR models on ONNX Runtime; no PaddlePaddle
- [x] Emits per-line bounding boxes **and** confidence, so this engine can be
      the verifier rather than only a reader
- [x] Read a generated image of known text back exactly
- [x] Model acquisition: the weights are embedded in the binary, measured at
      6.9 MB
- [x] Wire into the CLI, loaded lazily so commands that read no pages pay
      nothing

> **Measured, rather than estimated.** The `ppocrv6-tiny` set is 1.8 MB of
> detector, 4.5 MB of recogniser, 0.6 MB of angle classifier and 27 KB of
> dictionary — 6.9 MB in total, less than half what the design assumed. At that
> size bundling is not a trade-off to weigh.
>
> Verified end to end, with nothing configured: an image reading "Quarterly
> Review" and "Revenue increased by 12%" is written into the knowledge base with
> those words and `engines: [rapidocr]`. A second run is a cache hit in 36 ms,
> and `list` neither loads the weights nor extracts them.

### Step 4b — Model acquisition
- [x] Fetch and digest-verify at build time; `include_bytes!` into the binary
- [x] Write to a per-user location on the first page that needs them
- [x] Lazy, so `list`, `show` and `schema` never load a pipeline they do not use
- [x] A test holding the build script's pins against the ones `rapidocr-core`
      expects, since a build script cannot depend on the crate it embeds for

**Verify:** building from a clean tree downloads and verifies the weights once;
thereafter a build reuses the cache and the tools work with no network at all.

> **The pipeline is not ours to write.** `rapidocr-core` (Apache-2.0) implements
> detection, angle classification, recognition, DBNet post-processing, contour
> finding and CTC decoding. Writing those by hand would be a large amount of
> well-trodden work containing no decisions of our own, behind a trait that
> already makes the choice reversible.
>
> **The model call and the decisions are separated.** Ordering, paragraph
> merging and geometry normalisation are pure functions tested against synthetic
> recognizer output, so they do not need fifteen megabytes of weights on disk to
> be checked. A test caught a real fault in the merge rule: it compared vertical
> distance only, so two lines sitting side by side in different columns were
> being read as one wrapped sentence. Horizontal overlap is what tells the two
> apart.

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
- [x] Alignment by bounding-box overlap; text comparison after normalization
- [x] `VerifyPolicy`: off, on low confidence, on critical fields, always
- [x] Conflicts recorded in the IR and marked in the rendered output
- [x] A cost ceiling, so a large scan cannot run away

**Verify:** two engines given deliberately different text produce a recorded
conflict and a rendered marker; a policy of `Off` produces neither.

> Verified end to end, with a real endpoint and the real OCR weights. A page
> reading "Revenue increased by 12%" was put to a stub vision endpoint that
> answered 13%. The artifact came back with `engines: [openai-compatible,
> rapidocr]`, `verified: 1/1`, `conflict_count: 1`, and a `## Conflicts` section
> holding both readings. The same input with `--verify off` produced
> `verified: 0/1` and `conflict_count: 0`, and the OCR engine was never called.
>
> **The position-based path is not the one that runs.** A vision model answers
> with Markdown and no coordinates, so in today's configuration only one engine
> has geometry and there is nothing to align on. Rather than guess an alignment
> and blame the wrong region, the comparison falls back to whole-page text —
> coarser in what it can report, not weaker in what it checks. Region matching
> is implemented and tested, and runs as soon as both engines report boxes.

> **Three things the tests alone would not have caught.**
>
> *The default policy shipped a wrong number.* "Revenue increased by 13%"
> matches no currency, date, identifier or clause number, so the first version
> of the trigger stayed quiet and the disagreement was never looked for. A
> percentage is a value a reader acts on. Percentages and thousands-separated
> numbers joined the pattern set, and the reasoning is now in the code: a
> pattern that fires when it need not costs one free local pass, a pattern that
> stays quiet when it should have fired ships a wrong number.
>
> *The policy was not in the cache key.* Re-running with `--verify always` after
> `--verify off` returned the cached single-engine result, conflicts and all
> absent. Caught by running the binary twice rather than by any unit test.
>
> *A merged paragraph kept only its first line's confidence.* A merged paragraph
> took the score of the line that opened it, so an uncertain line could hide
> behind a confident one — from the one consumer that reads confidence to decide
> whether to check the page. It now carries the least confident line in it.

### Step 7 — Model selection by measurement
- [x] Build the corpus from real documents
- [x] Run every candidate through `bench`
- [x] Record the numbers that chose it

**Verify:** the chosen default is stated with the scores behind it, and
swapping it is a one-line change behind the trait.

> **The step that ran is not the step that was written.** It was planned as
> "fix the default model". It cannot be that: §8.2 settled that DocFusion runs no
> models and picks none — the user chooses, the tool reports what it found. So
> there is no default to fix, and inventing one would have quietly restored the
> thing that decision removed.
>
> What was left to measure was the only question actually open, and it turned
> out to be the interesting one: **on real documents, which engine reads
> better?** 24 pages from OmniDocBench v1.6, both engines, numbers in §8.5.
>
> *(v1.5 is the base subset's name, not the release — corrected 2026-10-13.)*
>
> The answer is that they are close on characters and not close on structure —
> and that they fail on *different pages*, which is the empirical case for the
> role matrix in §6.1 rather than an argument for one engine over the other.

> **The first result was wrong, and wrong in one direction.** DeepSeek wrapped
> whole pages in a fenced block behind an introductory sentence, and our scorer
> could not see text inside a code block. Every affected page scored a content
> error of 1.000 when the engine had read it perfectly. Two fixes, and the
> corpus mean moved from 0.292 to 0.245.
>
> **A measurement that damns the thing being measured is the one to double-check
> hardest.** This one was caught only because a flat 1.000 across two unrelated
> pages is not a plausible reading of two different documents.

### Step 8 — Probing, not detecting
- [x] Probe the configured endpoint rather than inspecting the machine
- [x] Report what was found, and what to do about what was not
- [ ] Check the two things no endpoint can report — **deferred, with reasons**

**Verify:** with no endpoint, the tool reports `OcrOnly` and still processes
text-layer documents and scans through OCR.

> **This step shrinks because of §8.2.** GPU vendor, VRAM, whether a daemon is
> running and which models it holds are questions about somebody else's program,
> and that program answers them better than any inspection we could write.

> **Verified both halves, end to end.** With the endpoint set: `capability
> vision`, the endpoint's models listed, the selected one found, exit 0. With
> `DOCFUSION_CONFIG` pointing at a file holding nothing but a comment:
> `capability ocr-only`, exit 0 — and a scanned page still went in through the
> bundled engine, and a text document through the pass-through, with no endpoint
> anywhere. With the endpoint pointed at a dead port: `capability ocr-only`,
> the problem and what to do about it, exit 1.

> **The listing is not a requirement.** `/v1/models` is not part of what an
> endpoint must implement to read a page, so `Probe` separates "answered, has no
> listing" from "did not answer". The first is a note and exit 0; the second is
> a problem and exit 1. Folding them together would report a working setup as
> broken, which is the mirror image of the failure this project is organised
> against.
>
> **`Capability` was designed as a type and did not become one.** Nothing
> branches on it — the answer is a report, and the tier is a field in it. See
> §7.

> **The two local checks are deferred rather than forgotten.** Free disk space
> costs a platform-specific dependency to guard against a risk that kilobyte
> markdown does not pose, and the WebView2 runtime is for a GUI that is P2. A
> health check that reports everything it *could* measure is longer than one
> that reports what matters.

### Step 9 — The setup guide
- [x] A guide the user's *agent* can execute: exact commands, exact file names,
      and a check that it worked
- [x] Cover `llama-server`, Ollama and LM Studio as interchangeable, since all
      three expose the same API
- [x] State the projector requirement and the download sizes, because a model
      served without its projector fails silently rather than loudly

**Verify:** following the guide on a clean machine produces an endpoint that
`docfusion` reports as working, without any step being guessed at.

> **Written at `docs/setup.md`, outside the knowledge base.** It is a product
> deliverable rather than a working note, and the two have different readers.

> **Every quoted output is output that came back.** The zero-config front
> matter, the `doctor` report, the authentication failure and the exit codes
> were all captured from runs, not written from memory. Two rows of the
> error table were corrected against the real wording after being drafted.

> **Verified, and said so.** The DeepSeek path went end to end. The three local
> runtimes could not be: none is installed here, and **github.com, ollama.com
> and lmstudio.ai are all unreachable from this machine** while `hf-mirror.com`
> and `modelscope.cn` are not. That is a fact about the reader's network that
> the guide has to state rather than step around, and §9 says plainly which
> rows are a vendor's documented commands rather than something that ran.

> **Two defects found while writing it, both in what a person would read.**
> An unreachable endpoint was reported as "no OCR or vision backend is
> configured" — a sentence contradicting the real reason printed beside it in
> parentheses, sending the reader to check a configuration that was already
> correct. Every producer of that error is a broken backend, so the mapping that
> produced it is gone. And the binary was named `docfusion-cli`, which is a fact
> about the repository rather than about the tool; a manual telling somebody to
> run that name has leaked the crate layout.

---

## Out of scope for P1
The GUI, search, cloud provider implementations beyond what is needed to make
the verifier work, and any semantic enrichment of the output.

**Out of scope permanently, as of this revision:** downloading, verifying,
installing, supervising or versioning models and inference binaries. That is a
product category, and it is not this one.
