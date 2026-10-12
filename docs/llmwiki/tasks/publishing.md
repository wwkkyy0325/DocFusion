# Publishing the repository

## Why

Eighty-five files and seventy-four commits of work that only existed on one
machine. The reason to publish is not distribution — it is that a project whose
whole argument is *"never silently lie about what a document said"* should be
readable by somebody who did not write it.

## Decisions

- **MIT.** The workspace manifest already declared it, so the licence file is
  the agreement rather than a change of mind. It is the most permissive of the
  common options, which is what was asked for.
- **`master`, not a rename to `main`.** The branch has been called that for
  seventy-four commits; renaming it is a decision about the repository rather
  than about publishing it, and the remote was empty so nothing had to be
  reconciled.
- **Two commits rather than one.** The source pane and the public documents are
  unrelated changes that happened to land in the same afternoon.
- **The readme is long on purpose.** The internal record is written for whoever
  works on this next; a front door is a different document for a different
  reader, and it has to carry the things a newcomer cannot infer from the code —
  including the ones that do not flatter the numbers.

## Steps

1. [x] Read the tree for anything that must not be published. Verify: no
       `config.toml`, no `.env`, and no key-shaped string in the pushed tree —
       the only `sk-` matches are placeholders in four tests, and the real
       configuration has always lived in `%LOCALAPPDATA%`, outside any
       repository.
2. [x] Confirm the suite is green before publishing a claim about it. Verify:
       `cargo test --workspace` — **603 passed, 0 failed**.
3. [x] `LICENSE` — MIT, matching what the manifest already said.
4. [x] `README.md`. Verify: every number and command in it is one that was run,
       and the benchmark table matches `design.md` §8.5 rather than a recollection
       of it.
5. [x] `THIRD-PARTY.md`. Verify: crate licences read from the registry manifests
       in `D:\dev\rust\cargo\registry\src` rather than from memory — which is how
       `comrak` turned out to be BSD-2-Clause, not the MIT the design doc's
       technology table implies.
6. [x] Push. Verify: `git ls-remote --heads origin` and `git rev-parse HEAD` agree
       with `origin/master`.

## The thing this turned up

**A permissive licence on the code does not extend to the benchmark.** The
evaluation corpus is 24 pages from OmniDocBench **v1.6** — the number in the first
draft of this file was v1.5, and it was wrong; see the changelog entry for
2026-10-13. The dataset is released for
research use only and not for commercial use. The corpus has always lived outside
this repository, for the original reason that real documents are often private —
which turns out to have been the right call for a reason nobody had in mind when
it was made. Nothing from the dataset is redistributed, and both the readme and
`THIRD-PARTY.md` say so where somebody will read it.

## Left alone, and known

- **`docs/setup.md` §1 is stale.** It says the recognition weights, "6.9 MB of
  them", are inside the binary. They are not, and have not been since
  `models-on-demand`: the set is six files and 144 MB, and a fresh installation
  reads no scan until they arrive. The readme states the current behaviour, so
  the two now disagree in public.
- **`design.md` §12's technology table gives PDFium "MIT/Apache"** where the
  licence is BSD-3-Clause, and does not mention that `comrak` is BSD-2-Clause.
- **`design.md` §13 still lists "Vector search, MCP server" under P4**, which §2
  rules out from the opposite direction: embeddings are a non-goal *ever*, not a
  phase.
