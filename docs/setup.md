# DocFusion setup

Everything here is a command you can run, and every command was run before it
was written down. The last section says which parts could not be verified from
the machine this was written on, so you know where to be careful.

---

## 1. Nothing is required to start

The recognition engine and its weights — 6.9 MB of them — are inside the binary.
There is nothing to install, nothing to download, and no model to choose. PDFs
with a text layer, images, and Office documents all work the moment the binary
exists.

Build it:

```bash
cargo build --release
```

The binary is `target/release/docfusion.exe`.

## 2. Prove it works, before configuring anything

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
endpoint        https://api.deepseek.com
  serves        deepseek-flash, deepseek-v4-pro
  selected      deepseek-flash  (listed)
capability      vision
```

Healthy, with no endpoint at all — this is a supported configuration, not a
problem:

```
endpoint        none configured
capability      ocr-only
```

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
250 MB, so four of them dominate everything else. Reading is otherwise about
three seconds a page on CPU, and eight pages take 71 s at one pipeline, 41 s at
two and 27 s at four.

If memory matters more than speed, turn it down — in the configuration file or
on one run:

```bash
docfusion --max-ocr-pipelines 1 --root kb add scan.pdf
```

```toml
# config.toml
max_ocr_pipelines = 2
```

Below one is one: zero would read nothing, so it is treated as a mistake rather
than obeyed.

## 9. Verification

Two of the three original candidates for a local VLM — llama-server, Ollama and
LM Studio — could not be exercised, because none of them is installed on the
machine this was written on and none of the hosts that distribute them is
reachable from it. Those rows in §5 are the vendor's documented commands, not
something that was run here. Everything else in this document was executed, and
the output quoted is the output that came back.

The DeepSeek path in §4 was verified end to end: configured in the file, read a
real page, `doctor` reporting `vision`, and a recorded conflict when the model
and the bundled engine disagreed about a number.
