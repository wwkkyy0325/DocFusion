# Task: throughput and resource use

**Goal:** a knowledge base of real size is bounded by the machine rather than by
the design, and the bound is one the person with the machine can set.

All three are done. The third was found by measuring the first two, and
measuring the third is what turned up the largest cost of all.

## Done

### Steps 1–2 — Pages read concurrently, and a manifest that tolerates overlap

- [x] The recognition backend keeps several pipelines, built lazily
- [x] The extractor reads pages on several threads, preserving input order
- [x] The verification ceiling is spent in page order, so it lands on the same
      pages every run
- [x] The first failure is reported in page order, not by arrival
- [x] The manifest opens with a write-ahead log and a busy timeout
- [x] Measured: 8 pages, 75.6 s serial against 28.1 s parallel, output identical

**Verify:** a second writer blocks rather than failing, and a document read in
parallel is byte-identical to the same document read one page at a time. Both
have tests; the manifest one was confirmed to fail without the fix.

### Step 3 — Pages arrive one at a time

- [x] `PageSource`: a page is produced when it is asked for and released when
      the work on it is done
- [x] A PDF reopens per page; an image is the one page it is
- [x] Measured: 32 pages, one pipeline, **691 MB → 439 MB** in the same time

**Verify:** the resident set no longer grows with the page count.

> **Measured, and the measurement corrected the diagnosis.** At 32 pages with
> four pipelines this looked like 39 MB, which is not what the arithmetic said.
> One pipeline showed the real 252 MB; four ONNX sessions had been masking it.

## Done, and the cost it turned out to have

### Step 4 — The pipeline count is a setting

- [x] `max_ocr_pipelines` in the configuration file, `--max-ocr-pipelines`, and
      `DOCFUSION_OCR_PIPELINES`
- [x] Below one is one: zero is a mistake, not a setting
- [x] Measured on eight pages: 71 s at one, 41 s at two, 27 s at four

**Verify:** the speed and the memory are both visible, and the choice between
them belongs to whoever is paying for the memory.

> **The dominant term, which is not the one this task was written about.** Each
> pipeline costs about **250 MB** resident — not the 130 MB its weights measure,
> because a session's working memory scales with the page it reads. Four of them
> take 1.2 GB on a 32-page document, against 250 MB for the pages themselves.
> Settling the page memory only made the pipeline cost visible.

> **The arithmetic, because it is the whole argument.** A page rendered at the
> documented 200 DPI is 1654×2339 pixels; as RGBA8 that is **15.5 MB**. Holding
> a thousand of them needs about **15 GB** before any recognition runs, on a
> document the design explicitly contemplates (§6.2 caps verification at a
> thousand pages, so a thousand-page scan is a case that was thought about).
> Ten pages are fine, a hundred are 1.5 GB, and a thousand do not fit.
>
> **This predates today's concurrency change and is unaffected by it** — the
> parallel path holds the same vector. It was found by measuring peak memory
> after that change, which is a reason to measure memory and not only time.
>
> **The shape of the fix.** A page source that yields `PageImage`s one at a
> time: for a PDF, render on demand under the PDFium lock; for a single image,
> the one page it is. The extractor then asks for an index rather than indexing
> a vector it was handed. Peak memory becomes roughly the pages in flight rather
> than the pages in the document — with four threads, about 60 MB instead of
> 15 GB at the thousandth page.
>
> **What to watch.** PDFium is behind one lock, so rendering stays serial; that
> is already true today and is not a regression. Rendering on demand means a
> page is rendered inside a worker rather than before the workers start, so the
> render cost moves from the front of the run into the middle of it. Neither is
> a reason not to do it, but both are things to measure afterwards rather than
> assume.
