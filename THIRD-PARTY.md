# Third-party components

DocFusion is MIT licensed (see [LICENSE](LICENSE)). It builds on other people's
work, and this file says whose, under what terms, and where the obligations
land.

Two kinds of thing are worth separating:

- **Code linked into the binaries this repository builds.** Distributed as part
  of a release, so the notices travel with it.
- **Weight files downloaded at runtime.** Not redistributed here at all — the
  program fetches them from their publisher, on request, and verifies them.

## Linked into the binary

| Component | Licence | Notes |
|---|---|---|
| [PDFium](https://pdfium.googlesource.com/pdfium/) — via `pdfium-render` and `pdfium-bundled` | **BSD-3-Clause**, copyright The PDFium Authors, with Apache-2.0 components | Chromium's PDF engine, embedded and extracted to a cache directory on first use. Its upstream `LICENSE` file carries BSD terms for the PDFium Authors and Apache-2.0 terms for included components. |
| [ONNX Runtime](https://github.com/microsoft/onnxruntime) — via `ort` | **MIT** | Runs the OCR and layout models. |
| [Tauri](https://github.com/tauri-apps/tauri) | **Apache-2.0 OR MIT** | The desktop window. |
| [SQLite](https://sqlite.org/) — via `rusqlite` (bundled) | **Public domain** | The manifest and the full-text index. |
| [CodeMirror 6](https://github.com/codemirror/dev) | **MIT** | The Markdown editor in the window. Committed as a single pre-built ES module under `crates/docfusion-gui/ui/vendor/`; see the README in that directory for versions and how to rebuild it. |

## Downloaded at runtime, not redistributed

| Component | Licence | Notes |
|---|---|---|
| RapidOCR's PP-OCRv6 weights (detector, recogniser, angle classifier, dictionary) | **Apache-2.0**, like the rest of PaddleOCR | ~7 MB. Fetched from ModelScope (`RapidAI/RapidOCR`) on request. |
| PP-DocLayout_plus-L | **Apache-2.0**, like the rest of PaddleOCR | ~125 MB. The page structure model, fetched from ModelScope (`PaddlePaddle/PP-DocLayout_plus-L_onnx`) on request. |

Both are verified against a pinned SHA-256 before anything is trusted. The
digests are in `crates/docfusion-store/src/models.rs`, which is the single place
that defines the set.

## Rust crates

Everything else is a crates.io dependency, pinned in `Cargo.lock`. All of them
are permissively licensed; the notable ones:

| Crate | Licence |
|---|---|
| `comrak` (Markdown → HTML) | **BSD-2-Clause** |
| `calamine` (XLSX) | MIT |
| `zip`, `quick-xml` (OOXML, export) | MIT |
| `image` | MIT OR Apache-2.0 |
| `blake3` | CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception |
| `clap`, `ureq`, `thiserror`, `ort` | MIT OR Apache-2.0 |
| `rapidocr-core` | Apache-2.0 |

To generate the full list for a release:

```bash
cargo install cargo-license
cargo license --all-features
```

## The benchmark dataset

The evaluation numbers in the README come from a corpus derived from
**OmniDocBench v1.6**, which is released **for research purposes only, not for
commercial use**. Nothing from that dataset is redistributed in this repository —
no page images, no annotations, no reference files — and the corpus is kept
outside it and referenced by path. See
[Evaluation](README.md#evaluation) for the citation and the caveats.
