# Headings, and reading a table from its rules

## Why

Two things, both opened by the same measurement: the corpus's worst number is
`headings 8/64`, worth a quarter of the loss, and a question about whether a
table with merged cells can be brought across.

## What was found

### 1. The reference cannot express a heading's depth — done

All sixty-four headings in the corpus's references are level one. Not one uses
`##`. OmniDocBench's ground truth records that a block **is** a title, not how
deep a title it is, and the conversion to Markdown had to write a number. A
reference that cannot express a depth cannot measure one.

Measured, comparing the text alone:

| | level compared | level ignored |
|---|---|---|
| headings | 8/64 | **21/64** |
| mean loss | 0.413 | **0.313** |

Fixed in the scorer, the same way and for the same reason as the table row that
Markdown had split across two lines. The words still have to be the words; only
the depth is dropped.

### 2. A heading written over two lines is cut in half — done

The biggest single loss is 071, a Security and Exchange Commission notice with
three columns and seven headings, of which the engine finds none. Looked at
rather than guessed at: every heading on that page is two lines —

```
SECURITIES AND EXCHANGE
COMMISSION
```

```
Submission for OMB Review;
Comment Request
```

— and the engine returns `SECURITIES AND EXCHANGE` and `Submission for OMB
Review;`.

That is **not** the reading but the writing. The front matter's title
held both lines the whole time — `SECURITIES AND EXCHANGE
COMMISSION` — so the
block was right and the renderer wrote it with the break left in, and Markdown
ends a heading at the first newline. One line fixes it: 071 goes from 0 of 7 to
**4 of 7**, the corpus from 21 of 64 to **25 of 64**, the mean loss from 0.313 to
**0.300**.

**One case got worse, and the two cases are genuinely different.** A newspaper
page has a kicker above a headline —
`中宣部、中央文明办等14部委下发通知要求` over `2011年深入开展文化科技卫生"三下乡"活动`
— the layout model puts both in one region, and that reference calls them two
headings, where 071's reference calls its two lines one. 081 went from 5 of 13
to 4.

The difference that tells them apart is the size of the type, which the renderer
cannot see and the block builder can. Measured across the corpus: a wrapped
heading's lines differ by between **1.00 and 1.26**, and a kicker's by **2.31
and 2.42**. Nothing falls between, so the threshold is a half rather than
something tuned. Splitting a heading region at 1.5 takes 081 back to **6 of 13**,
keeps 071 at 4 of 7, and leaves **every other case unchanged**.

Headings 25 of 64 to **27 of 64**, mean loss 0.300 to **0.298**.

**Two of the seven are unreachable whatever is done.** The reference has
`Submlsslon for OMB Revlew; Comment Request` — the ground truth itself carries
an OCR error — and `[SEC File No. 270-121, 0MB Control No. 3235-0110]`, with a
digit zero where the letter O belongs.

### 2b. What is left of the thirty-two misses, looked at one by one

Not guessed at. Each case was run and its headings diffed against the reference.

| what | headings | is it the engine's? |
|---|---|---|
| **LaTeX in the reference** — 041, 042 | 7 | No. `$u_{r}(V_{2})$` cannot come out of a recogniser; the page has to be read as maths. The corpus has exactly seven headings containing `$` and they are exactly these seven. |
| **A reference that is itself wrong** — `推厂` for `推广`, `Submlsslon`, `0MB` | 3 | No |
| **Reading order** — 082, 101 | 6 | Yes, and it needs article segmentation |
| **A full-width bracket read as half-width** — 111, `（五）` written `(五）` | 2 | Yes, and **one character wide** |
| **A space dropped** — 112, `8. 铝箔` written `8.铝箔` | 1 | Yes, and **one character wide** |
| **A heading not found at all** — 101, the exam instructions | 2 | Yes |
| the rest | ~11 | Yes |

**The measurement is deliberately not being widened for any of it.** The two
single-character ones are the tempting ones — fold full-width to half-width in
`normalize` and 111 comes back, allow a space to be one or the other and 112
does — and both would be hiding a real defect: the engine wrote `(五）`, with the
two brackets at different widths, which is not a thing any typesetter writes.
A heading scored as found for that would be a number that says the engine is
better than it is. The character metric already counts the character once.

**041 and 042 are a dead end** and are written down so they are not walked into
again: seven headings, all blocked by the same thing, none of it reachable
without reading formulae.

### 3. Rules are the right idea for merged cells and are not ready — not done

Built `rules.rs`: the grid from the lines a form is drawn with, with a merged
cell being a position that has no line through it. Unit-tested on generated
tables, it reads a row span and a column span **exactly**, where the model does
not.

Then measured. On the corpus, where several tables are ruled:

| | model | rules first |
|---|---|---|
| tables | **10/10** | 7/10 |
| similarity | **0.878** | 0.828 |

Two cases carry all of it. One is a table with a coloured header band: a block
of dark pixels across its top swallows the rules around it, and the top two rows
come back as one cell each, two columns wide. Excluding colour from "ink" made
it worse rather than better, because the band's own edges are then not lines
either.

So it is reverted and removed. **The idea is sound and the detector is not**,
and there is nothing to tune it against: no corpus case has a merged cell, so
the only ground truth for the case this exists for is a table the author drew.
Tuning against two hand-made pictures is how a detector gets fitted to two
hand-made pictures.

What would have to be true to try again: a corpus with ruled tables and merged
cells in it, so the detector is measured rather than believed.

## Steps

1. [x] Compare headings without their depth — verify: the corpus goes from
       8/64 to 21/64 and the loss from 0.413 to 0.313, with the words still
       compared.
2. [x] Find where the rest of the headings go — verify: 071 read and looked at,
       not guessed at. Answer: they are cut in half by the layout model.
3. [x] Read a table from its rules — verify: exact on a generated row span and a
       generated column span, and **worse on the corpus**, which is why it is
       not in the tree.
