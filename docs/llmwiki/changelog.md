# Changelog

Append-only record of meaningful changes. Newest entries at the top.

---

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
