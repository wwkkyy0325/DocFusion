# Table quality: the last table, the words in the cells, and PDFs

## Why

Three gaps are left after the stride fix took the corpus to 8 of 9 tables.

- **061 is the one table that is still not found.** 0 of 1, and the one that is
  found scores 0.39. The other eight are 0.78 and up. Either the region is not
  detected, or it is detected and the model cannot read it, and those are
  different problems.
- **The words in the cells are read by a 6.9 MB model.** On a photographed bank
  schedule every cell was in the right place and three characters were wrong —
  `63,00.00` for `63,000.00`, and `合计` missing from the total row. The Python
  chain, which uses the full recognition model, gets both right. Structure is
  solved; recognition is now the ceiling.
- **A PDF with a text layer never gets a table at all.** The table model runs
  inside the recognition backend, and a PDF with text never reaches it:
  `engines: [pdfium]`, cells pressed into paragraphs. Reproduced on a generated
  printed table.

## Steps

1. [x] Find out what 061's table is and which of the three it fails at — verify:
       the detector's region and the model's grid for that page, printed rather
       than guessed at. **Answer: none of them.** The table is detected, read
       into five columns, and correct; the reference was invalid Markdown, and
       the scorer was reading a six-row table as a one-row table followed by a
       paragraph. Fixed in the scorer. 061 goes 0/1 alike 0.39 → 1/1 alike
       0.51, and the corpus to 10 of 10.
2. [x] A larger recognition model, fetched on demand, with the embedded one
       still the no-setup floor — verify: bench tables 9/9 or the similarity up,
       and the three characters above read correctly. **Answer: no.** Measured
       both larger sets in the same family on the corpus, and neither is worth
       the download:

       | | tiny (embedded, 6.9 MB) | small (32 MB) | medium (139 MB) |
       |---|---|---|---|
       | mean loss | **0.413** | 0.412 | 0.418 |
       | content error | 0.187 | **0.185** | 0.190 |
       | headings | 8/64 | **9/64** | 8/64 |
       | tables alike | 0.878 | 0.890 | **0.900** |
       | corpus time | **552 s** | 665 s | 1224 s |

       Small is nine cases better and four worse, and it fixes one cell of the
       bank schedule while breaking another — `63,00.00` becomes correct and
       `固定资产 | 金额` becomes `国产 | 空`, because the larger detection model
       moves the line boxes the rest of the pipeline reads. Medium is worse than
       the embedded one on loss and content at twice the time. **The recogniser
       is not the ceiling.**
3. [x] Tables in a PDF that has a text layer — verify: a printed table PDF comes
       out with a Markdown table, the words taken from the PDF rather than
       recognised, and the corpus numbers unchanged. **Done.** A generated
       printed table reads as `engines: [pdfium+tables]` with all five columns,
       and the corpus is unchanged at 0.413 loss and 10 of 10 tables, which is
       the point: this adds a capability the corpus cannot see, not a score.
