# Changelog

Append-only meaningful changes. Newest entries at the top.

---

## 2026-10-09 — Chinese, and the kind of a failure rather than the sentence about it

**What:** The window is in Chinese and English, opens in whichever the machine
is set to, and switches with a header button. The instruction was that
everything a person sees, apart from the application's name, should be in
Chinese — including what the engine says when it fails.

**The interface's own words were the easy half.** They are all in `app.js`, and
`draw(view)` is already a function of the view, so the language is one more
thing in that function and switching is "change it and draw again". That is why
a switch costs almost nothing here and why it was chosen over Chinese alone: the
architecture had already done the work.

**The engine's words were the half that needed a decision.** A `thiserror`
message is English, and it is the same string the command line prints. A window
that had to show it in Chinese could only match on the sentence, which comes
apart the moment somebody rewords it — silently, in a place nobody looks. So the
failure now crosses the boundary in two halves: a `Cause`, which is the kind
(`unsupported`, `engine-failed`, `unreadable`, `unwritable`, `index`, `missing`,
`settings`, `nothing-to-read`), and a `detail`, which is the account as it
arrived. The frontend holds one sentence per kind and prints the account
underneath it.

**The mapping is an exhaustive `match`, and that is the point of it.** `cause_of`
in `lib.rs` goes over every variant of the library's errors with no catch-all,
so adding a way to fail stops the build rather than shipping a message that says
nothing. There is a test that walks all eight kinds and a test that pins the
names, because the frontend looks them up by string and that is a contract
between two languages.

**One thing is still English and it is not an oversight.** `detail` is somebody
else's words — `reqwest` said the connection was refused, the OS said
`os error 10061`. Translating that means guessing what a third-party crate meant
and writing the guess in Chinese, which is inventing an explanation rather than
reporting one. It is shown as it came, set apart so it reads as a quotation.

**A document that will not read is no longer an error on the wire.** `process`
used to put the failure in the view *and* reject with the string, so the
frontend had two ways to say the same thing and two chances for them to
disagree. It now returns the view, and only a task that never finished rejects.

**The check caught its own flaw.** The first `switched` shot set the remembered
language to Chinese and then clicked the switch, so it showed English and proved
nothing about the direction it was meant to test. Fixing it exposed a second
problem: headless runs were sharing a profile, so one shot's remembered language
decided the next shot's starting language. Each shot now gets a profile of its
own, and `default` — no language pinned at all — is kept in the set precisely to
show what the machine's own answer comes to on this machine, which is Chinese.

---

## 2026-10-09 — The drawer that would not close, and a window that said nothing

**What:** Three defects in the window, all reported from using it. The records
drawer could not be closed; the interface appeared to freeze while a document
was being read; and the drawer is now two tabs instead of an overlay.

**The drawer could not close because `hidden` was losing.** `aside` had
`display: flex`, which is an author rule, and `hidden` only works through a
user-agent rule saying `display: none`. An author rule beats a user-agent rule
for the same property, so `drawer.hidden = true` set the attribute and changed
nothing on screen. The fix is stated once for the whole frontend —
`[hidden] { display: none !important; }` — rather than remembered at each place
something is hidden, which is how it was missed the first time.

**The freeze was not a freeze.** Nothing was blocking: the engines are built
lazily, a read runs on the blocking pool, and the database write happens after
extraction as a single short statement, so the window's own thread was never
held. What was wrong is that the interface never drew the waiting state. Rust
sets `Working` as soon as a read starts, but that state only reaches the
frontend when the call returns, and the call is the thing that takes minutes.
So `run()` now draws the waiting screen *before* asking for the work, with a
clock that counts up. Long enough a delay that opening an already-read document
does not flash a spinner for one frame, short enough that a real read shows one
immediately. The clock is the point: it is the evidence the process is alive.

**Tabs, because a drawer is an overlay.** It covers the document, it has to be
dismissed before the document can be read, and switching between the two moved
things that were not switching. Two sibling panels cost one `hidden` attribute
each and leave the layout where it was.

**Verified in a browser rather than by looking at the window.** The application
could not be photographed: its window is composited by the system rather than
drawn into a device context, and a screen capture from a session whose desktop
is not rendering comes back uniformly black. Screenshots of the real window
would have been evidence of nothing. Instead the three `ui/` files are served
to a headless browser with `invoke` stubbed out, one scenario per state, and
the resulting pictures read. Everything below the `invoke` boundary is the file
that ships, so the layout, the tab switching and the clock are the real ones.
The scenario that matters is `working`: the stubbed read never resolves, which
is the case the old code would have sat through showing an unmoving empty page.
The harness is `target/uicheck/`, and it is thrown away with the build.

---

## 2026-10-08 — The window

**What:** `docfusion-gui`, a Tauri v2 application with a static frontend and no
build chain. Drop or choose a document, read it, see it rendered, copy it as
plain text. A records drawer lists what is in the knowledge base.

**No bundler, and that held up.** The `ui/` folder is three files loaded as
they are. The design chose that before there was code; building it confirmed it,
because the only hard part of this interface is rendering Markdown and that is
`comrak` in Rust — `render::html`, which the command line wanted anyway. What is
left for JavaScript is a view over a JSON value, plus the two things a webview
does better than Rust: the clipboard and knowing what is selected.

**Raw HTML in extracted text is escaped, not passed through.** The renderer says
so explicitly and has a test for it, because this output goes into a webview
where markup that passes through is script that runs.

**The engines are built by one function now.** `docfusion_store::engines` is
what both the command line and the window call. Two lists that agree today are
two lists that disagree after the next change, and a window that reads a
document differently from the command line is two products with one name.

**The configuration file moved out of the command line** and into the store, for
the same reason: it now has two readers.

**One knowledge base, one place.** The default was `knowledge`, relative, which
is right in a shell and wrong for an application started from a shortcut — it
would resolve somewhere different every launch, and the window and the command
line would open two libraries. Both now default to `Documents/DocFusion`, and
`--root` still overrides it.

**A wrong first diagnosis, worth recording.** The window appeared to clip its
content about two thirds of the way across: the heading cut mid-word, the
buttons overflowing an invisible edge. It was the screenshot. My capture took
the window's *position* from a call that returned the wrong origin while taking
its *size* from the window, so every image was a slice of the desktop with the
window's left portion in it. Asking for a 1400-pixel window settled it in one
look: everything renders, and the layout was never wrong. Two fixes were made
along the way that were worth making anyway — the empty state no longer crams
two forty-character paths into a centred sentence — but the bug was in the
instrument, not the thing being measured.

---

## 2026-10-08 — The machine decides how many pages to read at once

**What:** the reader count is no longer a constant. `docfusion-store::memory`
asks the operating system for its physical memory, takes a quarter of it,
divides by the 250 MB a reader costs, and caps the result at the core count.
`docfusion doctor` now says what it worked out — on this machine, *31.2 GB, so
16 pages at once* — and the number is used for both the extractor's threads and
the recognition engine's pipelines, because two constants for one quantity drift
apart and whichever is larger is wasted.

**Where it stops paying.** Reading eight pages takes 71 s at one reader, 16 s at
eight and 17 s at sixteen: **two gigabytes for no measurable gain.** On sixteen
pages the sixteenth reader is worth 14%. So the rule is what the user asked for
— a quarter of the machine — and the measurement says the useful number on a
sixteen-core machine is nearer eight. That is one flag, and the guide says so
rather than pretending the budget is also the optimum.

**`docfusion doctor` gained the machine's memory**, and the figure was checked
against what Windows reports: 33,476,141,056 bytes, exactly what the direct call
returns.

**The GUI question, answered in the design.** Can the Tauri shell talk to the
crates directly? Yes, and it is the plan — the webview is a view over the same
process, so there is **no port and therefore no port conflict**. That is not a
lucky accident of Tauri but a consequence of not having a service: port clashes
are the tax a desktop application pays for being two programs rather than one,
and a `docfusion serve` would have reintroduced them for something the command
line already does. §11 now says so, and says what a genuine long-running mode
would need if one is ever wanted.

**The setup guide gained §9**, for a program driving the tool rather than a
person: point at `docfusion schema` rather than at the document, the two rules
the output keeps, the exit codes, what is idempotent, that two processes may
overlap, and that credentials belong in the file rather than in an argument.

---

## 2026-10-08 — Pages are rendered when they are wanted

**What:** `PageSource` replaces the vector of decoded pages. A page is rendered
when its turn comes and released when the work on it is done. Measured over 32
pages with one pipeline: **691 MB → 439 MB**, in the same 265 seconds.

**The measurement corrected the diagnosis that prompted it.** At 32 pages with
four pipelines the change appeared to save 39 MB, which is not what the
arithmetic predicted. Isolating with a single pipeline showed the real 252 MB —
it had been masked, because four ONNX sessions cost about **750 MB** and
everything else is a rounding error beside them.

**Each pipeline costs about 250 MB resident, not the 130 MB its weights
measure.** A session's working memory scales with the page it is reading. Four
of them were taking 1.2 GB on a 32-page document while the pages themselves took
250 MB — the thing I set out to fix was the smaller half.

**So the pipeline count is now a setting**, `max_ocr_pipelines`, because a
3.5× speed-up for three quarters of a gigabyte is a good trade on one machine
and a bad one on another, and that is a decision the person with the machine
should get to make. Measured on the same eight pages: **71 s at one, 41 s at
two, 27 s at four.** Below one is one, since zero would read nothing.

**The unbounded part is gone.** Held pages scaled with the page count — fifteen
megabytes each — so a thousand-page scan accumulated towards eight gigabytes on
top of everything else. That term is now bounded by the pages in flight.

---

## 2026-10-08 — Reading a document's pages at the same time

**What:** pages are read concurrently, and the manifest survives two runs
overlapping. Measured on eight pages, local OCR only: **75.6 s → 28.1 s**, and
the output is byte-identical.

**Two things had to change, and only one of them was obvious.** The extractor
had to stop reading pages one after another — but the recognition backend also
had a single pipeline behind a `Mutex`, so concurrency above it would have
queued rather than worked. The backend now keeps up to four, built lazily, so a
one-page document still pays for one.

**The obvious lever did nothing.** ONNX Runtime defaults to one thread per
inference, and one page takes about three seconds on one core and about three
seconds on sixteen. The recogniser runs once per detected text line on a crop
too small for threads to pay for themselves, so widening a single page is a dead
end. That change was written, measured, and reverted, and the measurement is
recorded where the next person will look for it.

**Determinism was the constraint on the design.** The ceiling on verification
has to land on the same pages every run, so it is spent in page order before any
checking rather than while checking, where it would depend on which page
happened to finish first. A document whose contents differ between runs over the
same input is a document nobody can diff. The first failure is likewise reported
in page order rather than by arrival, and the parallel output was diffed against
the serial one to prove it.

**The cost is memory: 923 MB peak** for eight pages, against about 400 MB
serial. Reading more at once is a straight trade against resident pages, and
four is where that trade was drawn.

**A measurement that nearly went into the record wrong.** The first A/B showed
8% and meant nothing: the configuration file supplies a DeepSeek endpoint, so
both runs were network-bound API calls rather than local OCR. Re-run with no
endpoint configured, the difference was 2.7×.

**Found while measuring, and not fixed: a large scan will exhaust memory before
it starts.** `PageImage` is raw RGBA8 — a 200 DPI A4 page is 15.5 MB — and
`render_pages` renders *every* page of a PDF up front and holds them all. A
thousand-page scan therefore needs about 15 GB before the first page is read.
This predates today's change and is unaffected by it; see the task doc.

---

## 2026-10-08 — The setup guide, and P1 done

**What:** `docs/setup.md` — the document somebody's agent is handed. Every
command in it was run, and every quoted output is output that came back. P1 is
complete.

**Two defects surfaced by writing it**, both in what a person reads:

- **An unreachable endpoint was reported as "no OCR or vision backend is
  configured."** The real reason trailed in parentheses behind a sentence
  contradicting it — the worst shape a message can take, because it is
  believable and sends the reader to check the thing that is already right.
  Every producer of that error is a broken backend, never an unreadable file, so
  the mapping that produced it is deleted rather than corrected. An unreachable
  endpoint now says `the scan engine failed: could not reach http://...` and
  exits 1, where it used to exit 3 claiming nothing was configured.
- **The binary was called `docfusion-cli`.** That says which crate it is, which
  is a fact about the repository rather than about the tool, and a manual
  telling somebody to run it has leaked the crate layout.

**The guide says what it could not verify.** None of llama-server, Ollama or LM
Studio is installed on the machine this was written on, and **github.com,
ollama.com and lmstudio.ai are all unreachable from it** while `hf-mirror.com`
and `modelscope.cn` are not. Those three rows are the vendors' documented
commands and are labelled as such. Everything else ran. A guide that mixes
tested and untested steps without saying which is which is the same failure as a
metric that cannot tell misread from misordered.

**P1 is finished.** Ten steps: the bench harness, page images, the backend
traits, the local engine and its weights, the endpoint client, verification, the
baseline, `doctor`, and this. What P1 set out to make true — scans are read,
disagreements are visible, and no engine is wrong quietly — is now true, and
measured rather than asserted.

---

## 2026-10-08 — `docfusion doctor`

**What:** one command that answers "what can this installation read, and what is
missing to read more". Probes the configured endpoint rather than inspecting the
machine. P1 Step 8.

```
knowledge base  D:\...\knowledge  (not created yet)
engines         pdfium, docx, xlsx, pptx, text-passthrough, scan
endpoint        https://api.deepseek.com
  serves        deepseek-flash, deepseek-v4-pro
  selected      deepseek-flash  (listed)
capability      vision
```

**A missing model listing is not a fault.** Nothing requires an endpoint to
implement `/v1/models`, so `Probe` now separates "answered, has no listing" from
"did not answer". The first is a note and exit 0; the second a problem and exit
1. Folding them together would report a working setup as broken — the mirror
image of the failure this project is organised against, and just as misleading.

**No endpoint is a supported configuration, not a problem.** Exit 0, tier
`ocr-only`, and a line saying so. Verified end to end: with no endpoint
configured, a scanned page still goes in through the bundled engine and a text
document through the pass-through.

**`Capability` was designed as a type and did not become one.** Nothing branches
on it; the answer is a report and the tier is a field in it. §7 updated rather
than left describing a type that does not exist.

**The two local checks are deferred with reasons** rather than done for
completeness: free disk space would cost a platform-specific dependency to guard
against a risk that kilobyte markdown does not pose, and the WebView2 runtime is
for a GUI that is P2. A health check that reports everything it *could* measure
is longer than one that reports what matters.

**One test was costing the suite three minutes.** The "endpoint says nothing"
case used the stalled-server helper, which waits out the real 180-second
timeout. A closed port fails identically and instantly. A test suite people stop
running is a test suite that stops finding things.

---

## 2026-10-08 — The first real baseline, and the bug that made it a lie

**What:** both engines run over the 24-page corpus. DeepSeek-V4.1-Flash against
the bundled local recogniser. Numbers in design §8.5.

**Result: close on characters, not close on structure.** Reading order 0.245
against 0.466; content 0.171 against 0.203; headings 9/64 against 0/64; tables
7/9 against 0/9. The structural half is the difference a vision model buys.

**And they fail on different pages.** A Chinese newspaper page: the local engine
recovered 97.7% of the reference's characters, the VLM 72.8%, cleanly omitting
the masthead, editor, proofreader and phone number. The provider turns a page
into at most ~1024 image tokens, which downsizes a 2754×3939 scan to roughly
900×1280, and fine print does not survive that. That single page is the argument
for the whole two-engine design, measured rather than asserted.

**Two bugs the run exposed, neither of which a test would have found:**

- DeepSeek wrapped entire pages in a fenced block behind a sentence: "Here is
  the transcription of the page in GitHub-flavoured Markdown." The fence is
  packaging, not content, and left in place the page became one verbatim code
  block with no headings and no tables. `strip_envelope` now unwraps a single
  fenced block, with or without one introducing sentence, and only that.
- **The scorer could not see inside a code block**, because a fenced block's
  content is a literal rather than a text node. A page read perfectly and
  returned inside a fence scored a content error of 1.000 — read as "returned
  nothing" when it had returned everything. The most misleading answer a metric
  can give. Fixed; text inside a code block now counts as text.

Between the two, cases 011 and 012 went from 1.000 to 0.089.

**A note on why the run was worth the trouble.** Every number in the first
result was wrong in the direction of condemning the VLM. Reporting it as it came
out would have been a confident, well-measured, false answer — which is the
failure this project is built around not making.

**Raised but not resolved:** when the verifier recovers text the primary
dropped, the document still contains the primary's reading. The conflict is
recorded and neither reading is preferred, but nothing is resolved, and the
geometry-free comparison cannot classify a dropped region as the dangerous class
it is because it cannot localise it. Both belong with arbitration.

---

## 2026-10-08 — One place to put the key

**What:** `%LOCALAPPDATA%\DocFusion\config.toml` holds the settings that would
otherwise be retyped on every run. Precedence is flag → environment variable →
file → built-in default. `docfusion schema` publishes the path and the keys.
Design §10.2.

**Per user and outside any repository**, because it holds an API key. A config
file beside the documents is a config file that eventually gets committed. It
shares a directory with the extracted weights, so "where does DocFusion put
things" now has one answer rather than two — `dirs::user_dir` is the single
definition, and the model cache defers to it.

**A misspelt key is an error.** `deny_unknown_fields` turns `api_ky = "..."`
into a failure naming the file, the line and the valid keys. Silently ignoring
it would mean running against the wrong endpoint with nothing to say why.

**The settings that have defaults had to become `Option`.** A default applied by
the argument parser can never be overridden by a file, so `root`, `verify` and
`max_verify_pages` are now resolved in one place after the file is read. That is
a small improvement on its own: the defaults are stated once instead of being
scattered through attributes.

**`--json` and `--dry-run` are deliberately not settable.** They describe what
this run should do rather than how the tool is set up, and a file that quietly
made every run a dry run would be a trap rather than a convenience.

---

## 2026-10-08 — A real corpus, and the metric it broke

**What:** 24 pages sampled from OmniDocBench v1.5, converted into a `bench`
corpus, and run against the local recognition engine. `Score` gained a second
error rate. The baselines are in design §8.5.

**The metric was wrong, and the corpus is what showed it.** A three-column
newspaper page: the engine produced **97.7% of the reference's characters** and
scored a character error of **0.978**. It had read the page almost perfectly and
laid it out *across* the columns instead of *down* them. A single ordered rate
cannot tell "misread" from "misordered", and those need different answers.

`Score` now reports both — the order-sensitive rate, and the same comparison with
order taken out. Across the corpus the local engine is at **0.466 with order and
0.203 without**: about a fifth of the characters are genuinely wrong, and another
quarter of the reference is present but in the wrong place. The multi-column
cases show it starkly (0.776 → 0.028, 0.978 → 0.102); the single-column prose
cases barely move.

**Headings 0/64 and tables 0/9 are structural, not a measurement.** A line
recogniser has no concept of a heading or a table, so those two always score
zero for it. Comparing `loss` between this engine and a vision model would
conflate reading badly with not doing structure at all. Content and order rates
are the comparison; the structural counts describe the engine.

**Three known limits, recorded rather than corrected:** the reference Markdown is
generated by our converter (OmniDocBench ships region annotations, not Markdown)
so the rules decide what counts as correct; math-heavy pages are contaminated by
the dataset's own LaTeX spacing, which no engine reproduces; and the pages are
born-digital renders, so every number is optimistic about a real scan.

**Two things the corpus caught in the converter itself.** An allowlist of
categories silently dropped `equation_isolated` and `table` — the table metric
would have measured nothing at all. It is now a denylist, because a dropped
region is invisible until somebody puts the image beside the reference. And page
furniture carries no reading order by convention, so headers and footers were
being dropped even though every engine reads them.

**Also measured: 15 seconds a page**, worst case 56, on a CPU. That is slow
enough to matter for a knowledge base and is now a number to improve rather than
an impression.

---

## 2026-10-08 — Two engines, and the disagreement is kept

**What:** the second engine now checks the first. `docfusion-core::verify` holds
the policy and the comparison; `ScanExtractor` runs the verifier and records what
it finds; the renderer writes the disagreement into the artifact. New flags:
`--verify <off|doubt|fields|always>` and `--max-verify-pages`. P1 Step 6 is done.

**Two engines read every page, and neither reading wins.** The verifier is
whichever engine did *not* produce the primary result — today the local
recognition engine checking a vision model's Markdown. A page nobody checked and
a page that passed now say so separately: front matter carries `verified: 1/1`,
and a `## Conflicts` section shows both readings verbatim. Nothing marks one as
preferred, because the tool has no basis for choosing and a report that quietly
picked would be the silent resolution this project exists to avoid.

**The position-based alignment is not the one that runs.** A vision model answers
with Markdown and no coordinates, so only one engine has geometry and there is
nothing to align on. Rather than guess and blame the wrong region, the comparison
falls back to whole-page text. That is coarser in what it can report and not
weaker in what it checks — a dropped region still changes the character stream —
but it means conflicts are recorded with no region rather than an invented one.
Region matching is implemented, tested, and runs as soon as both engines report
boxes.

**Normalization folds whitespace and nothing else.** Spacing is much the largest
source of noise between a recognition model and a language model and is not a
disagreement about the document. Case, punctuation and character identity are
exactly what the comparison is for.

**The policy is part of the cache key.** It decides what the output says, so
omitting it served a cached single-engine result to a run that asked for a second
opinion. Found by running the binary, not by a test.

**Three findings from running it rather than testing it:**

- The default trigger shipped a wrong number. "Revenue increased by 13%" matched
  no currency, date, identifier or clause number, so the check never ran.
  Percentages and thousands-separated numbers joined the pattern set: a trigger
  that fires when it need not costs one free local pass, and one that stays quiet
  when it should have fired ships a wrong number.
- The OCR engine kept only the first line's confidence on a merged paragraph, so
  an uncertain line could hide behind a confident one — from the one consumer
  that reads confidence to decide whether to check the page.
- `ConflictKind::Missing` was removed. It described the same failure as
  `StructuralMismatch` — a region one engine saw and the other did not — and two
  names for one thing invites recording the same fault under whichever came to
  hand.

**Deferred:** arbitration. `Resolution` and `ResolutionSource::Arbiter` remain in
the IR, but a third engine has to be a third lineage and the stack has two.
Shipping the setting before there is anything to fill it would be a control that
silently does nothing.

---

## 2026-10-08 — The OCR weights ship with the tool

**What:** PP-OCRv6 tiny is fetched and digest-verified at build time, embedded
with `include_bytes!`, and written to a per-user location the first time a page
needs reading. P1 Step 4 is done.

**Measured, not estimated.** The set is 1.8 MB of detector, 4.5 MB of
recogniser, 0.6 MB of angle classifier and 27 KB of dictionary — **6.9 MB**,
less than half what the design assumed. At that size there is no trade-off to
weigh between shipping the weights and downloading them on first use, so the
strong form of "works with no setup" is affordable: offline from install,
nothing to configure, nothing fetched at run time.

**Decisions and why:**

- **Extraction is lazy.** Loading the weights takes seconds. `list`, `show` and
  `schema` never read a page, and paying for a pipeline they do not use would be
  a visible cost on every command line.
- **The pins live in two places, and a test holds them together.** A build
  script cannot depend on the crate it is embedding for, so the filenames and
  digests appear both in `build.rs` and in `rapidocr-core`. A test compares the
  two lists: without it, an upstream update would leave the crate looking for
  weights that are no longer embedded, and the symptom would be a model that
  reads nothing rather than a build that fails.
- **Extraction writes through a temporary file.** A run interrupted half way
  would otherwise leave a short file that the size check accepts as complete.
- **The weights go in a per-user location, not beside the knowledge base.** They
  are the same for every knowledge base, and extracting 6.9 MB into each one
  multiplies the cost by however many there are.

**Verified end to end with nothing configured:** an image reading "Quarterly
Review" and "Revenue increased by 12%" is written into the knowledge base with
those words and `engines: [rapidocr]`. A second run is a cache hit in 36 ms;
`list` neither loads the weights nor extracts them.

**The pipeline is not ours.** `rapidocr-core` implements detection, angle
classification, recognition, DBNet post-processing, contour finding and CTC
decoding. Writing those by hand would be a large amount of well-trodden work
containing none of our own decisions — and it sits behind the `OcrBackend`
trait, which is what makes the choice reversible if it disappoints.

A test caught a real fault in the paragraph merge rule along the way: it
compared vertical distance only, so two lines side by side in different columns
were being read as one wrapped sentence.


## 2026-10-08 — The endpoint reached from the command line

**What:** `--endpoint`, `--model` and `--api-key`, with matching environment
variables, and `ScanExtractor` registered once a backend exists. Scanned
documents now go in and Markdown comes out.

**Decisions and why:**

- **A missing model name is refused at parse time.** An endpoint may serve
  several, so guessing would silently produce whatever it happened to pick, and
  the failure would surface as bad output rather than as a configuration error.
- **The text-layer engine stays registered first.** Engines that are equally
  plausible keep their registration order, and parsing a PDF is free and exact
  while rendering it costs an image and a model.
- **Configuration is flags and environment variables, not a file.** An agent
  driving the tool cannot guess where a configuration file is; `docfusion schema`
  publishes both forms.

**A rendering fault the benchmark had missed.** Running the scan path end to end
produced list items separated by blank lines:

```
- first

- second
```

In Markdown that is a *loose* list, which a reader renders with each item
wrapped in a paragraph — the same words presented as something else. Adjacent
list items are now written without a separator, because they are one list.

The benchmark could not have caught this, and that is worth recording. Its
scoring compares headings, tables and text; a loose list has all three right.
Metrics have gaps, and this was one — the fault was found by looking at output
rather than at a score.


## 2026-10-08 — The vision endpoint client

**What:** The OpenAI-compatible client that reaches `llama-server`, Ollama,
LM Studio, vLLM or a hosted provider. P1 Step 5.

**Decisions and why:**

- **One client, every runtime.** That is the whole point of §8.2: the endpoint
  is the unit of configuration rather than the runtime, so nothing here needs to
  know what is on the other end.
- **`temperature: 0`.** Recognition is not a creative act, and a model that
  paraphrases is worse than one that does not.
- **A short, plain prompt.** Document-tuned models are trained against a
  particular prompt shape and do worse when it is dressed up; a long instruction
  costs tokens on every page for a benefit nobody has measured.
- **The prompt version is part of the engine stamp.** Editing a prompt changes
  the output while the model stays the same, so a cache keyed only on the model
  would serve the old one.
- **`Caps` claims nothing.** A general vision model reports neither geometry nor
  confidence, and claiming otherwise would let the verifier align on boxes that
  do not exist.
- **A low token count is treated as the projector failure.** A GGUF model served
  without its multimodal projector does not fail; the image never becomes vision
  tokens and the model returns repeated nonsense with a 200. The check is
  skipped when the endpoint simply does not report usage, because a missing
  field is not evidence of a problem and failing on it would break working
  setups.
- **A timeout names the likely cause.** A model still loading and a model too
  large for the machine look identical from outside, and "did not answer within
  180s" alone would send somebody looking in the wrong place.

**Testing note:** the tests run a real HTTP server on an ephemeral port rather
than mocking the transport. The client is the thing under test, and mocking
below it would test the mock — including for the cases that matter most, which
are the failures: an endpoint that accepts a connection and then says nothing,
one that is not listening, and one that returns a status rather than a body.


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
