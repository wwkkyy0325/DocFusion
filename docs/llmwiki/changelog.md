# Changelog

Append-only meaningful changes. Newest entries at the top.

---

## 2026-10-10 — One local set, and a card that shows it

**What:** The local models became one set of six files and one download of
**138 MB**, defined once in `docfusion_store::models`, and the settings tab
grew a card that says what the set is, what it does, what it weighs and where
the download has got to. `layout.rs` became `models.rs` because it stopped
being about the layout model the moment it held the recogniser's weights too.

**What was given up, deliberately.** The four recognition weights — 6.9 MB —
used to be fetched by `build.rs` and compiled into the binary, which made "a
scan is readable with no setup, offline, from install" true in the strong
sense. They are now downloads like everything else, so a fresh installation
reads no image and no scanned PDF until the set has arrived, and the command
line is no different from the window. Put to the project's owner with the cost
written out, and taken: one set to maintain beats a small always-there half.

**What it bought.** Three places to change when a model moves became one; two
answers to "where do models live" became one (`DOCFUSION_MODEL_CACHE` and the
per-user directory disagreed by a `models/` segment and nobody had noticed,
because with the variable unset they were the same only for one of the three
files); a `build.rs` that needed the network to compile stopped existing; and
the settings tab stopped being able to describe only the half of its subject
that happened to be downloadable.

**The card, and why it is one card.** A row per file would be six things to add
up before answering the only question somebody brings to it. Progress is
reported for the **whole set** rather than per file — six files each running
from zero to a hundred would look like six downloads and make the bar fall back
five times — and the first frame is filled to what is already on disk, because
the fetch resumes. The shape is Verse's, whose downloader `models.rs` already
borrowed; the one thing not copied is per-file progress, for that reason.

**A missing model is a download, not a bug.** `RapidOcrBackend::build` refuses
with the names of the files that are not there, and that now travels as a type
— `ExtractError::Unavailable` — rather than as a sentence, so the window draws
it as `Cause::Download` with a sentence pointing at the settings tab. The
vision backend raises `BackendError::Unavailable` too and means something else
by it, so the distinction is drawn at the call site that knows which backend it
is holding; the first version collapsed the two and every test in `scan.rs`
still passed, which is why there is now one that does not.

**Found while doing it:** `Cause::Download` had never reached the frontend's
strings at all, so a failed download was drawn as "Something went wrong."
There are three tests for that class of gap now, because the compiler cannot see
across the boundary into the frontend and a person did not: an exhaustive match
over `Cause`, a check that every sentence asked for is written in both
languages, and a check that every element looked up is in the markup. Each was
proved to bite by breaking the thing it watches, and the first version of the
second one was **vacuous** — it only counted `t().x`, and most sentences here
are named through `words.x` in the table that fills the markup, which is
exactly where a rename lands.

**Measured:** 504 tests pass, `cargo clippy --workspace --all-targets` is
clean. The fetch path was exercised by hand against the real home: with the
dictionary deleted, `doctor` reported one file of the set missing and `--fetch`
put it back in the subdirectory the pipeline looks in; with an empty home it
reported the whole set, 138 MB, `capability text-only`. Not downloaded, and
correctly no longer claimed: `ppocrv6-small` and `ppocrv6-medium`, tried and
rejected earlier, were cleared out of the user's model directory.

---

## 2026-10-10 — A space in a Chinese line that is not there

**What:** Joining two lines put a space between them, and a break in a Chinese
line is where the column ran out. Measured: headings **27 of 64 to 32 of 64**,
mean loss 0.298 to **0.292**. 082 goes from 3 of 10 to 7 of 10.

**Why it was worth a number of its own:** on that page, five of ten headings
were missed for this and nothing else — the words were right and the space
between them was not, and no amount of reading better would have found them.
`洛阳市抗旱从种子抓起` was being written `洛阳市抗旱 从种子抓起`.

**The rule was already in the project.** A PDF hands its text over in pieces
and the pieces need the same decision, so `needs_space` and `is_cjk` existed —
privately, in the PDF reader. They are now in `text` and used by both. A space
belongs between two Latin words and not between two Chinese ones, and this was
the second time the question came up; the answer is kept rather than decided
again.

**And the same fault in prose, which the corpus cannot see.** A paragraph keeps
the line breaks the page had, and Markdown turns a newline inside a paragraph
into a space when it is rendered — so a Chinese paragraph reaches the window
with a space at every place the column ran out. Rendered and looked at rather
than reasoned about: the HTML is one `<p>` with the newline still in it. A
break between two Latin words is left alone, because there it *is* a space. The
corpus does not move at all — 0.292, 32 of 64, 10 of 10 — because the scorer
builds its text from text nodes and a soft break is not one. This one is here
because the words in the window were wrong, not because a number moved.

---

## 2026-10-10 — One heading region, one heading or two

**What:** A layout region holding two lines of text is split into two headings
when its lines are **not** the same size. Measured: headings 25 of 64 to **27 of
64**, mean loss 0.300 to **0.298**, and every case but one unchanged.

**Why:** both a wrapped heading and a newspaper kicker arrive as one region with
two lines in it, and the references disagree about them — 071 writes
`SECURITIES AND EXCHANGE` over `COMMISSION` as one heading, 081 writes a kicker
over its headline as two. Joining both cost 081 a heading to gain 071 four.

**The sizes are the whole of the difference, and they are only where the lines
are.** Measured across the corpus, a wrapped heading's lines differ by between
1.00 and 1.26 and a kicker's by 2.31 and 2.42 — nothing in between, so 1.5 is a
half rather than a number tuned until the cases passed. Each part keeps its own
place on the page, so the page is still read in order, and only a heading region
is ever split: a paragraph with a larger first line is a paragraph.

---

## 2026-10-10 — A heading is written on one line

**What:** A heading whose words ran onto a second line on the page was written
with the break left in, and Markdown ends a heading at the first newline — so a
two-line heading came out as a heading followed by a paragraph, and its second
half matched nothing. Measured: headings 21 of 64 to **25 of 64**, mean loss
0.313 to **0.300**.

**Why it looked like a reading fault and was not:** the front matter's title
held both lines the whole time — `SECURITIES AND EXCHANGE
COMMISSION` — which is
how it was found. The block was right and the writing of it was wrong.

**The trade it makes:** a newspaper page has a kicker above a headline, the
layout model puts both in one region, and that reference calls them two headings
where the other one calls its two lines one. Joining gains 071 four and costs
081 one. The two cases differ in **font size** — a wrapped heading's lines are
the same size and a kicker's are not — and that is the one thing the renderer
cannot see. Acting on it needs the block built where the lines are.

---

## 2026-10-10 — Reading a table from its rules, built and not taken

**What:** Nothing shipped. A table was read from the lines a form is drawn with,
with a merged cell being a position that has no line through it. It is exact
where the model is not, and it is worse where the model is fine, and it is out
of the tree.

**Why it was tried:** a question about merged cells. The table model has a token
for a span and measured, it gets them wrong: `<td> rowspan="2" <td >` for a cell
it never closes, a span of two for a cell three rows tall, and no span at all
for a header spanning two columns. A rule has no such trouble — the absence of a
line **is** the merge, stated by the page.

**Measured.** On two generated tables, one with a row span and one with a column
span, the rules read both exactly where the model read neither. On the corpus,
where several tables are ruled:

| | model | rules first |
|---|---|---|
| tables | **10/10** | 7/10 |
| similarity | **0.878** | 0.828 |

**Why it lost.** A table with a coloured header band — a slide exported to a
table, which the corpus has — puts a block of dark pixels across its top, and
that block swallows the rules around it: the top two rows came back as one cell
each, two columns wide. Excluding colour from "ink" so the band is not a line
made it worse rather than better, because the band's own edges are then not
lines either. A skew of a pixel or two was a second source of false merges, and
looking within a pixel or two of where each line was found fixed that one.

**Why it is not worth tuning further now.** There is nothing to tune against. No
corpus case has a merged cell, so the only ground truth for the case this exists
for is a table drawn by hand — and a detector fitted to two hand-made pictures
is a detector fitted to two hand-made pictures. The idea is right and the
detector is not, and the way to try again is a corpus with ruled tables that
have merges in them.

---

## 2026-10-10 — A heading is compared by its words, not its depth

**What:** The scorer no longer compares the depth of a heading. Measured on the
corpus: headings **8 of 64 → 21 of 64**, mean loss **0.413 → 0.313**.

**Why:** every one of the corpus's sixty-four reference headings is level one.
Not one uses `##`. OmniDocBench's ground truth records that a block **is** a
title and not how deep a title it is, and the conversion to Markdown had to
write a number — so a section heading on a page comes out as `#`, and the engine
calling a section a section and marking it `##` was scored as a miss. A
reference that cannot express a depth cannot measure one, and comparing them was
measuring the converter.

The words still have to be the words: a heading whose text differs is still a
miss, and there is a test for it. This is the same shape of decision as the
table row Markdown had split in two — a rule about how to read a reference,
made in the reader and not by editing the corpus.

**What it exposes:** the remaining forty-three are the engine's. They are
concentrated: a three-column SEC notice (071) with seven headings and none
found, two Chinese books (041, 042) with eight between them, and the two
newspapers (081, 082) with twenty-three. Looked at rather than guessed at, 071's
headings are each **two lines** —

```
SECURITIES AND EXCHANGE
COMMISSION
```

— and the layout model marks the first line a heading and the second body text,
so the engine returns `SECURITIES AND EXCHANGE` and matches nothing. That is
the next thing to fix, and it is in the engine rather than the measurement.

---

## 2026-10-10 — Merged cells: one bug of ours, and a ceiling that is the model's

**What:** A cell spanning rows was read as a cell spanning columns, which put
every column after it in the wrong place. Fixed, with the rows under a tall cell
now holding its columns. Measured on a generated equipment register with one
merged label, rows one and two come out exactly right. What is still wrong on it
is the model's prediction, not the reading of it.

**Why it came up:** a question about whether a table with merged cells can be
brought across faithfully. Three separate answers, and only the first is about
this code.

- **Markdown cannot express a merged cell.** GFM has no colspan and no rowspan.
  So the cells are laid out in a full grid, the positions inside a merged cell
  are left empty, and the alternative — repeating the words down the rows — is
  text the page does not have. The two honest options are empty positions or
  emitting HTML for those tables; HTML was not taken because it makes the output
  two different formats in one file.

- **The span bug was ours.** The model has `colspan="n"` and `rowspan="n"`, both
  of the form `name="digits"`, and `span()` read the digits without the name:
  both set the column count. `Cell` now carries `down` beside `across`.
  `seatings` lays the grid out before a word is placed, because a row that does
  not know a cell above still holds its column starts its own cells three places
  too early.

- **The rest is the model, and it is measured rather than assumed.** On the
  register it emits `<td> rowspan="2" <td >` for the tall cell: one it never
  closes, and a span of two for a cell that covers three rows. On a table with a
  header spanning two columns it emitted five plain cells and no span at all.
  Both are the prediction being wrong, and no reader can recover from them. The
  one part that was ours to fix is that a never-closed cell was dropped
  entirely; it is now finished at the next `<td`, keeping the box from the token
  that opened it — measured, that token carries the merged label's box and the
  `name="digits"` that follows carries something else's.

**What this suggests for real forms:** the documents people actually ask about —
bank schedules, equipment registers, tax forms — are fully ruled, and a ruled
form says where its cells are with its own lines. Building the grid from the
rules is the classic answer, it needs no model, and a merged cell is simply a
position with no rule through it. That is where the accuracy for irregular
tables is, not in a better parse of a sequence model's tokens.

**Unchanged:** the corpus, at 0.413 loss and 10 of 10 tables. None of its tables
has a span, which is why none of this was visible there.

---

## 2026-10-10 — Tables in a PDF that has a text layer

**What:** A PDF whose pages carry a text layer now comes out with its tables as
tables. Before this it could not: the table model runs inside the recognition
backend, and a PDF with text never reaches it, so the cells of every table were
pressed into paragraphs. Reproduced on a generated printed table, five columns,
which came out as `Region  Q1  Q2  Q3  Q4` and four lines of numbers.

**Why it was not noticed:** the corpus is twenty-four images and not one PDF, so
the numbers never saw it. This is a capability the measurement cannot see, and
it was found by a person opening a document rather than by a bench run.

**Key decisions:**

- **The words are the file's, and only the boxes are the model's.** A PDF that
  carries a text layer says exactly what is on the page; running a recogniser
  over it would be inventing a worse copy of what the file already holds. So the
  page is rendered, the detector finds the tables, the table model says where
  the cells are, and the text goes into them from the file.
- **One placement rule, not two.** Where a word goes in a cell is one question
  with one answer, and answering it twice is how the two answers come to differ.
  The new reader lives beside the recognition one and calls the same `laid_out`
  and the same cell-filling: it hands the geometry the file's own lines in the
  shape it expects, and everything below that is untouched.
- **Per run, not per line, and that is the whole of what makes it work.** A PDF
  reports a table row as one line running its whole width. Handed that as a
  single box, the placement rule puts the entire row in the cell holding most of
  it — which is the first, since a row of five cells gives every cell a fifth.
  Measured exactly that way before it was fixed. A *run* is a stretch of text the
  file draws together and is the finest thing that can go in a cell.
- **Rendering happens outside PDFium's lock.** `PdfTextLayer` holds the process
  wide PDFium lock while it reads text, and rendering takes the same one: the
  first version deadlocked, on the first document, for the full ten minutes it
  was given. Reading is one pass under the lock, and everything that has to
  render happens after it.
- **The page's name and the extractor's name are different names.** Making the
  id `pdfium+tables` when a model is present broke `--engine pdfium` and made the
  page unresolvable — `stamp_of` looks a page's engine up by matching the
  extractor's id, and a page filed under a name no extractor answers to is a
  document that cannot be written at all. The recognition path already had this
  right: `scan` is the id and `rapidocr+layout+tables` is what the page says. The
  composition goes in `provenance.engines` and in the stamp, which is the cache
  key; the id stays what a person can name.

**Not done:** headings on a text-layer page still come from font size alone, and
the reading order of a PDF is still the file's own drawing order rather than the
column-aware order the recognition path works out. Both are visible next to each
other now that a PDF page can carry a table block.

---

## 2026-10-10 — A bigger recogniser, measured and not taken

**What:** Nothing shipped. The two larger model sets in the family the embedded
recogniser comes from were measured on the corpus, and neither earns its
download.

**Why it was worth asking:** a photographed bank schedule came back with every
cell in the right place and three characters wrong — `63,00.00` for
`63,000.00`, and `合计` missing from the total row. Structure solved, words not,
and the assumption was that the 6.9 MB recogniser embedded for the no-setup
promise was the thing to blame. `rapidocr-core` already knows both larger sets,
so it was a one-line change and a bench run.

**Measured:**

| | tiny (embedded, 6.9 MB) | small (32 MB) | medium (139 MB) |
|---|---|---|---|
| mean loss | **0.413** | 0.412 | 0.418 |
| content error | 0.187 | **0.185** | 0.190 |
| reading order | **0.324** | 0.325 | 0.331 |
| headings | 8/64 | **9/64** | 8/64 |
| tables alike | 0.878 | 0.890 | **0.900** |
| corpus time | **552 s** | 665 s | 1224 s |

Small is nine cases better and four worse — a wash with a 32 MB price. Medium is
worse than the embedded model on loss, content and order, at twice the time.

**The result worth keeping:** `small` fixes the cell it was reached for and
breaks another on the same page. `63,00.00` becomes `63,000.00`, and the header
`固定资产 | 金额` becomes `国产 | 空`. The larger *detection* model moves the
line boxes, and the line boxes are what the layout model and the table model
both read. A recogniser is not a drop-in: it is one of three models in a chain
and the one in the middle does not get to change alone.

**What it says about where the error is:** the recogniser is not the ceiling.
Two larger models moved `content` by 0.002 and left `headings` at 8 of 64. The
remaining loss is structural — headings found and reading order — and that is
where the next work is, not here.

---

## 2026-10-10 — Two references that were not Markdown tables

**What:** The scorer now joins a table row that was written across several
lines back into one. Measured on the corpus: mean loss 0.425 → **0.413**, tables
**8 of 9 → 10 of 10**, similarity 0.861 → **0.878**.

**Why:** Markdown has no way to put a line break inside a table cell, so a table
whose cells held one is written out with the break left in — and the row, and
the table with it, end at the first of those lines. OmniDocBench keeps the
`<br>` the original table had, and two of the corpus's references arrived that
way: 061, whose fourth row spills onto a fifth line, and 111, whose
eleven-column header is spread over eleven.

Read strictly, 061's reference says its table has one row and is followed by a
paragraph. The engine returned the six rows that are actually on the page and
was scored against a table that never existed. **061 was written down as the
worst table in the set at 0.39 and it is not a bad table at all** — repaired, it
is found, and 111 turns out to hold two tables rather than one.

**Key decisions:**

- **The repair is in the reader, not in the corpus.** The corpus is data outside
  this repository and may be regenerated from OmniDocBench, at which point a
  hand-edit is lost; and a corpus is the instrument, so editing it to move a
  score is the one thing not to do quietly. A rule that says how to read a
  reference is versioned with the code that reads it.
- **Only a row that is not closed is joined.** A row ending in `|` is complete
  and has nothing to continue it, so a looser rule would swallow the horizontal
  rule or the paragraph below a table. An unclosed row is what a break inside a
  cell produces and the only thing that does.
- **Applied to both sides.** The renderer writes one line per row, so for the
  output it does nothing; applying it anyway is what makes it a rule about
  Markdown rather than a thumb on the scale. The evidence that it is not one:
  **111's similarity goes down**, 0.94 to 0.93, when its reference stops lying
  about its shape.

---

## 2026-10-10 — A table taller than it is wide

**What:** Fixed the table model's preprocessing writing a resized picture into
its square with *the picture's* stride instead of the square's. The fix is one
line of index arithmetic. Measured on the corpus it takes tables from **7 of 9
to 8 of 9** and their similarity from 0.840 to 0.861.

**Why it hid for so long:** the two strides are equal exactly while the
picture is wider than it is tall. `squashed` resizes the longest side to 488, so
a landscape crop gets `across == 488` and the two agree; a portrait crop gets
`across < 488` and the picture is folded into the square every 488 pixels. Every
table in the corpus was landscape. A bank schedule photographed in portrait,
1196×1746, came back as twenty-eight rows of text jumbled into four columns —
and the model's *structure* was perfect the whole time, 28 rows by 5 columns,
which is what made it look like a reading problem rather than a geometry one.

**The tell:** the row count is right and the column count is not. A model that
cannot see the table gets both wrong; a model given a sheared picture can still
find the rulings between rows, which survive any vertical smear, and loses the
columns, which do not.

**How it was found:** by having a second implementation to disagree with. A
Python chain that reproduces the pipeline (`target/table-probe/chain.py`,
written earlier to choose between structure models) got the table exactly right
on the same picture with the same weights, which narrowed the search to what the
two do differently — six lines, one of them the write. Two implementations of
one pipeline disagreeing is a much better signal than one implementation looking
wrong, and the Python half is worth keeping for that alone.

**Also:** the regression test asserts the row it is about lands on the square's
second row and that the place the picture's stride would have used is still the
zero padding. Confirmed to fail against the old code, which is the only thing
that makes the test worth having.

---

## 2026-10-10 — The models the window could not fetch

**What:** A run through the window produced Markdown with no table in it. Nothing
was wrong with the pipeline: the two models the local engine reads a page with
had never been downloaded on this machine. The window now says which of them are
here and fetches the missing ones itself, with progress and a stop.

**Why:** Every measurement in the table work was taken under
`DOCFUSION_HOME=target/layout-home`, a home with the models in it. The window
runs under the real home, which had none. Both sets of numbers were true about
different installations, and the front matter's `engines: [rapidocr]` was the
only sign, in a place nobody reads. The one way to fetch them was
`docfusion doctor --fetch`, which means the case that needs the models most — a
person opening a document in a window — was the case that could not get them.

**Key decisions:**

- **The window fetches them, rather than telling people to use the command
  line.** A desktop application that asks its user to open a terminal has not
  finished the job, and "the layout model" is not a thing to send somebody to a
  terminal for.
- **The availability check is now forgettable, and that is the load-bearing
  part.** It was a `OnceLock`, taken once for the process, on the reasoning that
  nothing changes while a program runs. Fetching from the window breaks that
  premise, and a button that appeared to work but changed nothing until the next
  launch is worse than no button. `layout::Cached` remembers the answer and
  `forget()` throws it away; the next document read gets the new name. Between
  documents is early enough, because what the answer decides is the name a page
  is filed under and that has to be stable within one.
- **Progress, and a stop.** 124 MB is minutes. A line that only changes at the
  end is a line that cannot be told apart from a download that has stopped, and
  a fetch that cannot be stopped is one somebody cannot call back on a metered
  connection. Stopping keeps what arrived; the fetch resumes from there.
- **The models are not part of the endpoint switch.** The switch above them is
  about a model at the other end of a network; these are files on this disk with
  no address and no key, and they are what decides whether a table comes out as
  a table. Greying them out with the endpoint would have been a settings page
  saying tables are off when they are not.

**Also corrected:** the conflict note told the reader the engine inside "does not
know what a table is, so its readings are always text and nothing else". That
was true when it was written and false the moment the table model was wired in.
It now says what the engine does and what a plain-text reading means.

**Also removed:** `layout::ensure_table`, which no caller had ever used — the
doctor fetches both models through `ensure_of` directly.

**Measured:** case 062 without the models, 11×11 table, read as a hundred loose
lines; with them, `engines: [rapidocr+layout+tables]` and the table. Bench totals
unchanged at 0.428 loss, 0.332 order, 7 of 9 tables alike 0.840 — the caching
change alters when the answer is taken, not what it is.

---

## 2026-10-10 — A table is measured by what is in it now

**What:** TEDS, in `docfusion_core::bench`, reported per case and as a mean.
The count beside it — *was a table of the right shape returned* — is unchanged,
because the two answer different questions and a table can pass one and fail
the other.

```
061  tables 0/1  alike 0.39      101  tables 1/1  alike 1.00
062  tables 1/1  alike 0.98      102  tables 1/1  alike 1.00
082  tables 2/3  alike 0.64      111  tables 1/1  alike 0.94
                                 112  tables 1/1  alike 0.94
                       7 of 9 found, alike 0.840
```

**A shape is not a table.** Two tables of fifteen rows by three columns with
not one word in common share a shape, and the count has always called both
found. One of them is right and the other is a page of wrong numbers under the
right headings, which is the failure that reads as success.

**The first mean this reported was 0.953 and it was wrong.** Seventeen of the
corpus's twenty-four documents have no table at all, and a document with no
tables scores one — there is nothing to be wrong about. Averaging those in
against the seven that do reported 0.953 for a set whose table cases average
**0.840**. A vacuous perfect score is still a score, and it has to be kept out
of an average by hand.

### Three bugs, all found by the tests, none of them in the model

**The traversal only walked the first child.** A node's leftmost leaf comes from
its first child, and that is what the code took — so every node not on the
leftmost path was left holding a zero, which the distance below then subtracted
from.

**The keyroots were collected in the wrong order.** A subtree's distance is read
out of the table while a larger one containing it is being computed, so the
smaller has to have been done first. Collected in the order the leftmost leaves
happened to run in, the root came out before its own rows and every distance was
measured against an empty table: **two documents with no table in common scored
0.98.**

**And two of the test expectations were wrong rather than the code.** A table
whose rows are deleted takes its cells with it, so three edits and not two; and
two tables of the same shape are not zero apart when the structure is identical
and only the words differ — that is what TEDS means, and the test was rewritten
to say so, side by side with the count that calls it a match.

---

## 2026-10-10 — Tables are read by a model now, and 1 of 9 became 7 of 9

**What:** `SLANet_plus`, 7.4 MB, downloaded by the same command as the layout
model and run in the same process, reads the grid of a table — the rows, the
columns, and a box for every cell. The recognised words go into the cells by
where the boxes are.

| | before | after |
|---|---|---|
| **tables found** | **1 / 9** | **7 / 9** |
| mean loss | 0.481 | **0.428** |
| error, order ignored | 0.197 | **0.194** |
| error, order included | 0.360 | **0.332** |
| 24 pages | 65 s | 64 s |

`061`, `062`, `101`, `102`, `111` and two of `082`'s three went from no table to
one. `062` is an eleven-column financial statement that the geometry read as a
column of loose words with the values against the wrong headings.

**This was never a search problem, it was a model problem.** The project spent
its geometry on inferring a table from whitespace — merge the x-spans, find the
gutters, cluster the rows — and the industry does not infer one, it predicts
one. PP-Structure is detect the lines, read them, and predict the structure;
**this project already had the first two**, because that is what `rapidocr` is.
One model was missing and it is smaller than the weights already in the binary.

**Chosen by measurement, over three candidates and one that could not be
tried.** `SLANet_plus` 317/574 cells, `SLANeXt_wired` 45/588 and
`SLANeXt_wireless` 37/588 — and the second two are not disproven, only
unreachable: their cell boxes do not decode, and their structure heads are fine,
so comparing them fairly needs an adapter nobody wrote.

### Three things only running them said

**Same family, same tensor signature, different box space.** `x[1,3,S,S]` in, a
location output and a fifty-class structure output out, both. And `SLANet_plus`
answers in the padded square while `SLANeXt` answers in the resized picture
inside it — a table 1470 by 479 comes back in the top 0.326 of one and spread
across 0 to 0.98 of the other. Swapping a model is not swapping a model.

**The dictionary PaddleOCR ships does not match the model's vocabulary.** Fifty
classes, forty-eight entries in the file, low ids right and high ids wrong:
decoding straight through turns every plain cell into ` rowspan="20"`. The
merged cell is *appended* by PaddleOCR rather than stored, and the file's last
entry arrived after this model was trained.

**An hour was spent on the harness, twice, before the model was.** A document
with two tables was flattened into one grid; then two lists of tables were
paired by position when the detectors returned them in different orders, which
scored two correct tables as nothing. 42.8%, then 33.4%, then 55.2%.

### Two things it took a second attempt to get right in the engine

**Losing a line is worse than losing a table.** The first version put a line in
a cell only when the line's centre fell inside the cell's box. On `061` that
dropped most of the table's words and the content error went from 0.084 to
**0.614** — the page scored worse than it had as loose prose. Every line now
goes to the cell holding most of it and, failing that, to the nearest.

**The empty rows are load-bearing.** The model puts its header in a row of its
own and leaves it empty, so a table comes out with a blank line every other row.
Dropping them looks like an obvious tidy and **took the tables from seven of
nine back to one** — a row with no words in it is still the row the headings are
above. Kept, with the note, rather than re-learned.

### The box mapping was wrong, and the first reading of it was wronger

**The boxes are normalised to the padded square and getting back to the picture
is two steps, not one.** A table 1470 by 479 is resized to 488 by 159 and padded
to 488 by 488. A box at 0.25 of the square is the square's 122nd row, and the
picture is only the first 159 of its 488 — so it is three quarters of the way
down the *picture*, not a quarter.

The first version multiplied by the square's side and then by the picture's own
height, which is right only when the two happen to be equal. A wide table's
fifteen rows all came out as ten-pixel stripes in the top twelfth of the page,
and their words stacked into rows that were not theirs. **The headers of an
eleven-column table were split across two rows of the output, and that was read
as the model predicting a `rowspan` — it was not.** The grid was fifteen rows by
eleven columns with no spans at all, which is exactly what the reference has.
The lesson is the same one the geometry has taught twice: the number that looks
like a model failure deserves a look at the boxes first.

Fixing it took mean loss 0.450 to 0.428 and the reading-order error to **0.332,
better than the 0.360 of no table model at all** — the tables are now in the
reading order they belong in and not scrambling the page around them.

### Not right yet

**The character metric is the wrong instrument for a table.** It is measured
over the whole page, so a table with every cell correct and its rows split
differently scores worse than the same words as prose. `docfusion bench` needs a
TEDS before table quality can be read off it.

---

## 2026-10-10 — A switch for the model, so that stopping it stops nothing else

**What:** a switch in the settings tab that turns the vision model off without
losing the endpoint, the model name or the key. Off, the window reads with the
engine inside the app and calls nothing.

**The only way to stop the model was to throw the settings away.** Emptying the
endpoint was documented as "how the model is switched off", and it worked —
`endpoint = ""` is not a URL and nothing was called. It also deleted the address
and the key, so a person who wanted to read a private document offline had to
write their endpoint down somewhere first and type it back afterwards. A switch
is the shape of that requirement. The deletion still works and still deletes;
it is no longer the only thing on offer.

**Absent means on.** The field is an `Option`, and a file written before the
switch existed has no opinion — which must read as "the model is used", because
that is what it did. Getting this the other way round would have silently
switched off every installation that upgraded.

**The switch gates the file and not the flags.** `--endpoint` on a command line
is as explicit as anybody can be, and a setting turned off in a window must not
stop a run that names one. So the merge in `Cli::with_config` skips the file's
endpoint, model and key when the switch is off, and does nothing to a flag. Both
directions are tested.

**Disabled rather than hidden**, and only the fields the switch is about. What
is switched off can still be read — a half-typed address is still somebody's —
and `#save` stays live, because saving the switch is the thing they are trying
to do. Covered by two new headless screenshots, `settings-zh` and `off-zh`.

**Also:** the settings page now says what belongs in the endpoint field. It
accepts any OpenAI-compatible server — vLLM, llama.cpp, Ollama, or a hosted one
— and said so nowhere.

---

## 2026-10-10 — A page is read a column at a time

**What:** reading order stops being top-to-bottom-then-left-to-right and becomes
column-aware. The page is cut into vertical corridors first and into bands
second; anything spanning the width separates the bands, and between two of them
the columns are read one after another.

**It is the largest single improvement the corpus has recorded**, and it went
where the numbers said it would — the pages that read well and scored badly,
where the characters were all present and the sequence was wrong.

| | `rapidocr` | `+layout`, before | `+layout`, now |
|---|---|---|---|
| error, reading order included | 0.466 | 0.419 | **0.360** |
| error, order ignored | 0.203 | 0.197 | 0.197 |
| mean loss | 0.566 | 0.511 | **0.481** |
| headings, tables | 0/64, 0/9 | 8/64, 1/9 | 8/64, 1/9 |

| case | before | after |
|---|---|---|
| 021 academic, two column | 0.523 | **0.145** |
| 022 academic, two column | 0.591 | **0.054** |
| 082 newspaper, Chinese | 0.803 | **0.550** |
| 102 exam paper, Chinese | 0.866 | **0.713** |
| 101 exam paper, Chinese | 0.849 | **0.732** |

**Nothing regressed**, because the single-column case still goes through the same
comparison it always did — a page this cannot help is not a page it can hurt.

### The corridor was there and the code could not see it

**The first attempt merged the boxes' horizontal spans and looked at the gaps,
and it did not fire on half the pages it should have.** The first two-column page
measured has columns 39% of the width each with a clean 26-pixel corridor between
them, and the answer came back one column. The cause was not the corridor: a
**running head nineteen pixels tall** lying across the top of it bridged the gap,
because a box that crosses a gap is indistinguishable from a box that fills it
when all you have is spans.

**So the corridor is measured instead of inferred.** The page is cut into 256
strips and each is asked how much of the page's *height* is covered at that x. A
gutter is a run of strips that is almost entirely empty. The head covers 0.9% of
a 2017-pixel page, which is nothing, and the columns separate.

**Two thresholds were wrong before the right question was asked.** The gutter
width was first 2% of the page and then 1%, and both were guesses about a
corridor that the algorithm could not find either way. The 26-pixel corridor on
that page is 1.9% — the first threshold missed it by 1.4 pixels — but lowering
the threshold to 1% changed nothing at all, and that was the useful result: it
said the number was not what was being measured.

### What still does not work, and now for a reason

**The three newspaper cases did not move**, and they are not a bug this time.
Their coverage across the page is 65–98% of the height with no dip anywhere: the
columns are filled at every height by something at a different height. **There is
no corridor to find.** A newspaper page's reading order is not column-major
anyway — it is article-major, and articles cross columns — so what those pages
need is article segmentation, which is a different problem from this one and is
now named as such.

**The engine's version was raised to 1.1.0**, because the identity is half the
cache key and a page read before this change is not the page this build
produces. Raised for the whole engine rather than only the layout-aware half,
which re-reads more than it must and never serves a page it should not.

---

## 2026-10-10 — The layout model, scored on a corpus instead of by hand

**What:** `rapidocr` against `rapidocr+layout` on the 24-case corpus, same
build, same pages, engine chosen by whether the model is on disk. The first
numbers the layout work has had that were not counted by hand.

| | `rapidocr` | `rapidocr+layout` |
|---|---|---|
| headings found | **0 / 64** | **8 / 64** |
| tables found | **0 / 9** | **1 / 9** |
| error, reading order included | 0.466 | **0.419** |
| error, order ignored | 0.203 | **0.197** |
| mean loss | 0.566 | **0.511** |
| 24 pages | 53.2 s | 63.5 s |

**The layout model does what it was bought for, and not much of it.** Every
number moves the right way, headings go from none to some, and it costs 19%
more time. But **1 of 9 tables** is not a feature, it is a toehold.

**The earlier "7 of 7" was measuring something else, and this entry is the
correction.** That figure counted table *regions detected by label*, on seven
pages picked because they had tables, using a probe rather than the tool. This
counts tables that survive into the artifact, on all twenty-four pages, through
the whole pipeline. **Detecting a table is not producing one**: a region that
comes back as a table and cannot be turned into rows and cells is rendered as a
paragraph, deliberately, so that its words are not lost — and it counts as a
table not found. The second number is the honest one and the first should not
have been quoted as if it were.

**Where it fails is where the design said it would.** The worst reading-order
scores are the Chinese exam paper (0.849) and the Chinese newspaper (0.803) —
both multi-column, which this module documents as not solved. The content
score is much better than the order score on the same pages, which is the
signature of an engine that read the words and then put them in the wrong
sequence: exactly the distinction §8.5's two error rates exist to expose.

**The corpus was there all along, one directory outside the repository.**
`D:\workspace-new\wwkkyy0325\docfusion-corpus` — twenty-four cases from
OmniDocBench v1.5's base subset, covering single, double and three-column
layouts in both languages, across papers, books, slides, newspapers, magazines,
exam papers, reports and notes. It was reported missing twice in this session
because it was looked for inside the project and one level of the parent. It is
referenced by path, as §8.5 intends, and the path is worth writing down.

---

## 2026-10-09 — PaddleOCR-VL, run rather than read about

**What:** the local VLM candidate, actually run on this machine against the page
the local engine was measured on. It was not added. The short version is that it
**cannot use the graphics card at all**, and on the processor it is **155×
slower and worse at the one thing it was going to be better at**.

| | this page, end to end | the table |
|---|---|---|
| `rapidocr` + layout model | **1.6 s** | correct |
| PaddleOCR-VL, DirectML | **fails** | — |
| PaddleOCR-VL, CPU (int8) | 247 / 262 / 273 s | **degenerate in all three** |

### The graphics card cannot run it

All three ONNX graphs **load** on DirectML and **none of them run**. The failure
is a `Reshape` node — `node_view_5` in the vision encoder, `node_view_2` in the
decoder — in `MLOperatorAuthorImpl.cpp`. DirectML needs static shapes and a
transformer with a KV cache is made of dynamic ones. This is the risk that was
named before the test and it is now a measurement.

**And the error it produces is itself broken.** DirectML's message comes back in
the machine's own language — this is a Chinese Windows — and pybind11 decodes it
as UTF-8, so what a caller sees is `UnicodeDecodeError: 'utf-8' codec can't
decode byte 0xd3`, a sentence about bytes that says nothing about reshapes. The
cause is only visible in the log line above it. Worth knowing before shipping
anything on this provider on a non-English machine.

### The processor runs it into a wall

```
VLM Table Recognition: hit max_tokens without detectable loop
(max_tokens=2048, gen_tokens=2048, len=2048)
```

The table region ran to the token ceiling and produced `1980s 2000s 2005s 2010s
…` until it was cut off — **in three runs out of three**, with the fourth, the
first one, producing a correct table. This is the int8 export's documented
weakness: it holds up on a small block and degenerates on a whole region, and a
whole region is what it was given. The fp32 export is said not to have the
problem, and is 3.6 GB rather than 1.2 and substantially slower than a figure
already too slow to use.

### What this does not say

**The page is the easy case, and the easy case is where the VLM has nothing to
add.** A ruled table of printed text is what the recognition engine and the
layout model already read correctly in 1.6 seconds; a model twice as good at it
would still be 155× slower for no gain. The property the VLM is *for* is the
hard page — handwriting, a skewed photograph, a table with no rules, merged
cells — and **none of that was tested**, because the corpus that had such pages
is gone and a synthetic page cannot be made hard on purpose.

So the result rules out this model at this quantization on this page. It does
not rule out the class, and it should not be read as doing so.

**Also, for the record:** the output was not byte-identical between runs, which
is disqualifying on its own for a design whose cache key is a content hash. The
sampler in the harness explains some of that; the degenerate loop explains the
rest.

### What it cost

About two hours, 1.2 GB in the Hugging Face cache, and one venv — and it settled
a question that had been argued from blog posts for several turns. The reference
implementation was used rather than written, which is why it cost hours and not
a week.

---

## 2026-10-09 — The GPU, measured before it was believed in

**What:** a `directml` feature on `docfusion-store`, off by default, and the
numbers that decided it stays off. Not a capability: the models run 5–6× faster
on the graphics card and **the tool gets 9% faster**, because the models are not
where the time goes.

**The question was whether a GPU can be used without asking the user to install
anything.** It can. `DirectML` ships with Windows, works on NVIDIA, AMD and
Intel alike, needs no toolkit, and `ort` and `rapidocr-core` already carry the
feature — this was a flag and three lines, not an integration.

| | processor | DirectML |
|---|---|---|
| layout model, isolated | 451 ms | **77 ms** | 
| text detector, isolated | 275 ms | **56 ms** |
| one page, end to end | **2.335 s** | 2.637 s |
| eight pages | 13.11 s | **12.02 s** |
| twenty-four pages | 38.15 s | **34.69 s** |
| output | — | **identical** |

**The models get six times faster and a page gets nine percent faster**, and
that gap is the whole finding. A page is mostly things a graphics card cannot
help with: decoding the PNG, finding the contours in the detector's probability
map, joining the recognised lines, writing the file, and starting the process.
The inference is a minority share, and making a minority share six times smaller
does not make a page six times faster. It makes it 9% faster.

**On one page it is a loss**, because building a DirectML session costs about
two seconds of shader compilation the first time, and a one-page run never earns
that back. From eight pages on the cost is paid and the saving is steady at
**about 0.14 s a page** — measured at eight and at twenty-four, where the saving
grew with the page count rather than levelling off, so it is linear and not an
artefact of the first few.

**Off by default, and the fallback is written anyway.** A machine with no
DirectML device is the ordinary case, so a build that fails there drops back to
the processor instead of refusing to read the page — for both the detector and
the recogniser. It must not drop back *silently*: nothing downstream can tell
which provider ran, and that is the reason the feature is opt-in rather than on.
**The fallback itself is untested**, because there is no such machine here.

**CUDA was not the path.** It needs cuDNN alongside the toolkit, and cuDNN is
not on this machine — which is exactly the "user must install something first"
that rules it out. DirectML needs nothing, and that difference is the whole
reason to prefer the slower of the two interfaces.

**Whether the provider belongs in the engine's name is left open.** The output
was byte-identical here, down to the boxes, but the detector's probability map
differs in the fifth decimal, and a threshold is exactly where a fifth decimal
can flip a box. If this is ever turned on by default it has to join `rapidocr`
and `rapidocr+layout` in the identity, because the identity is the cache key and
the same bytes have to mean the same answer.

**How to re-measure:** the `directml` feature, and `cargo test --release
--features docfusion-store/directml -- --ignored times_the_detector`, which
prints the per-page time either way.

---

## 2026-10-09 — The layout model on the engine, and a page that stops being one column

**What:** the second half of the layout work. The detector runs in the same
process as the recogniser, the words it reads are put inside the regions the
detector found, and the twenty classes become the block kinds the IR already
had. **No endpoint, no vision model, nothing configured:**

```
## Annual Report

Revenue increased by twelve percent this year, and the growth came
from every region we operate in. Costs were flat, so the margin
widened slightly, and the outlook for the coming year is unchanged.

Table 1: Revenue by region

| Region | Units | Revenue |
| --- | --- | --- |
| North | 1,204 | 412,000 |
| South | 988 | 301,500 |
| East | 1,733 | 556,900 |
| West | 640 | 198,200 |
```

**The same page without it** is a heading that is not a heading, and a table read
as a column of loose words — with the values in the wrong rows:

```
Annual Report

...

Region

Units

Revenue

North

1,204

412,000

South

301,500

988
```

**Cost, release build, one page:** 1.44 s without the model, 2.59 s with. Almost
all of the difference is one-off — the 124 MB digest and the session load; the
inference itself was measured at 405 ms a page.

**The id changes when the model is here, and that is not a detail.** The local
engine calls itself `rapidocr` or `rapidocr+layout`, and the engine identity is
the cache key. Two different outputs under one name is a cache that serves one
to a run that asked for the other — so a knowledge base read before the model
arrived is read again after it, which is the correct thing for it to do.

### The coordinate bug, and why the earlier measurement survived it

**The model does not answer in the coordinates of the page.** Its `im_shape` and
`scale_factor` inputs are named for mapping its boxes back, and measured they
scale them by the ratio *twice over*: told a page is 2200 by 1700, it puts a
title 4.5 times too far right and 7.6 times too far down. The reference snippet
this was built from passes the page's shape and uses the boxes it gets back, and
so did the probe here — which means the drawing used to justify the first
measurement had boxes in the wrong places, and it was read as confirmation.

**It was believable because a wrong box on a page that is nearly square lands
near its own region.** On an A4 research report everything came out in the
middle-left, which looks like a plausible place for a figure. It only became
obvious on a page whose ink is in a corner.

**The counts were unaffected.** The mapping is applied to the coordinates after
the classes and scores are decided, so the "7 of 7 pages with tables" figure —
counted by class, not by box — stands exactly as it was measured. Only the
drawing was wrong.

**The fix is to tell the model the truth, then do the arithmetic here.** It is
given an 800×800 square, which it was, and told so; the boxes are scaled to the
page in two multiplications that can be read. The probe was corrected to match,
and now draws boxes on the ink.

**The test that should have caught it did not.** "Every box is within the page"
passed with the wrong mapping, because a box in the middle of a page is within
the page. It now checks where the title is — a tenth of the way down and a third
of the way across — which is the assertion the wrong answer fails.

### Where the judgement is, and where it isn't

**The twenty classes become seven kinds in one table, and that table is the
whole of what this module thinks about a page.** Several of the classes could be
argued either way: `header` and `footer` are running matter, and they are read
as ordinary text because every other extractor in this project reads them as
ordinary text. Dropping them only in the layout pass would be one rule, in one
place, for no stated reason.

**Columns before rows, and that order is why a wrapped cell stays one cell.**
Two lines of one cell do not overlap at all, so read as lines first they are two
rows — a row of `Widgets and` beside an empty cell and a row of `gadgets` beside
an empty one. Grouped into a cell first, its box is as tall as both lines and
the columns beside it land on it.

**A table that cannot be reconstructed is a paragraph, not an empty table.**
This is the one that loses data silently: `BlockKind::Table` with no rows
renders as nothing at all, so a one-column region called a table would take its
words out of the document. Three tests failed on exactly that before it was
fixed, which is what they were for.

**Reading order is top to bottom then left to right, and that is wrong for two
columns.** A second column is read as a second row of the first. Named here
rather than discovered later.


**What:** `PP-DocLayout_plus-L` — the thing that finds headings, tables, headers
and footers on a page — with a downloader to fetch it and a `doctor` line to ask
about it. Not wired into the engine yet: measured first.

**The measurement, on the project's own corpus.** The recognition engine finds
tables on **0 of the 7 pages whose ground truth has one**. This model finds
**all 7**. On a research report it put `table` on both real tables at 0.98 and
0.97, `paragraph_title` on both section headings, and caught the header, the
footer and a caption — eleven of eighteen regions that the engine reads as body
paragraphs today, headers and footers included, which is a knowledge base
quietly accumulating the same running head forty times.

**405 ms a page**, against about 4.2 s of recognition: roughly a tenth.

**Measured before it was wired, and that was the point.** Writing the ONNX
session, the DETR decode and the spatial join is four hundred lines; running the
same model from Python against the same pages is fifty. If the answer had been
no, the four hundred would have been wasted. It was twelve minutes of work
instead of a day to learn that this is worth doing.

**The model's own configuration is wrong, and only running it says so.**
`inference.yml` declares `mean 0, std 1, norm_type none`, which reads as "leave
the bytes alone" — and that gives a best score of 0.049 and no boxes at all.
Divided by 255 it gives 0.985. A file that describes the model is not the model.

**124 MB, so it is fetched and not embedded.** The line is *can* against
*nice*: 6.9 MB of recognition weights are embedded because they are the promise
— a scan is readable with no setup and no network — and moving that promise
behind a download would cost the whole of it to save 6.9 MB. The layout model is
an improvement, and the tool reads every page without it.

**The downloader follows Verse's decisions**, which the brief asked for: a
state a caller can watch, resume rather than restart, mirrors in order, the
digest checked before anything is trusted, and a failure reported *and*
returned. The last one is the bug Verse's own comment says it shipped once —
a fetch that returns an error without telling the callback leaves a window
saying "downloading" for ever, because the return value is the thing the window
does not read. Nine tests, against a local HTTP server, cover all of it,
including the case that a server ignoring `Range` must get a file started over
rather than appended to its own first half.

**Two mirrors, and the digest is the reason to trust either.** ModelScope and
Hugging Face serve byte-identical files — the hash computed from one is
published by the other — and that agreement is the only evidence a pinned hash
about a quarter of a gigabyte is the right one. Verified end to end by seeding
half the file and asking for the rest: **12 seconds, resuming from 40 MB, and
the digest matches**.

---

## 2026-10-09 — What a reading was made of, which the reading cannot say

**The question was whether the local engine can find tables.** It cannot, and
the answer was already on record twice: `ocr.rs` produces `BlockKind::Paragraph`
and nothing else — one occurrence of `BlockKind::` in the whole file — and the
project's own benchmark scored it 0 of 64 headings and 0 of 9 tables against 9
and 7 for a vision model. That is what a text detector and a recogniser are:
one finds boxes, the other reads them, and neither knows what a table is.

**The real defect was that the evidence was thrown away.** `Variant` carried
only `text`. The block kinds were known when the conflict was built and dropped
on the way, so the two readings reached the screen as two columns of prose with
nothing to tell them apart — and a person looking at that has no way to know
whether the engine failed to find a table or whether there was no table.

**So the shape travels with the reading.** `Variant` now carries counts —
headings, lists, tables, text — taken from the blocks the reading was built
from, written on the engine's line of the `## Conflicts` report, and read back
by the window. The report is the artifact and the window is a view of it, so the
answer is in the file rather than recomputed on screen.

**Counted, not inferred.** `Shapes::of` walks the top level only, so a four-row
table is one table rather than five. On the unknown side the parse is all four
counts or none: half a shape is worse than no shape, because the zeroes nobody
wrote would be a claim about the reading rather than a reading of the line. An
older document with no counts on that line still shows its text and its engine
name.

**What could be done instead, and is not.** Glyph boxes have height, and a line
set larger than the body can be *measured* rather than guessed at, so headings
are within reach and the IR already has a `confidence` field to mark them with.
Tables need a layout model, which is a dependency this project does not ship.
Both are written down as available; neither is worth doing before the thing they
would produce has been asked for, and a heuristic that occasionally invents a
heading is a worse answer than a plain statement of what each engine finds.

**The first version of the label made the comparison worse.** The sentence
explaining that the local engine only ever produces text made one column three
lines taller than the other, so the two readings no longer started at the same
height — a panel whose whole purpose is comparison, made harder to compare. The
explanation moved to the head of the panel, where it is said once, and each
reading kept a one-line label.

---

## 2026-10-09 — Whether it works, said and kept saying

**What:** The settings page says whether the configuration *works*, not only
whether it is saved: a dot and a word at the end of the title row, and errors
that appear under the field they are about.

**Saved and working are different questions.** Saving is local and instant.
Whether the settings work is a remote question that costs a model call, so it is
a button and not a side effect of saving — a save that sometimes takes a minute
is a save nobody presses twice. What was missing was not an automatic test but
*the answer staying put*: the test's result used to be a sentence that scrolled
away, and a person who tested, wandered off and came back knew nothing.

**The mark goes back to untested when anything it was about changes.** A tick
that survives an edit is a tick about settings that are no longer there, which
is worse than no tick at all. Editing also now says there are unsaved changes,
so what is on file and what is on screen are never quietly different.

**Three answers, not two.** Read the number is working; answering something
*else* is not working, however healthy the connection is — a model that cannot
see is exactly the case this page exists to catch, and calling it "connected"
would be the wrong reassurance.

**Errors moved to the fields.** The form used to say what was wrong in the last
line of the page, which means walking from the sentence to the thing it is
about, and a reader who does not make that walk concludes the form is broken.
Now the sentence is under the field and the field's border says which one.

**And this round's own bug was caught by the check, not by reading.** A field
error rendered as `TypeError: Cannot set properties of null`: the paragraph in
the markup was `id="model-error"` while the code looked up `set-model-error`,
and nothing but running it was ever going to say so. The failure showed up in
the note line of the Test button, which is where a thrown exception in that
handler lands — the designed path doing its job on an undesigned mistake.

---

## 2026-10-09 — The dropdown, thrown away for the one that cannot break

**What:** The hand-written model dropdown is gone. It is a `select` now, and the
closed control looks exactly like the text fields beside it.

**The bug could not be reproduced, and that is the whole story.** A diagnostic
was built that drives a real click through the list and draws any uncaught error
on the page: the option was clicked, the value was set, the list closed, nothing
threw. Two rounds of guessing at the cause had already gone by. A third would
have been the same guess with more conviction.

**The instrument broke twice on the way.** The first version registered its
error box on `load`, and errors happen after `load`, so it could never show
anything; the second lost a backslash to shell escaping and turned `\n` inside a
template literal into a real newline, which is a syntax error, which took out
the whole stub and made the application report `invoke is not a function`. Both
times the report was about the tool. It is the third time this session that a
broken instrument has lied about the thing under it.

**So the component was replaced with the one that has no failure mode left.**
Keyboard, screen readers, type-ahead, popup placement, scrolling and the dark
popup are all the browser's and have been for thirty years. `appearance: none`
plus a chevron of this program's own makes the closed control identical to the
text fields around it. About a hundred and twenty lines of hand-written control
code were deleted to make room for it.

**Two controls now, one showing.** The `select` when the endpoint has said what
it serves; the text field when it has not, because a model nobody listed can
only be typed. The model already set stays in the list even when the endpoint
does not list it or lists it as blind, since dropping it would change the
setting without saying so.

**A vendored library was the other option and was not taken.** No build chain
means a component arrives as a file plus its styles, in a window that currently
carries no third-party code at all — and it would still be code that can be dead
to a click. The trade is written down in the design for whoever wants the
custom popup badly enough.

---

## 2026-10-09 — Models that cannot see, and a list that swallows the mouse

**What:** The model list offers only models that can be shown a page, and the
list's interaction stopped doing the thing that made it feel like it was holding
the mouse.

**The endpoint had been saying it all along, and `probe()` was throwing it
away.** `GET /v1/models` on DeepSeek answers with `input_modalities` per model —
`deepseek-flash` is `["text","image"]`, `deepseek-v4-pro` is `["text"]` — and
`Probe::Models` kept only the `id`. So the window offered a model that fails on
every scanned page, which is the failure this whole tool exists to make loud.
The setup guide has said `deepseek-flash` is the only model on that API that
accepts images since the day it was written; the interface ignored a fact that
was already written down two files away.

**A model that says nothing is still offered.** Most OpenAI-compatible servers
report nothing about their models, so reading silence as a refusal would hide
every model on every one of them. It is a yes, a no, or nobody said — and only
the no is left out, with a count in the sentence so that leaving it out is said
rather than done quietly.

**`doctor` gained the same check, and it is the one that matters.** A configured
model that cannot see produces a problem before anything is read, rather than a
document-long discovery. Checked against the real endpoint with
`--model deepseek-v4-pro`, which is the model that prompted this.

**The interaction: nothing swallows the press any more.** The usual way to stop
a list closing before a click lands is to `preventDefault` the mousedown, and
that is a known way to leave a page feeling like it is holding the mouse. The
list is closed by *blur* instead, guarded so a click on the list is not leaving
the field — which needs the options to be focusable, so they carry
`tabindex="-1"`. Plus a press anywhere else closes it, which is what everything
that opens over a page does.

**And one more thing that would have reopened the list under the pointer:**
focusing the field at the end of a choice fires the focus handler, which opens
the list. A `choosing` flag covers it. *Could not be reproduced from here* — the
harness has no mouse — so this is the mechanism removed plus the two ways it is
known to go wrong, not a confirmed diagnosis.

---

## 2026-10-09 — The list is ours, and a build that was never delivered

**What:** The model list is drawn by this page rather than by the browser, the
hints say what is now known rather than what was known an hour ago, and the two
overlapping buttons were a build that never reached anybody.

**The overlap was not a bug in the code, it was a bug in the delivery.**
`cargo test` compiles a test binary for `src/main.rs`; it does not produce
`docfusion-app.exe`. So the change that moved the button out of the field was
written, rendered in the harness, and never compiled into the program anybody
was running. The fix had been right for two turns. Building the binary is what
makes a frontend change real, and the harness rendering it correctly is not
evidence that the application has it — they are two different consumers of the
same three files.

**The browser's list could not have been styled, so it was replaced.** A
`datalist` popup is drawn by the browser, outside the page, and no stylesheet
can reach it — it would have been a system menu in the middle of a page with its
own type, spacing, corners and colours. `#models` is now an ordinary `<ul>` with
`role="listbox"`, positioned under the field, in the same colours as everything
else, and it is *capturable* — which the browser's popup is not, and which is
most of the reason it was worth doing.

**The behaviour a native one would have given had to be written out:** opening on
focus, filtering only once somebody has typed (a field holding a model name, on
being opened, would otherwise show exactly that one model and look broken),
arrow keys, Enter, Escape, and closing on blur. The filter keys off `picking`
rather than off the text being non-empty, and that distinction is the whole of
it.

**The hints now say what the measurement says.** The thinking hint had the token
counts from the first experiment; it has the page timings from the second one,
which is the finding that actually decides the question — 1m55s against 1m13s
with an identical body.

---

## 2026-10-09 — The model list, and what thinking actually buys

**What:** A button under the model field that asks the endpoint which models it
serves and offers them, plus an experiment that answered a question about
gating thinking — with a result that argues against the gate.

**The model name is the one field no amount of care can fill in.** It is a
string that either exists on the far end or does not, and nothing a person can
remember or check will say which. The endpoint knows, and has said so since the
day `probe()` was written — the listing has been in the store all along and used
by `doctor`, and the window had never asked.

**A button under the field rather than an icon inside it.** The first attempt put
an icon in the field, and the browser draws its own arrow there for an input
with a `list` — two affordances in one place, one on top of the other. Hiding the
browser's did not take, so the field keeps the arrow everybody already knows and
the action became a word underneath it. Which is also more discoverable: a
labelled button beats a small picture.

**The experiment: the same newspaper page, read with thinking off and on.**

| | time | output | conflicts |
|---|---|---|---|
| off | 1m13s | 20548 B | 1 |
| on | 1m55s | 21299 B | 1 |

**Every word of the body is identical.** The only difference is that with
thinking on the model added `## SECURITIES AND EXCHANGE COMMISSION` and
`### Submission for OMB Review` — it classified the headings. The conflict
against the local engine did not move.

**Which answers the question that prompted it.** The proposal was to gate
thinking: read everything without it, and escalate where a conflict appears.
Measured, that gate would have fired on exactly this page and changed nothing
about the disagreement, because thinking alters *structural interpretation* and
a conflict is usually a *reading* disagreement. It would have spent 1.6× the
time to arrive at the same text.

**What thinking does buy is heading structure**, which is a real column in the
project's own benchmark and a real thing to want — but it is a trade to make
once per installation, not a gate to open per conflict. And it is one page and
one run: the honest way to settle it is `docfusion bench` over the corpus both
ways, which costs money and has not been run.

---

## 2026-10-09 — Thinking was on, and costing most of the bill

**What:** A Test button on the settings page, a switch for the model's thinking
mode, and the answer to a question that turned out to be worth more than the
feature.

**The question was whether thinking mode was on.** It was, because the request
never mentioned it and the provider's default is on at high effort. Asked to
reply with one word, `deepseek-flash` spent **24 of its 26 output tokens
reasoning**; with thinking asked off, the same request spent **1**. That is not
a rounding error and it applies to every page of every document.

**Two things fell out of the measurement that are worse than the bill.**
`temperature` is *ignored* while thinking is on — a reasoning model takes no
sampling temperature — so the `temperature: 0` this client has always sent, with
a comment calling it load-bearing, was doing nothing at all. And the design
leans on the content hash pinning which engine reads a document, on the premise
that the same bytes produce the same answer; a model deliberating at high effort
is a worse fit for that premise than one that is not.

**Measured rather than read.** The docs were unreachable from here and the
search summaries were hedged and mentioned retired model names, so the endpoint
was asked directly. It is a better instrument than a documentation page: it
answers about the account, the model and the version actually in use.

**The request carries the setting, so it is a switch and not a constant.**
`thinking` is DeepSeek's spelling. Servers that do not know the field ignore it
— checked, by sending DeepSeek an invented one and getting a normal answer back
— but a strict provider rejects an unrecognised argument outright. So the switch
sends *nothing* when it is on rather than sending a different value, because
"do not send it" is the only version of this that a provider which has never
heard of the field can accept.

**The Test button shows the endpoint a picture.** A connection check cannot see
the failure that matters: a model served without its multimodal projector is
reachable, listed, fast, and blind, and answers every request in fluent text
about nothing. Asking it to read a number off a page separates "reachable" from
"can see", and it is the one question somebody configuring this by hand cannot
ask. Three outcomes are said apart, including "it answered something else", with
both answers shown so the failure is visible rather than described.

**The picture is drawn rather than shipped.** A five-by-seven bitmap font and
ten glyphs, scaled up — no font, no asset, nothing to go missing or turn out to
be the wrong file. It also has the property a corpus page would lose after the
first run: the tool has never seen it before.

**And it was checked against the real endpoint before being wired to a button.**
The live test reads 4321 off the drawn page, which is the only way to know that
a glyph has no bit in the wrong place and that the picture is large enough not
to be mistaken for a page whose image never arrived. It is kept as a test that
does not run by itself, because it spends money and needs a key.

---

## 2026-10-09 — The key field, and a settings page that looks finished

**What:** The key is masked, with an eye to reveal it, and the settings page was
redesigned — centred and bounded, a title and a sentence saying what the page is
for, labels that read, and the one control that changes anything set apart from
the fields.

**The request and the design disagreed, and the disagreement was the work.** Dots
the length of the real key, and an eye that shows it, both need the window to
know the key — and the window had been built so that it never does. The view is
serialised for *every* command, so a key in the view is a key in every document
read.

The answer keeps both: the view carries `key_length` and never the key, and the
eye is a command of its own. The secret crosses the boundary when somebody
presses a button whose tooltip says what it does, and at no other time.

**The dots are a placeholder, not a value.** A field *holding* a row of dots is
one accident away from saving a row of dots as the key — and a placeholder cannot
be submitted. The cost is that placeholder bullets are wider than the characters
they stand for, so the masked field is visually longer than the revealed one. The
count matches; the width does not. That is in the design rather than left to be
noticed.

**The old test survived and got stronger.** `the_settings_never_carry_the_key_itself`
was written against `has_key`; it is now written against `key_length` and checks
the serialised form rather than the field. It is the test standing between a
convenient settings page and a secret in every command the window sends, so it
was not going to be deleted to make room for the feature.

**The fixture was lying, and the picture was small enough to hide it.** The
stub's key was twenty-six characters with a `key_length` of thirty-five typed
next to it, which would have made any shot of the masked field evidence of
nothing. The length is derived from the key string now. A fixture whose two
halves disagree is worse than no fixture: it looks like proof.

---

## 2026-10-09 — The report on a document, told apart from the document

**What:** A window showing a document with a disagreement was confusing in three
ways at once. It said "one place" in the footer with nothing to press; it said
`openai-compatible · rapidocr` and nothing about what those are; and the report
itself was English prose at the end of the document, reading as though somebody
had written it into the page.

**A correction first.** I said earlier that the two readings were nowhere on
screen. They were, at the end of the Markdown, under `## Conflicts` — I had
counted with `grep -c conflict` and the heading is capitalised. The report
existed; what did not exist was any way to get to it or any reason to think it
belonged to us rather than to the document.

**The window splits the Markdown at the heading this program writes.** In a file
the two belong together — a disagreement has to travel with the document it is
about, and an agent reading the document should see the doubt as well as the
text — and that decision stands. On a screen it is wrong, so the document is
drawn alone and the report is drawn after it as a section of its own.

**Reading it back is a contract between two crates, so it is tested as one.**
The test renders a document that really has a disagreement through the real
renderer and reads it back the way the window does, which is the only test that
can see both halves. Three more cover what the format actually looks like: every
line of a reading is indented, including the empty ones, so an empty line is one
of the reading's lines rather than the end of it — and the indentation is what
stops a reading that mentions "Page 3" from being read as another finding.

**The kind is kept as it was written rather than matched against a list here.**
The frontend holds the table of kinds and shows anything outside it as it
arrived, because a window that recognised two sentences and dropped a third
would lose a finding without saying so.

**`openai-compatible` is a protocol, not a name for anything.** The footer now
says what each engine is — the configured endpoint, the engine inside this app —
with the name it goes by everywhere else one hover away, and the panel labels
each reading with both.

**Neither reading is marked as the right one, and the two are side by side.**
That is the entire reason to show them at all: a reading-order failure, which is
the common one and the one this engine makes on multi-column pages, is
indistinguishable from a good reading until they are next to each other. Each is
capped in height and scrolls inside itself, because a disagreement about a whole
page is two pages of text and nobody compares two pages on a screen.

**Still missing, and named as missing:** nothing in the body points at the place
the engines parted company. Closing that means the renderer emitting a marker at
the block, which puts it in the text a pipeline indexes — a change to the
artifact, not to the window. The design says so rather than leaving it implied.

---

## 2026-10-09 — The theme button, as a shape rather than a word

**What:** The theme control is now a sun or a moon — a stroked `<svg>`, drawn in
the button's own colour. No icon font, no library, and no emoji: `☀` is one
codepoint away from the system font rendering it in colour, which is not
something a button gets to decide.

**A wordless button still needs words.** The sentence the label used to carry
goes into `title` and `aria-label`, translated like the rest — an icon has no
accessible name of its own, and "sun" is not the name anyway. "Switch to the
light theme" is.

**Two shapes, one wrong, and the picture was right about which.** The crescent's
inner arc was taking the far side of its circle instead of the near one, which
draws very nearly a whole circle, so at sixteen pixels it read as a sun. The fix
is one flag in one path.

**Then the real bug, which the picture found after the code had said it was
fine.** `hidden` is an IDL property of `HTMLElement`. An `<svg>` is not an
`HTMLElement`, so `svg.hidden = true` set a JavaScript field on the object and
never wrote the attribute — the stylesheet's `[hidden]` never matched and both
shapes stayed on screen at once. And because the diagnostic read `.hidden` back,
it returned the value that had just been written and reported that everything
was working. `toggleAttribute("hidden", …)` is what works; the probe now reads
the attribute, because the property is the thing that lied.

**The lesson is not about SVG.** A check that asks the code what it thinks it
did will agree with the code. The picture could not: it showed a sun where a
moon belonged, and no amount of reading would have.

---

## 2026-10-09 — Dark, because it was asked for

**What:** The window opens dark. One button switches it to light and the choice
is remembered. No system query: this interface has one appearance decision to
make and dark is the answer to it, so there is no third state and no way for the
window to open in one theme and change its mind a moment later.

**One palette rather than two blocks.** Every colour is declared once as
`light-dark(light, dark)`, and one `color-scheme` on the root element decides
which of the pair is used. Two blocks of declarations is the arrangement where
somebody changes a colour in one and not the other, and the themes quietly stop
matching; written this way the two values sit on the same line, which is the
only place they can be compared.

**`color-scheme` does a second job that is easy to miss.** It tells the webview
what kind of controls to draw. The scrollbars, the text cursor, the password
reveal button and the autofill background are all the browser's rather than
ours, and a dark page that claims to be light gets every one of them wrong — no
`background-color` fixes that.

**Links in a document had no rule at all.** They kept the browser's default,
which is a dark blue chosen for a white page, and on a dark one it is very
nearly invisible. The dark theme is what made it visible; it was wrong in light
too, just less obviously.

**The window is darker than the page it is about to draw.** `backgroundColor` in
`tauri.conf.json` holds the same value as `--paper` in the dark theme. The
window exists for a moment before the webview paints, and that moment is white
unless it is told otherwise. This is the one colour that lives in two places,
and it is written down in the design for that reason.

**The check now has two axes, and paid for itself twice.** Every state is
rendered in both themes, and the two boxes that use the warning wash — the
settings-file banner and the engine's account of a failure — are checked in both
because they are the two places a missing colour would be least obvious. The
`flipped` shot boots dark and clicks, which is what proves the switch redraws
rather than the pinned shots, which only prove the stylesheet has both palettes
in it.

---

## 2026-10-09 — A settings tab that writes the file the command line reads

**What:** A third tab with an endpoint, a model and a key, and a button. Before
this, pointing the tool at a model meant opening a TOML file by hand — which is
why the endpoint was set once and never touched again.

**It writes `config.toml` itself, not a second file of its own.** Two stores for
one setting are two answers, and the one that loses is the one the command line
acts on.

**Edited in place, and that took a fix.** `toml_edit` changes the three keys and
leaves the rest, which is the whole point: most of that file is the comments
explaining it, and a save that deleted them would delete them silently. The
first version still lost something — assigning a value replaces the whitespace
and comments attached to it, so a note written at the end of a line went with
it. The trivia is now carried over by hand. The test fixture is the shape the
real file has, trailing note and commented-out setting included, because a test
against a tidier file than the one in use is a test of the wrong thing.

**The key goes one way.** Typed in the window, written to the file, never read
back: the view carries `has_key` and never the key. That is not only hygiene —
it decides what the form means. An empty key box is *I did not touch this*, not
*delete it*, because a form that re-sent the key on every save would be a form
that needs the key.

**Clearing the endpoint is how the model is switched off.** Endpoint and model
are set together or not at all, since one endpoint can serve several models and
guessing would silently read documents with whatever it happened to be serving.
An empty value is removed from the file rather than written as `""`, which is a
different thing to a URL parser.

**Saving takes effect at once**, so the endpoint moved behind a lock in the
window's state. A settings page whose settings apply on restart is not one.

**The check caught the same mistake twice.** The first `badaddress` shot set the
bad value before the tab had finished filling the form from the view, so the
value was overwritten and the shot showed a successful save — a check that looks
like it passed when it was never asked. It is the second time in this session a
scenario was written in the wrong order, and both times the picture was the
thing that said so.

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
