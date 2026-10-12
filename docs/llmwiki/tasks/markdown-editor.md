# The document editor, and what an edit means for the record

## Why

The pane reads. Everything in it is the pipeline's output, and the only way to
change a word in a document was to leave the program. That is right for a
scanner's output most of the time and wrong for the case this is actually used
in: a two-hundred-page filing read correctly except for the three cells nobody
could have recognised, where the alternative to typing over them is reading the
whole thing again somewhere else.

## Decisions

Asked and answered:

- **The body, and only the body.** The front matter is where the document says
  where it came from and the report is what the engines disagreed about. Neither
  is anything a person editing the words should be able to damage, and the way
  that is enforced is that neither is ever handed to the editor: `render::parts`
  splits the file in three, `with_body` replaces one of them, and `joined` puts
  them back. An editor cannot damage what it was never given.
- **An edit is recorded, and a re-read warns first.** The file stops being what
  the engines read the moment somebody types in it. That is worth saying in the
  list, and worth asking about before a re-read takes it away.

Two things the design turned on that were checked rather than assumed:

- **A cache hit does not write the document.** `Pipeline::reuse` returns before
  the write, so re-adding a file that has not changed keeps the edit. The
  warning is therefore about the *document* rather than about a particular read:
  it may turn out that nothing was going to be overwritten. Being asked and
  finding out that nothing was lost is a smaller cost than losing an edit.
- **`reindex` is the path that does.** Which is where the first version of this
  was wrong — see below.

## Shape

```
crates/docfusion-core/src/render.rs      parts, Parts, edited, edited_at_of
crates/docfusion-store/src/manifest.rs   schema v5, edited_at
crates/docfusion-store/src/pipeline.rs   save_body
crates/docfusion-gui/src/lib.rs          source, preview, save_source, edited
crates/docfusion-gui/src/state.rs        Record.edited
crates/docfusion-gui/ui/app.js           toggleEditor, split, typed, saveSource
crates/docfusion-gui/ui/index.html       the two questions
crates/docfusion-gui/ui/style.css        .split, .source-editor
```

## Steps

### 1 — the three pieces

1. [x] `render::parts` — front matter, body, report, line by line rather than by
       searching for the heading's text. Three pieces that reassemble to exactly
       the input, which is what a round trip through an editor has to be.
       Verify: a test that joins them back, and one for a document with no body
       at all — where a search for the heading *with a newline in front of it*
       finds nothing and leaves the report inside the words being edited.
2. [x] `body_of` redefined as `parts(..).body.trim_end()`, so the indexing rule
       and the editing rule are one split rather than two that agree today.
3. [x] `edited` writes `edited_at` into the front matter, replacing any note
       that was there: a document edited five times says when the fifth was.
4. [x] A file with no front matter is left alone. Writing one would be inventing
       a provenance for somebody's own notes; the manifest knows either way.

### 2 — the record remembers

5. [x] Schema v5: `edited_at`, with a migration like the last four.
6. [x] `Pipeline::save_body(id, body, at)` — body in, file out, record marked,
       output hash moved with it (`reconcile` recognises an output by that hash,
       and a stale one would make the file look like somebody else's), and the
       index rebuilt from what is now on disk rather than from what the engines
       read.
7. [x] **A test caught a real bug here.** `produce` built its record with
       `..record.clone()`, so `reindex` wrote the engines' words over the edit
       and kept the mark saying somebody had edited it. Now cleared explicitly.
8. [x] Verify: four tests — an edit reaches the body and nothing else, saving
       the same words changes only the note, the record is marked and the index
       follows, and a re-read clears the mark along with the edit it was about.

### 3 — the window

9. [x] `source`, `preview`, `save_source`, `edited` commands. The source is a
       command rather than a field of the view, which is serialised for every
       command the window sends.
10. [x] A toggle in the status bar, off by default, in the bar rather than over
        the document: what is on screen is the document, and furniture over it
        is furniture over the words.
11. [x] The split: a real `.document` on the left, the editor on the right. Each
        half scrolls itself, because two documents as long as each other in one
        scroller is one that cannot be read while the other is scrolled to the
        paragraph being edited.
12. [x] The preview is debounced by 120 ms and rendered by the pane's own
        renderer, so what is typed is shown where it will be read.
13. [x] `Ctrl+S` saves, and the bar says `未保存` while it is true.
14. [x] **No block menus in the preview.** A block's menu exports the file, and
        while the editor is open the file is not what is on screen.
15. [x] Closing with unsaved words asks: keep editing, save, or throw them away.
        The answer is read off whether the words are still unsaved rather than
        off which button was pressed — the two are the same question.
16. [x] The same question before anything that would replace the pane, which is
        `read` — the one funnel both adding a file and opening a record go
        through.
17. [x] The list marks a document somebody has edited, and a re-read asks before
        taking it away.

### 4 — the two halves, which were taken back out

18. [x] **A `textarea`, with the two halves synced by arithmetic.** Wrong, and
        wrong in a way that looked right: `line × lineHeight` is the pixel a line
        begins at only when a line is one row. The two halves drifted further
        apart the longer the document got.
19. [x] **A `textarea`, with a hidden ruler of the same text to measure
        against.** Correct, and the wrong thing to be doing — an element of the
        same text, in a box of the same width and the same font, laid out beside
        the editor so that a question about the editor's own text could be
        answered.
20. [x] **CodeMirror 6, bundled and committed to `ui/vendor/`**, which answers
        it directly with `lineBlockAtHeight` and `lineBlockAt`.
21. [x] **And then all three were taken back out.** The sync, the ruler, the
        highlight, the line numbers threaded through `Rendered` and
        `window_html` — the cost of answering "where am I" was counted against
        what the pane is for, and it lost. What is left is: edit on the right,
        watch the left, save, leave.
22. [x] **A `SyntaxError` took the whole window down**, and the check that was
        supposed to catch it had never been able to: `node --check` passes a file
        with a duplicate top-level declaration in it. It is
        `no_name_is_declared_twice_at_the_top_of_the_script` now, proved to bite.
23. [x] Docs: `design.md` §11.5; `changelog.md`,
        `crates/docfusion-gui/ui/vendor/README.md`.

## What is not done

- **The mark is not in the file for a document that never had front matter.**
  The manifest knows it was edited; the file has nowhere to say so.
- **`reindex` still has no window entry point**, so the warning it exists for is
  reachable only from the command line.
