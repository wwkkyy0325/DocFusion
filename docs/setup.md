# DocFusion setup

Everything here is a command you can run, and every command was run before it
was written down. The last section says which parts could not be verified from
the machine this was written on, so you know where to be careful.

---

## 1. What is required to start, and what is not

Build it:

```bash
cargo build --release
```

The binary is `target/release/docfusion.exe`.

**A document that already carries its text needs nothing else.** PDFs with a text
layer, and DOCX, XLSX, PPTX, TXT and Markdown, are read by code inside the binary
— no model, no network, no configuration, nothing to download.

**A scan or an image needs weights, and they are not inside the binary.** They
were, once. Fetching them on demand bought a single place that says what
"complete" means, digests checked before anything is trusted, and mirrors tried
in order — at the price of a fresh installation that reads no image until the
weights arrive. Six files, 144 MB in total, in three parts. (`doctor` counts in
mebibytes, so it prints the same set as **138 MB**; the two numbers are one set.)

| | size | needed? |
|---|---|---|
| the recognition set — detector, recogniser, angle classifier, dictionary | 6.9 MB | **yes.** Without it a page of text that is a picture cannot be read at all |
| the table structure model (`SLANet_plus`) | 7.8 MB | no. Without it a table stops being read as a grid |
| the page layout model (`PP-DocLayout_plus-L`) | 124 MB | no. Without it no headings, tables, headers or footers are found on a scan |

Fetch them with the command line:

```bash
docfusion doctor --fetch
```

or with the **Download** button on the window's settings page, which shows the
same set file by file as it arrives. Both write to the same place, and neither
runs on its own: nothing is fetched at startup and nothing refreshes in the
background. `DOCFUSION_MODEL_CACHE` overrides where they go; by default that is
`%LOCALAPPDATA%\DocFusion\models`.

A download that is interrupted resumes rather than restarts, and every file is
checked against a pinned SHA-256 before anything is trusted. The digests are in
`crates/docfusion-store/src/models.rs`, which is the only place the set is
defined.

## 2. Prove it works, before configuring anything

This page is a PNG, so it is a page of text that is a picture: **fetch the
recognition set from §1 first**, or it is the one thing that will not be read.
Nothing else has to be configured.

```bash
docfusion --root kb add page.png
```

Expected:

```
added    kb\docs\ppt-1001115-eng-page-003.md
```

Open that file. It should contain the words that were on the page, with
front matter naming the engine that read it:

```yaml
engines: [rapidocr]
verified: 0/1
conflict_count: 0
```

If it does, the whole local path works and no configuration is needed for it.

## 3. Adding a vision model (optional)

A vision model reads *structure* — headings, tables, reading order across
columns — which a line recogniser cannot. It is optional: with none configured,
every scan is still read. What it costs is a round trip per page, and its
weakness is fine print on large pages (see §8).

**What an endpoint has to provide.** DocFusion speaks one protocol and nothing
else:

- `POST {base_url}/v1/chat/completions`
- a `messages` array whose `content` is an array of blocks, containing
  `{"type": "text", ...}` and `{"type": "image_url", "image_url": {"url":
  "data:image/png;base64,..."}}`
- the image block must be in a **`user`** message; some providers reject it in
  `system` or `assistant`
- the answer in `choices[0].message.content`, as Markdown

Anything satisfying that works. llama-server, Ollama, LM Studio, vLLM and the
hosted APIs all do, which is why there is one client rather than five.

## 4. Cloud, with DeepSeek

Put this in the configuration file. On Windows that is
`%LOCALAPPDATA%\DocFusion\config.toml`; run `docfusion schema` and read the
`config.path` field if you are somewhere else.

```toml
endpoint = "https://api.deepseek.com"
model    = "deepseek-flash"
api_key  = "sk-..."
```

There is a **Test** button on the settings page that shows the endpoint a drawn
page with a number on it and says whether the answer contained the number. Use
it: an address that answers is not the same as a model that can see, and the
ones that cannot answer in fluent text about nothing.

**Thinking mode is asked to be off, and you should leave it that way.** The
provider's default is on at high effort, and it is most of what the model
writes — asked for a single word, `deepseek-flash` spent 24 of its 26 output
tokens on it, and 1 with it turned off. It also makes `temperature` meaningless,
which is the setting that keeps a model from paraphrasing. Turn it on only if
your endpoint refuses a request carrying a field it does not recognise, which is
how the request carries this one:

```toml
reasoning = true
```

`deepseek-flash` is the only model on that API that accepts images —
`deepseek-v4-pro` is text-only, and asking it to read a page fails. Check with:

```bash
curl -s https://api.deepseek.com/v1/models \
  -H "Authorization: Bearer $DEEPSEEK_API_KEY"
```

Look for `"input_modalities": ["text", "image"]` on the entry you intend to use.

Everything in the file can also be given per run, and either form wins over the
file, so a one-off test does not need editing anything:

```bash
docfusion --endpoint https://api.deepseek.com --model deepseek-flash \
          --api-key "$KEY" --root kb add page.png
```

## 5. Local, if you want to run the model yourself

The same client, so nothing else changes. Start whichever you have, then point
DocFusion at it:

| Runtime | Command | Endpoint to configure |
|---|---|---|
| llama.cpp | `llama-server -m model.gguf --mmproj mmproj.gguf --port 8080` | `http://127.0.0.1:8080` |
| Ollama | `ollama serve`, then `ollama pull <vision model>` | `http://127.0.0.1:11434` |
| LM Studio | start the local server from the app | `http://127.0.0.1:1234` |

**The projector is not optional, and leaving it out fails silently.** A GGUF
vision model served without its `mmproj` file does not error — it answers `200
OK` with repeated nonsense, because the image never reached the model. DocFusion
catches this by looking at the reported prompt-token count, which is far too low
for a page image when this has happened, and says so. But it is worth knowing
before you spend an afternoon reading gibberish.

**Network reality, measured from the machine this was written on:**

| Host | Reachable |
|---|---|
| `github.com` | **no** |
| `ollama.com` | **no** |
| `lmstudio.ai` | **no** |
| `hf-mirror.com` | yes |
| `modelscope.cn` | yes |
| `api.deepseek.com` | yes |

So on that machine, the usual "download the installer" steps for Ollama, LM
Studio and llama.cpp do not work, and the model files come from a mirror rather
than from HuggingFace. Check from your own machine before following any guide
that assumes otherwise.

## 6. Check it worked

```bash
docfusion doctor
```

Healthy, with an endpoint:

```
knowledge base  kb
engines         pdfium, docx, xlsx, pptx, text-passthrough, scan
memory          31.2 GB, so 16 pages at once
endpoint        https://api.deepseek.com
  serves        deepseek-flash, deepseek-v4-pro
  selected      deepseek-flash  (listed)
models          present, 138 MB — the words on a page, and the shape of it
capability      vision
```

Healthy, with no endpoint at all — this is a supported configuration, not a
problem:

```
endpoint        none configured
models          present, 138 MB — the words on a page, and the shape of it
capability      ocr-only
```

**`capability` has three values, not two**, because there are three things a
machine can be. `ocr-only` is a claim about a download now rather than about
what the program ships: with no recognition weights on disk, the answer is
`text-only`, and it says so rather than promising to read a scan it cannot.

Exit code is `0` when nothing is wrong and `1` when a configured endpoint could
not be reached. A missing `/v1/models` listing is not an error: nothing requires
an endpoint to implement it.

`doctor` reports `vision` only when the endpoint answered *and* listed the model
you configured. If it could not confirm that, it says `ocr-only` and prints the
reason — which is the honest answer, even if the endpoint would in fact work.

## 7. When it goes wrong

| Message | What it means |
|---|---|
| `no OCR or vision backend is configured, so images cannot be read` | The file is fine; nothing is set up to read it. This is the only case where the message is about configuration. |
| `the scan engine failed: could not reach http://...` | A backend **is** configured and did not answer. Check the endpoint, not the configuration. Exit code 1. |
| `the scan engine failed: the endpoint at ... answered 401 when asked to read a page` | The key is missing or wrong. |
| `the endpoint reported only N prompt tokens for a page image, so the image never reached the model` | The most likely cause is a vision model served without its multimodal projector. |
| `... is not valid: unknown field \`api_ky\`` | A misspelt key in the configuration file. It is an error rather than a silent no-op on purpose. |

## 8. What to expect, measured

From 24 pages of real documents, both engines reading the same corpus:

| | bundled OCR | DeepSeek-V4.1-Flash |
|---|---|---|
| character error, in reading order | 0.466 | **0.245** |
| character error, ignoring order | 0.203 | **0.171** |
| headings found | 0 / 64 | **9 / 64** |
| tables found with the right shape | 0 / 9 | **7 / 9** |
| slowest page | 56 s | 74 s |

**They fail on different pages.** On one Chinese newspaper page the bundled
engine recovered 97.7% of the characters and the vision model 72.8% — it cleanly
omitted the masthead, the editor, the proofreader and the phone number. The
provider converts a page to at most about 1024 image tokens, which downsizes a
2754×3939 scan to roughly 900×1280, and fine print does not survive that. The
bundled engine runs at full resolution and does not care.

So: keep both. The vision model is for structure and for pages whose layout
matters; the bundled engine is better at dense small text, and it is what
verifies the other one.

Resource use, measured on the same machine:

| | peak memory |
|---|---|
| a command that reads nothing (`list`) | 2 MB |
| reading a small page | 131 MB |
| reading a 2754×3939 page | 392 MB |
| a 32-page document, one pipeline | 439 MB |
| a 32-page document, default settings | 1.2 GB |
| process start-up | 41 ms |

**Most of that is the number of pages read at once**, not the document. Pages
are read in parallel and each reader holds a loaded model; each costs about
250 MB, so several of them dominate everything else.

**How many is decided from the machine, not chosen here.** A quarter of the
physical memory, divided by what a reader costs, capped by the number of cores.
`docfusion doctor` says what it worked out:

```
memory          31.2 GB, so 16 pages at once
```

On this machine, reading one page at a time takes 71 s for eight pages, eight at
once takes 16 s, and sixteen at once takes 17 s — two gigabytes more for no
measurable gain. **Between eight and sixteen is where the returns stop on a
sixteen-core machine**, and where that point falls elsewhere is not something
this was tested on. If memory matters more than speed, say so — in the
configuration file or on one run:

```bash
docfusion --max-ocr-pipelines 8 --root kb add scan.pdf
```

```toml
# config.toml
max_ocr_pipelines = 8
```

Below one is one: zero would read nothing, so it is treated as a mistake rather
than obeyed.

## 9. Calling it as a step in something else

This is the section for a program or an agent driving `docfusion`, rather than a
person typing at it. There is no server to start and no port to arrange: every
call is a process, and a process that reads a document takes about three seconds
a page.

**Read the contract rather than this page.** It is printed by the tool:

```bash
docfusion schema
```

It lists every command, the fields each one emits, the exit codes, and the path
and keys of the configuration file. It is generated from the same source as the
behaviour, so it cannot be out of date in the way a document can.

**Two rules the output keeps, so a caller can rely on them:**

- **stdout carries results and nothing else** — NDJSON with `--json`, human
  lines otherwise. Progress, warnings and errors always go to stderr. A caller
  parsing stdout never has to defend against a log line.
- **Exit codes say what happened**: `0` success, `1` failure, `2` some inputs
  succeeded, `3` nothing can read this input, `4` the index disagrees with the
  file tree.

**It is safe to run twice.** The same file added again is a cache hit in
milliseconds; the same content under a different name is recognised as the same
document. Two processes writing to one knowledge base wait for each other rather
than failing.

**Put the credentials in the configuration file, not on the command line.** A
key passed as an argument is a key in a process listing and in a shell history.
See §4 — every setting there can also be given per run, and either form wins
over the file.

A worked example — one line of NDJSON per document, on stdout, with nothing else
mixed into it:

```console
$ docfusion --json --root kb add page.png 2>/dev/null
{"command":"add","conflicts":0,"id":"541182669b…","output":"kb\\docs\\ppt-…md",
 "path":"…/page.png","reused":false,"status":"done"}
```

`reused` says whether the work was done or answered from the index, so a caller
can tell a fresh reading from a repeat without timing anything. Filter it with
whatever the calling program already has — `jq`, a JSON parser, a line split on
`"reused":false`.

**The knowledge base is a directory of Markdown plus one SQLite file.** Nothing
is stored anywhere else and nothing is stored only in the index — if the index
is lost, `docfusion reconcile` rebuilds it from the tree.

## 10. Uninstalling

**This section is about the installed application**, from the `.exe` on the
releases page. A tree built with `cargo build` has no uninstaller; deleting
`target/` and the directories below is the equivalent.

DocFusion writes in three places, and they are not equally replaceable:

| | where | what it costs to lose |
|---|---|---|
| the program, and the models, cache and `config.toml` | `%LOCALAPPDATA%\DocFusion` | nothing — install it again, fetch 144 MB again, re-enter the API key |
| the library | `%USERPROFILE%\Documents\DocFusion` | **the documents.** The Markdown, and the copies of what was read, which is what `reindex` needs |

The first row is one directory, and that is a coincidence rather than a decision
worth knowing about if you go looking: the installer puts the program in
`%LOCALAPPDATA%\DocFusion`, and the program had already chosen that same folder
for the files it owns — both names came from the product name. Upgrading does not
disturb it, because the installer replaces the files it put there and leaves
everything else alone, so the models and your key survive a reinstall.

Uninstalling removes the program. It then **asks twice**, because the last two
rows are not the same decision:

1. **The template's own "delete the application data" checkbox** decides whether
   the models, the cache and `config.toml` go. They are files the program can
   fetch or rebuild, and `config.toml` holds an API key — which is a reason to
   remove it, not a reason to hesitate.
2. **A separate question about the library**, asked only if one is there, naming
   its path and saying plainly that converted documents in it cannot be
   recovered. **The default button is No.** Press Enter and your documents stay.

Two things it will never do:

- **Delete a file you added.** DocFusion copies nothing into the library that it
  did not write, and the originals are wherever you left them. `docfusion delete`
  has the same rule.
- **Delete a library it cannot find.** The location is configurable — `--root`,
  or `DOCFUSION_ROOT` — so a library somewhere else is *not* found and *not*
  touched. This is deliberate: guessing where somebody keeps five hundred
  documents, and guessing wrong, is a far worse failure than leaving files on
  disk. If you moved your library, remove it yourself.

**A silent uninstall (`/S`) keeps everything.** It skips the confirmation page
that the checkbox lives on, so no question is answered and nothing is deleted.
That is by construction rather than by accident, which is the only way a rule
about deleting documents should work.

## 11. Verification

Two of the three original candidates for a local VLM — llama-server, Ollama and
LM Studio — could not be exercised, because none of them is installed on the
machine this was written on and none of the hosts that distribute them is
reachable from it. Those rows in §5 are the vendor's documented commands, not
something that was run here. Everything else in this document was executed, and
the output quoted is the output that came back.

The DeepSeek path in §4 was verified end to end: configured in the file, read a
real page, `doctor` reporting `vision`, and a recorded conflict when the model
and the bundled engine disagreed about a number.
