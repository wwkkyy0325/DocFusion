# Export, and the copy button that was doing two jobs

## Why

The status bar had one **Copy** button. It put the whole document's plain text on
the clipboard, which is what somebody wants when they are pasting a page into a
chat window — and is the wrong answer to every other thing somebody wants from a
document they have just had read for them.

Two of those other things were not reachable at all:

- **A table.** Copying a table gives the words with the columns gone. A file that
  a spreadsheet can open is the only useful form of a table outside this program,
  and there was no way to get one.
- **A block.** The thing most often wanted out of a document is not the document.
  It is the one table on page four.

## Decisions

The rule the whole feature is built on:

> **An export keeps something the words do not have.** A button that gave the
> words would be a second button doing the first one's job.

That is what decides every cell of the table, and it is what killed the first
draft — which offered plain text for a block, and markdown-for-everything, and
so had four buttons on a paragraph where three of them did the same thing.

| what | formats | why |
|---|---|---|
| whole document | Markdown, HTML, plain text, CSV | a document is not a thing to paste, it is a thing to keep |
| a table | Markdown, HTML, **CSV** | a table must come out as a table |
| anything else | Markdown, HTML | **no plain text** — that is what Copy is for |

- **One Copy, many Exports.** Copy stays, in the corner of every block, because
  it is genuinely the fastest way to move words. What goes is the *document-wide*
  Copy in the status bar, which the per-block one now covers better.
- **A `dialog`, not a menu.** One button per format, each saying what it keeps.
  The project has a documented failure with hand-built popup menus and a
  documented success with `dialog` for the delete question; this is the second
  one, for the same reasons (focus trap, Escape, backdrop, inert page behind).
- **The pane numbers its blocks**, rather than the window counting them.

## Shape

```
crates/docfusion-core/src/export.rs      Format, write, wants_a_mark   (new)
crates/docfusion-core/src/render.rs      blocks(), plain(), body_of()
crates/docfusion-cli/src/main.rs         docfusion export
crates/docfusion-gui/src/lib.rs          window_html(), the export command
crates/docfusion-gui/src/state.rs        (unchanged — no new View field)
crates/docfusion-gui/ui/app.js           blockActions, askExport, runExport
crates/docfusion-gui/ui/style.css        .block, .block-actions, .formats
crates/docfusion-gui/ui/index.html       the export dialog
```

## Steps

### 1 — what can be written

1. [x] `Format` with `md | html | txt | csv`, `parse`, `extension`, and one rule
       for what suits what: `suits(table) = self != Csv || table`. A first draft
       had it backwards and refused **Markdown** for a table.
2. [x] `write(markdown, format, block)` returning `Option`, `None` for a
       combination that does not suit — refused rather than approximated, because
       a caller that gets an empty string cannot tell it from an empty document.
3. [x] CSV from the Markdown rather than from the tree, RFC 4180 quoting, split
       on the `|` that are bars (a cell holding one is written `\|`).
4. [x] A byte-order mark for a spreadsheet and for nothing else — verify: a test
       for each of the four formats, plus `efbbbf` on a real `.csv` by hand and
       `2d 2d 2d` on a real `.md`.
5. [x] `wants_a_mark` applies only to a **file**. A mark at the front of a pipe
       is three bytes the reader did not ask for.

### 2 — the blocks a document is made of

6. [x] `render::blocks` — a `Rendered { table, html, markdown }` per top-level
       comrak node, and `html()` redefined as `blocks().join("")` so that the
       nth element of the pane and the nth block are the same thing by
       construction rather than by convention.
7. [x] **A block that renders to nothing is not a block.** Front matter is a
       top-level node whose HTML is empty, so counting it put every index off by
       one and `--block 0` exported the front matter — a thing that is not on
       screen and cannot be pointed at.
8. [x] `render::plain` with a tab between cells and a newline between rows: the
       rule for words joins 名称 and 数量 into one word, which is two.

### 3 — the command line

9. [x] `docfusion export <id> --to <fmt> [--block N] [--out FILE]`, `--json` for
       the byte count, `--block N` where N is what the pane shows.
10. [x] Three refusals that are three sentences, because they are three
        mistakes: a block that does not exist, a format that does not suit that
        block, and a format that does not suit a document. A first draft merged
        them and told `--block 9` that it was not a table.

### 4 — the window

11. [x] The status bar's Copy goes; Export (whole document) takes its place.
12. [x] `window_html` wraps every block in `<section class="block"
        data-block="N">`, so the number a block's buttons send back is written by
        the same code that produced the list — verify:
        `the_pane_numbers_the_blocks_the_way_an_export_takes_them`, proven to
        bite by numbering from one and watching it name block 0 as the wrong
        block.
13. [x] Per-block export, revealed by `:hover` **and** `:focus-within` —
        invisible but tabbable is worse than either.
14. [x] **Beside the text, not on it.** The pair started as an overlay at the
        block's top right, which put it on the first line of every paragraph
        and on the last header cell of every table. Each block now gives up a
        strip and `.document` is that much wider, so the measure is unchanged.
15. [x] **And then one row, one glyph.** Moving the pair into the strip was not
        enough: stacked, it is ~50 px tall where a one-line paragraph is ~22,
        so **adjacent blocks' pairs hung into each other's share of the strip**.
        The constraint is one row tall. Two worded buttons in a row cost 117 px
        of strip; one `⋯` costs 48 and opens a dialog with Copy above the
        formats. Confirmed by the `⋯` turning into a `✓` — a tick because the
        button is one glyph wide and a word would widen it into the text.
16. [x] A block copies its `.block-body`, not itself: the block holds the
        buttons, so copying it would paste the word "Copy".
17. [x] The dialog says which block it is about, by its own first words — the
        backdrop covers the document, so a question that named only the format
        would be a question about something nobody can see any more.
18. [x] `export::write` names the file; the block's number goes in the name, so
        two exports of one document are not offered under one name.
19. [x] `:focus-visible` rather than `:focus-within`, on the button rather than
        on the box around it. **A modal `dialog` hands focus back to whatever
        opened it when it closes**, so `:focus-within` was true from the moment
        the menu was used and the `⋯` stayed on screen for good.
20. [x] The export's own confirmation goes. It was a word written onto the
        button that asked, and that button is transparent unless the pointer is
        over the block — which, after a save dialog, it is not.

### 5 — the formats that were missing

21. [x] `Format::Docx`, written from the `comrak` parse into WordprocessingML:
        headings by style, a table with its header row marked, lists with their
        markers written down, code kept as one paragraph. The styles the body
        names are defined in `styles.xml`, or the document opens in Times New
        Roman with an empty navigation pane.
22. [x] `Format::Xlsx`, one sheet per table, and **a cell is a number when the
        number and the text say the same thing** — which keeps `007` and `12.50`
        as text and makes everything else addable.
23. [x] The rule stayed one rule: `a spreadsheet is a table and only a table`,
        which now covers two formats instead of one.
24. [x] `Format::binary`, because a zip cannot be printed; the command line
        refuses `--to docx` without `--out`.
25. [x] No `pdf`, deliberately. A PDF needs a font embedded and a page laid
        out, which is another program; the HTML export opens in a browser and
        prints to PDF from there.
26. [x] **Both writers verified by the readers this program already has** —
        `extract::docx::DocxParser` and `extract::xlsx::XlsxParser` are handed
        what was just written. Proved to bite by dropping one `</w:p>`.
27. [x] Docs: `design.md` §10.1, §11.5; `changelog.md`.

## Verified by hand

By hand against a scratch knowledge base, since the window cannot be tested from
here:

```
$ docfusion export notes --to md --block 0
# 标题

$ docfusion export notes --to csv --block 2
名称,数量
甲方,12
乙方,3

$ docfusion export notes --to txt --block 2
block 2 has no plain-text export: the words with no structure are what
`docfusion show` prints, and what `export` is for is the structure

$ docfusion export notes --to csv --block 9
this document has 3 blocks, numbered from 0; there is no block 9
```

```
$ docfusion export notes --to csv --out /tmp/t.csv && xxd -l 3 /tmp/t.csv
efbbbf                                  # a spreadsheet gets a mark
$ docfusion export notes --to md  --out /tmp/t.md && xxd -l 3 /tmp/t.md
2d 2d 2d                                # .md = "---", and does not
```

## What is not done

- `reindex` still has no window entry point, and an orphaned source copy still
  has no command that reclaims it. Both were noted when the copy was added and
  neither was asked for here.
