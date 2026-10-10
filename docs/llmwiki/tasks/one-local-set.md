# One local set, and a card that shows it

## Why

The settings tab offers the local models as one line of prose and a `Download`
link. What is missing is the answer to three questions somebody actually has
before pressing it: *what is this*, *what does it do*, and *how big is it*.
Verse answers all three with a card per model, and `layout.rs` already borrows
its downloader — progress, resume, mirrors, verify-before-trust — so the shape
is the only thing left to borrow.

The second thing: there is no such thing as *one* set today. The recognition
weights (four files, 6.9 MB) are downloaded by `build.rs` and compiled into the
binary; the layout model (124 MB) and the table model (7.4 MB) are downloaded
at run time into a different directory. Three places to change when a model
moves, two answers to "where do models live", and a settings tab that can only
describe half of what it is talking about.

Now: **one set, one catalogue, one root, and everything in it is a download.**

| File | Bytes |
|---|---|
| `ppocrv6-tiny/PP-OCRv6_det_tiny.onnx` | 1,829,618 |
| `ppocrv6-tiny/PP-OCRv6_rec_tiny.onnx` | 4,489,813 |
| `ppocrv6-tiny/ch_ppocr_mobile_v2.0_cls_mobile.onnx` | 585,532 |
| `ppocrv6-tiny/ppocrv6_tiny_dict.txt` | 27,156 |
| `PP-DocLayout_plus-L.onnx` | 129,736,329 |
| `SLANet_plus.onnx` | 7,782,138 |
| **total** | **144,450,586 (144 MB)** |

## What this gives up

A fresh install reads no image and no scanned PDF until the set has arrived —
including the command line. "Offline from the moment it is installed" was a
promise made in four places in this tree (`models.rs`, `ocr.rs`, `engines.rs`)
and it is being traded for one set that is the same everywhere. Asked and
answered: the trade is wanted.

Sizes below are in the units the rest of this project already used: `MB` means
MiB. The set is 144,450,586 bytes, which is the **138 MB** on the card — and the
parts add up to the whole, which they would not if this one number were
decimal.

## Steps

1. [x] `layout.rs` becomes `models.rs`, because it stopped being about the
       layout model when it started holding the recogniser's weights too —
       verify: `cargo build` with the old module path gone, and the module doc
       saying what the file now is.
2. [x] The four recognition weights become `Spec`s in that catalogue, in
       `ppocrv6-tiny/` under the one root — verify: a test holds their digests
       against `rapidocr_core`'s pins, which is the check `build.rs` used to
       make at build time.
3. [x] One root: `default_root()` honours `DOCFUSION_MODEL_CACHE` the way
       `model_cache_dir()` already did, and `Spec.file` may be a path —
       verify: `ensure_of` creates the file's parent, and a fetch into a
       subdirectory lands there.
4. [x] `build.rs` and everything it generated are gone — verify: no
       `include_bytes!` of a model, no `weights` module, no `Source::Bundled`,
       and `cargo clippy` clean with `#[ignore]`d test updated to say why.
5. [x] `RapidOcrBackend::local()` reads the set from disk and refuses, naming
       the files that are missing, when any of them is not there — verify:
       `absent` and `present_all` are unit-tested, and the refusal was read off
       the real thing by hand: with the dictionary deleted, `doctor` said one
       file of the set was missing and `--fetch` put it back, in the
       subdirectory the pipeline looks in. The refusal path itself is not under
       test, because reaching it means emptying a per-user directory that other
       tests on the same machine share.
6. [x] The window draws one card: name, what it does, total size, and a state
       line — verify: the markup test in `lib.rs` finds the elements, and the
       progress is the whole set's, not one file's six times.
7. [x] A page that needs reading and cannot says so and points at the settings
       tab, rather than arriving as an engine failure — verify: both languages
       have the sentence and the view carries the cause.
8. [x] `docfusion doctor` says one line, not two.
9. [x] Docs: `design.md`, `changelog.md`.
