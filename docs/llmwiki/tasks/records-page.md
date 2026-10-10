# The records page learns to be used

## Why

A row in the records list was one big button. Clicking anywhere on it opened the
document, which made reading a list of what had been read into a way of
accidentally leaving it — and the two things somebody actually wants from a row,
*look at it again* and *get rid of it*, had no target at all. Deleting a document
was not possible from anywhere: the only way out was to delete the Markdown by
hand and let `reconcile` notice.

Verse's history list solves the same two problems, and its answer is the one
taken here: a row shows what the thing is, and **two acts, kept apart by what
they destroy**. "Remove this row" and "delete this file" are two different
sentences, and one button that guessed between them would be a button that
sometimes deleted a document.

## What this app has that Verse does not

Verse's row is a pointer into a session roster plus a `history.json`. Here the
list **is** the index of the knowledge base, and the document is a Markdown file
under `docs/`. So the two acts land like this:

| | Removes | Leaves |
|---|---|---|
| Remove from the list | the index row | the Markdown, at `docs/<name>.md` |
| Delete the file | the index row **and** the Markdown | nothing |

Neither touches the document the user chose. The PDF or the picture was never
copied into the knowledge base, so there is nothing there to delete — which is
the sentence the question says out loud, because it is the thing somebody
pressing a red button wants to know.

Both are consistent with what `reconcile` already believes: a record whose file
has gone is drift it forgets, and a file whose record has gone is untracked
drift it reports and does not put back.

## Steps

1. [x] Two commands, one per act — verify: `forget` and `delete` are exercised
       against a temporary knowledge base, and the source document is asserted
       to survive both.
2. [x] The delete refuses a path that leaves the knowledge base — verify: a test
       that relinks a record to `../outside.md` and checks the file outside is
       still there. `starts_with(root)` does not catch it: it walks components
       and `..` is one.
3. [x] A row is name, when it was read, and two buttons — verify: the markup
       test finds every element the script looks up, and the sentences test
       finds every string in both languages.
4. [x] One question, asked before either act, as a modal `dialog` — verify: read
       by hand, since a modal cannot be driven from a test here. The elements,
       the strings and the disabled state are covered.
5. [x] A record knows whether its Markdown is still on disk, so the question can
       say "there is nothing to delete" rather than offering a button that fails
       — verify: `Record::on_disk` tested against a deleted file.
6. [x] Removing the record being shown empties the document pane — verify: a
       test over the view. **The first version of this was wrong in a way the
       tests could not see** and is written down in the changelog.
7. [x] `docfusion forget` and `docfusion delete`, because of this project's own
       rule that the window must not be the only place a capability lives — the
       operations are `Pipeline::forget` and `Pipeline::delete`, and both front
       ends call them. `delete` obeys `--dry-run`, since it is the one command
       here that destroys anything. — verify: run by hand against a scratch
       knowledge base; every path below.
8. [x] Docs: `design.md`, `changelog.md`.

## Verified by hand

```
$ docfusion --root kb add kb/src/one.txt
$ docfusion --root kb forget one.txt
forgot …/kb/src/one.txt  (still at docs\one.md)     # the file is still there
$ docfusion --root kb --dry-run delete two.txt
would delete docs\two.md                            # and nothing was
$ docfusion --root kb delete two.txt
deleted docs\two.md  (…/two.txt itself is untouched)
$ docfusion --root kb forget nope.txt
error: no document in the knowledge base matches nope.txt
$ docfusion --root kb --dry-run delete 1f5db128
would delete docs\three.md                          # eight characters is enough
```

## Not done, and worth saying

- **No undo, and no trash.** Deleting is `remove_file`. Verse does the same and
  says "cannot be undone" in the question rather than pretending otherwise.
- **The question is not under test.** A modal cannot be driven from the tests
  this project has, so what the dialog *says* is covered by the strings and
  markup tests and what it *looks like* was looked at.
