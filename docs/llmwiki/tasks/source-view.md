# The source, beside the reading

## Why

The copy the knowledge base keeps was added for `reindex` — reading a document
again after the engines change, from bytes rather than from a path that may have
moved. It turns out to have a second use that needs no new storage at all: **the
one thing the Markdown cannot show anybody is what it was looking at.** A
document read off a scanned page is right or wrong against that page, and until
now the page was somewhere else.

## Decisions

Asked and answered:

- **A split in the pane, like the editor.** Source on the left, reading on the
  right. Not "open it with the system program", which would have worked for every
  format including the ones with no pages and would have put the comparison
  outside the thing doing the comparing.
- **No alignment.** The Markdown does not record which page a block came from.
  Writing `<!-- page: 3 -->` into it would mean the renderer, the search index,
  the exports and the editor all learning to filter it, which is a change to the
  file format for a view whose whole job is to be looked at.

## What has pages, and what does not

| source | what the left half shows |
|---|---|
| PDF | the page, rendered at 110 dpi |
| image, scan | the picture itself, which is one page |
| docx, xlsx, pptx | **nothing** — a zip of XML has no picture of itself |
| txt, md | **nothing** — the page is its words, and they are on the right |

The last two get a sentence rather than an empty half, because an empty half
reads as a pane that failed to load.

## Shape

```
crates/docfusion-gui/src/state.rs    Source
crates/docfusion-gui/src/lib.rs      Watched, page_count, page_image, three commands
crates/docfusion-gui/ui/app.js       toggleSource, original, drawPage, closeSource
crates/docfusion-gui/ui/style.css    .pages, .split.editing, .nothing
```

## Steps

1. [x] `Source` — `Pages { count }` or `Nothing`, named rather than numbered,
       because "no pages" and "one page" are different answers and only the
       first needs explaining to anybody.
2. [x] `page_count` and `page_image`, dispatching on the mime: a PDF is opened
       with `RenderedPages` and rendered a page at a time; an image is decoded.
       Verify: a test that a picture of a number is one page, that a Word mime
       and a text mime are none, and that bytes claiming to be a PDF are an
       error rather than a document with no pages.
3. [x] **A test caught a small lie here**: `page_image` answered an out-of-range
       page for an image with the same picture. It refuses now — a function that
       silently returns something about a page that does not exist is a function
       that hides a mistake in whoever asked.
4. [x] Rendering at **110 dpi** rather than the 200 the recognition path uses: a
       page beside a column of text is not a page being read character by
       character, and four times the pixels is four times the bytes over the
       bridge for a picture nobody is zooming into.
5. [x] The copy is read **once** and held while the pane is open, because
       rendering is per page and `RenderedPages` reopens the document for each
       one. Let go by `close_source` when the pane closes: a scan somebody looked
       at a minute ago is not a thing to keep in memory for the rest of the
       session.
6. [x] Pages are drawn **as they are scrolled to**, through an
       `IntersectionObserver` with `rootMargin: "600px"`, and each one only once.
       A two hundred page scan is two hundred pictures, and rendering them to
       show the first is a minute of work nobody asked for.
7. [x] A page goes to the window as a **data URI**: the content policy allows
       `img-src data:` and nothing else, and the copy is a file inside the
       knowledge base rather than one of the window's own assets.
8. [x] The two modes are exclusive. Each closes the other, and opening the
       source with unsaved words in the editor asks first, because closing it
       would throw them away.
9. [x] Docs: `design.md` §11.5; `changelog.md`.

## What was measured

The first version was called slow, and it was. On a real 696 KB PDF, one page at
110 dpi:

| | before | after |
|---|---|---|
| `RenderedPages::open` | 752 ms first time, **0.18 ms after** | unchanged |
| render | 63 ms | 63 ms |
| encode | PNG, 690 ms, 958 KB | JPEG, 381 ms, 210 KB |
| over the bridge | 1.3 MB as base64 | 280 KB |
| an image source | decoded, then encoded as a 2 MB PNG | its own bytes, no work |

**The number that decided the fix was not the one that looked biggest.** The
752 ms is PDFium starting up rather than a parse paid per page, so caching the
open document — which looked like the obvious win — would have bought nothing
after the first page. What was per page was the encoding, and what was worst was
the image case, where a file that already *is* a picture was being decoded and
encoded again as a much larger one.

`PAGE_DPI` is the dial that is left: 110 dpi is 301 ms of encoding against
88 dpi's 198 ms, and the page is the evidence somebody is checking a character
against, so it is not a number to shrink for speed without saying so.
