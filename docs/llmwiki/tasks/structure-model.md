# Wiring the structure model into the local engine

The downloader landed first (`feat: fetch the structure model`). This is the
other half: running PP-DocLayout_plus-L over a page and using what it finds to
give the recognised words their shape.

Measured before any of this was written: the bundled engine finds **0 of 64
headings and 0 of 9 tables** on the project's corpus, and PP-DocLayout finds
**7 of 7 pages with tables** at about **405 ms a page**. The measurement is what
justified the work; the wiring is what turns it into output.

## Steps

- [x] 1. `layout::default_root()` — one answer to where models live, used by
      `doctor` and the detector alike.
      → verified: `doctor` prints the same path it printed before.
- [x] 2. `structure.rs`: the twenty classes and their kinds, the region type,
      and `decode` over the model's flat output.
      → verified: seven unit tests on `decode`, no weights needed.
- [x] 3. `structure.rs`: `blocks_from` — assign each recognised line to the
      region that holds it, and build the blocks.
      → verified: twelve unit tests over hand-built regions and lines.
- [x] 4. `structure.rs`: table reconstruction. Global columns from the gaps that
      run through the whole table, cells from the lines that stack inside one
      column, rows from where the cells cross.
      → verified: a test that renders the result and gets a Markdown table back.
      A `Table` with no rows renders as nothing at all, and three tests failed
      on exactly that before the fallback was fixed.
- [x] 5. `Detector`: the ONNX session, the resize to 800×800, the two extra
      inputs, lazily loaded.
      → verified: the ignored end-to-end test reads a rendered page through the
      real model and finds the title at the top left. See the coordinate bug
      below — the first version of that assertion passed with a wrong answer.
- [x] 6. `ocr.rs`: attach the detector when the model is here, and take a
      different engine id when it is, so that a cached page is never served for
      a run that would have produced a different one.
      → verified: `cargo test` (433 passing), clippy clean, then a real page.
- [x] 7. Read a page with a table through the whole tool, with the model and
      without it.
      → verified: with `rapidocr+layout`, a five-row Markdown table and a
      heading; with `rapidocr`, the same words as loose lines with the values
      in the wrong rows.

## The coordinate bug

The model answers in the coordinates of the 800×800 square the page is squashed
into, and the reference drawing that the first measurement was read from treated
them as page coordinates. On an A4 page the answer lands in the middle — inside
the page, near enough its own region to look right. Found when the end-to-end
test put a title half a page below where the title is.

The classes and scores are decided before the mapping is applied, so every count
in the earlier measurement stands. Only the drawing was wrong. Both the code and
the probe now map the boxes themselves.

## Not in this step

- Reading order across columns. Regions are ordered top to bottom then left to
  right, which is right for one column and wrong for two. Named here rather
  than discovered later.
- Dropping running headers and footers. The model labels them; they are read as
  ordinary text for now, which is what every other extractor in this project
  already does with them, so this is not a regression — only an improvement not
  taken yet.
- List items. The model has no list class, so `lists` stays 0.
- A document title. The model calls a title `paragraph_title` rather than
  `doc_title` on the pages measured, so it comes out as `##` and `#` is rarely
  reached. Promoting the first title on the first page would fix it and would be
  a heuristic; not done.
