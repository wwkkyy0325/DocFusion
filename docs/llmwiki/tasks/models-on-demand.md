# Models on demand, from the window

## Why

A run through the window produced Markdown with no table in it. Nothing was
broken: the two models the engine reads a page with — the layout model
(`PP-DocLayout_plus-L`, 123 MB) and the table model (`SLANet_plus`, 7 MB) — had
never been fetched. `docfusion doctor` said so plainly:

```
structure       not downloaded; `docfusion doctor --fetch` gets it (123 MB)
tables          not downloaded; `docfusion doctor --fetch` gets it (7 MB)
capability      ocr-only
```

Every measurement in `table-structure.md` was taken under
`DOCFUSION_HOME=target/layout-home`, a home that had the models in it. The
window runs under the real home. Both numbers were true, about different
installations.

`RapidOcrBackend::bundled()` drops the layout model and the table model when
they are not on disk, and says nothing about it. The front matter's
`engines: [rapidocr]` was the only sign, and the settings tab claimed tables
were impossible anyway.

## Steps

1. [x] Fetch the models and verify the pipeline end to end — verify:
       `docfusion doctor` reports both present, and case 062 reads as
       `engines: [rapidocr+layout+tables]` with its table intact. Measured
       before and after on the same page: without the models the 11×11 table
       comes out as a hundred loose lines, with them as a table.
2. [x] Say which local models are here, in the view — verify: `View.local`
       carries both flags and the bytes still to fetch; `missing_bytes` has a
       test for each of the three states a file can be in.
3. [x] Make the availability check forgettable, which is what makes step 4
       mean anything — verify: `layout::Cached` is asked once, is not asked
       again while it remembers, and is asked again after `forget`.
4. [x] A button that fetches them, with progress and a stop — verify: the
       command's pieces are tested (`Cached`, `missing_bytes`, and `ensure_of`
       against a local server), the frontend parses, every element it looks up
       exists in the markup, and every key it writes has both languages. The
       button itself is pressed by hand: no test can click it.
5. [x] Correct the conflict note, which says the engine inside "does not know
       what a table is" — verify: both language strings no longer make that
       claim.
6. [x] Drop `layout::ensure_table`, which nothing calls — verify:
       `cargo clippy` is clean with the function gone.
