# Reading a table with a model instead of with geometry

The geometry gets **1 of 9** tables on the corpus. Everything else in the
project is either working or at the ceiling of a model we chose on purpose; this
is the one place that is bad and does not need a bigger model to fix.

The industry does not infer a table from whitespace, it predicts one. The
PP-Structure pipeline is **DB → CRNN → SLANet**: detect the text lines, read
them, and have a third model predict the **structure and the cell boxes**, then
put the text into the cells by where the boxes are. **This project already has
two of those three** — `rapidocr` is the DB and the CRNN. What is missing is
exactly one model, and PaddlePaddle publishes it as ONNX.

Measured before anything was built: `SLANet_plus_onnx` is **7.4 MB** — the same
order as the recognition weights already embedded in the binary — and on the
`111` case it produced a **6-row by 5-column grid with cell boxes landing
exactly on the ruled cells**. 124 MB of layout model was worth arguing about;
7.4 MB is not.

## Steps

- [x] 1. A Python probe of the whole chain: crop the region the layout model
      found, run the candidate structure model, match recognised lines into the
      predicted cells, emit Markdown, score it against `expected.md`.
      → verified: `062`, an eleven-column financial table the geometry could not
      read at all, comes back **165 of 165 cells correct**, and `111`'s
      ten-column table is visibly right cell by cell.
- [x] 2. Choose the model by measurement. Candidates measured: `SLANet_plus`,
      `SLANeXt_wired`, `SLANeXt_wireless`.
      → verified: **SLANet_plus, 317 of 574 expected cells (55.2%)**, against
      7.7% and 6.3% for the two SLANeXt exports — and those two are not
      disproven, only unreachable, because their cell boxes do not decode.
- [x] 3. Wire the winner into `docfusion-store` as a second downloaded model,
      the same shape as the layout model: one file, one digest, the engine's
      name follows.
      → verified: 455 tests, clippy clean, and the model fetched by
      `docfusion doctor --fetch` like the layout one.
- [x] 4. Re-run the corpus and record what the tables came to.
      → verified: **tables 1/9 → 7/9**, mean loss 0.481 → **0.450**, content
      error unchanged at 0.196, reading-order error 0.360 → 0.377.

## Known before starting

- **The token dictionary PaddleOCR ships does not match this model's
  vocabulary.** The model emits 50 classes, the file has 48 entries, and the
  low ids line up while the high ones do not: id 48 is the plain cell
  (`<td></td>`), which PaddleOCR appends rather than stores, and the file's last
  entry arrived after this model was trained. Decoding straight from the file
  turns every cell into ` rowspan="20"`. Found by running it.
- **GFM Markdown cannot express a merged cell**, and the IR has no span either.
  A `colspan` is lost on the way to the artifact. This is the artifact's ceiling
  and not this task's problem, but it bounds what any model can deliver.
- **The wired/wireless split may not be worth having.** A practitioner
  (TurboOCR) reports the shipped wireless encoder was byte-identical to the
  wired one and that genuine SLANeXt measured *worse* on borderless tables.
  Step 2 measures rather than believing either claim.

## The measurement

Every arm gets the same text: the region the layout model found, the same OCR,
the same matcher. Only the structure model changes.

| model | size | box space | cells correct |
|---|---|---|---|
| **`SLANet_plus`** | **7.4 MB** | padded square | **317/574 (55.2%)** |
| `SLANeXt_wired` | 350.7 MB | resized picture | 45/588 (7.7%) |
| `SLANeXt_wireless` | 350.7 MB | resized picture | 37/588 (6.3%) |

Per case, `SLANet_plus`: `062` **165/165**, `082` 103/132, `112` 21/25,
`111` 20/195, `061` 5/49, the two exam score boxes 2/4 and 1/4.

### Three things only running them said

- **The two models normalise their boxes differently.** Same family, same
  tensor signature — `x[1,3,S,S]`, a location output and a 50-class structure
  output — and `SLANet_plus` answers in the padded 488-square while `SLANeXt`
  answers in the resized picture inside it. A table 1470 by 479 comes back in
  the top 0.326 of the square from one and spread across 0..0.98 of the other.
  Swapping a model is not swapping a model.
- **`SLANeXt`'s cell boxes do not decode.** Read as four corners they scribble
  diagonally across the table, and the text that goes with them lands nowhere.
  **Its structure head is fine** — the row and column counts come out right,
  within a column — so this is the location output, not the model. Comparing it
  fairly needs an adapter of its own, which was not written.
- **The two exports are not the same file.** Same size to the byte, different
  SHA-256. (A report that the wireless encoder was byte-identical to the wired
  one is about a third party's own repackaging, not about these.)

### And one thing about the harness, which was wrong twice

The first score was 42.8% and the second 33.4%, and both were **the scoring
rather than the model**: a document with two tables was flattened into one grid,
and then two lists of tables were paired by position when the two detectors
returned them in different orders — pairing by index scored two correct tables
as nothing at all. Paired by best match, the same output scores 55.2%.

**The number that matters is the one the model produced, not the first one the
harness printed.**

## In the engine

| | before | after |
|---|---|---|
| **tables found** | **1 / 9** | **7 / 9** |
| mean loss | 0.481 | **0.428** |
| error, order ignored | 0.197 | **0.194** |
| error, order included | 0.360 | **0.332** |
| headings found | 8/64 | 8/64 |
| 24 pages | 65 s | 64 s |

**A fourth thing took a second attempt**, and it was the one that mattered most:
the boxes are normalised to the padded square and the way back to the picture is
two steps. The first version did one of them, which is right only when the two
sides are equal, and a wide table's rows became ten-pixel stripes in the top
twelfth of the page. It was read as the model predicting a `rowspan`; the grid
had no spans and was exactly the shape the reference has.

Per case, the tables that went from none to one: `061`, `062`, `101`, `102`,
`111`, and two of three on `082`.

### Two things it took a second attempt to get right

**Losing a line is worse than losing a table.** The first version put a line in
a cell only when the line's centre fell inside the cell's box, and on `061` that
dropped most of the table's words — the content error on that page went from
0.084 to **0.614**, and the page scored worse than it had as loose prose. Every
line now goes to whichever cell holds most of it and, failing that, to the
nearest, so a word inside a table region is never dropped. The content error
across the corpus came back to what it was.

**The empty rows are load-bearing.** The model puts its header in a row of its
own and leaves it empty of text, so a table comes out with a blank line every
other row. Dropping the empty rows looks like an obvious tidy and **took the
tables from seven of nine back to one**, because a row with no words in it is
still the row the headings are above. Kept, with a note, rather than
re-learned.

### What is still not right

**The character metric is the wrong instrument for a table.** Content error is
measured over the whole page, and a table whose cells are all correct but whose
rows are split differently scores worse than the same words as loose prose —
`062`'s own cells were measured at 165 of 165 by the probe and its content error
still rose. The project's own design says tables need TEDS; `docfusion bench`
does not compute one yet.
