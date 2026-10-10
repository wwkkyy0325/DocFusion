# P3 Retrieval, and the copy of the source

## Why

Two things, and only one of them was in the plan.

**Search was.** `design.md` §13 puts it in P3: *"SQLite FTS5 + Chinese
tokenization, search UI"*, deliverable *"a knowledge base for humans"*. §2's
non-goal list contradicts it — *"Vector / semantic search. FTS5 only, and only
in P4"* — and the phase table wins: FTS5 is P3 and was always going to be built.
The two lines are reconciled in this change.

**The copy was not.** The knowledge base has been Markdown plus a pointer at a
file somebody else owns, and the design said so out loud. That makes three
things true which are now being traded away:

- **A record whose source moved is a dead pointer.** `reconcile` forgets it, and
  nothing can read it again.
- **`reindex --stale` is impossible.** The whole point of that command is to
  re-read a document after the engine, the model or the prompt changed — and it
  needs the original bytes. Today, moving a file ends its re-readability
  permanently. This is the real reason for the change; search is the other one.
- Provenance: what exactly did this Markdown come from.

## Decisions

Asked and answered:

- **`forget` leaves the copy; `delete` takes it.** Forgetting stays a pure index
  operation, and an unreferenced copy joins untracked Markdown as drift that
  `reconcile` reports and does not delete — a person's own notes are in that
  tree too, and the index claiming nothing about a file is not a reason to
  remove it. Deleting a document is the other sentence, and it means the room
  comes back: leaving the copy would make it a delete that does not free
  anything.
- **Always copy, no switch.** A knowledge base is either self-contained or it is
  not, and two shapes of knowledge base is two things to test and one more
  state for the window to draw. `docfusion delete` is how something stops
  taking up room.
- **Search is a box at the top of the Records tab.** Empty: the list it is now.
  Non-empty: matching documents with the passage that matched.

## Shape

```
knowledge/
  docs/…                       # the Markdown, as now
  .docfusion/
    manifest.db
    sources/
      <blake3>.<ext>           # the bytes that were read, content-addressed
```

Under `.docfusion/` and not beside `docs/` on purpose: a knowledge base whose
`docs/` is synced or committed should not drag the contracts and bank statements
that were read into it along with the Markdown. The id is already the BLAKE3 of
the source, so two folders holding the same file produce one copy and the
aliases table already models that.

**The path is recorded, not derived.** `documents.source_copy`, a new column, so
that "which records have no copy" is a query rather than a walk, and so the
extension cannot be guessed differently in two places.

## Steps

### 1 — the copy

1. [x] Schema v3: `source_copy`, with a migration like the one v2 had.
2. [x] `ingest` writes the copy for both paths — the fresh read and the reuse —
       so re-adding a source whose copy has gone puts it back. A copy that will
       not write is a failed ingest rather than a silent one.
3. [x] `reconcile` reports the copies nothing points at and deletes none of
       them, and `--fix` gives a copy back to every record whose source is still
       findable. Run by hand over a copy of the real knowledge base: 13 records
       with no copy, 13 copies taken, clean on the second run.
4. [x] The two ways out: `forget` leaves the copy, `delete` takes it — verify:
       a test for each, and the same two tested from the dialog's side.
5. [x] `keep_source_of` reads and hashes every candidate rather than trusting
       the path — verify: a test where the file the record names has been
       edited, which must be refused rather than copied in.

### 2 — the index

5. [x] An FTS5 table over the Markdown body. **Not `trigram`, which was the
       plan and is wrong**: it matches nothing shorter than three characters, so
       on a test corpus 合同, 供销, 学生 and 报到 all returned zero while
       产品供销合同 and 24年 returned one. Most Chinese words are two
       characters. `unicode61` with the CJK pulled apart into single characters
       before it is stored, and each query term asked for as a phrase, finds all
       of them and is smaller than trigram besides.
6. [x] Kept in step on write and on `forget`; rebuildable from the tree with
       `docfusion index`. The index is keyed by the document's own row, so
       writing one is two lookups rather than a scan of the content table —
       which on the write path is every ingest.
7. [x] `docfusion search <words...>`, ranked by bm25 with the title weighted
       eight to one, with the matching passage.
8. [x] Both front ends say when the index is behind, before answering rather
       than after, because an index that is behind is *the* reason a search
       finds nothing while the archive plainly holds it.

### 3 — the window

9. [x] A search box on the Records tab; empty is the list as it is now.
10. [x] Results name the document and show the passage, with the match marked by
        an element built from the text rather than by any markup in it.
11. [x] The deletes and the search box interacting — deleting a document whose
        text is in the index takes it out of the index too, which is tested, and
        the box re-reads the list when it is emptied.

### 4 — the payoff

12. [x] `docfusion reindex [document]`. What the copy was for. Which documents
        are out of date is answered by comparing the stamp the record holds with
        what the installed engines would say about the same bytes — read from
        the **copy**, because a re-read that skipped exactly the documents whose
        sources had moved would skip the ones that need it most. Only the first
        few hundred bytes of each file are read to ask, since the question is
        about routing and a scan that read every file would cost the archive's
        own size in I/O to answer it.
13. [x] Docs: `design.md` §2, §9.1, §10.1, §13; `changelog.md`.

## Verified by hand

```
$ docfusion --root kb reindex
every document was read by the engines installed now
$ mv kb/src/合同.txt somewhere-else            # the source goes away
$ docfusion --root kb reindex 合同.txt
read        …/kb/docs/合同.md                  # read from the kept copy
$ docfusion --root kb search 合同
98840feb  合同.txt                             # and still findable
```

## What was measured, and what it changed

**The tokenizer, before writing any of it.** A twenty-line probe against a
temporary FTS5 table, because the choice is invisible until somebody searches
for a two-character Chinese word and finds nothing:

| query | trigram | per-character |
|---|---|---|
| 产品供销合同 | 1 | 1 |
| 合同 | **0** | 1 |
| 供销 | **0** | 1 |
| 学生 | **0** | 1 |
| 报到 | **0** | 1 |
| openEuler | 1 | 1 |
| 甲方 学生 (one in each of two documents) | **0** | **0** |

The last row is the one that says the query means what it should: two terms are
an `AND`, and a term of Chinese characters is a phrase, so neither finds a
document that holds only half of it.
