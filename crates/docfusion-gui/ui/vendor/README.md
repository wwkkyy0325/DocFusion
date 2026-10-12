# What is in here, and how it got here

`codemirror.js` is CodeMirror 6, bundled into one ES module.

**Why it is committed rather than installed.** This window has no build chain:
`ui/` is served to the webview as it is, and the content policy is
`default-src 'self'`, so a module can only be loaded from this directory. A
dependency that has to be fetched is a dependency that cannot be, on a machine
that is offline, which is where this program is meant to work.

**How to rebuild it.** In a scratch directory, from a machine with a network:

```
npm install codemirror @codemirror/lang-markdown esbuild
cat > entry.js <<'JS'
export * from "codemirror";
export { markdown } from "@codemirror/lang-markdown";
export { tags } from "@lezer/highlight";
export { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
JS
npx esbuild entry.js --bundle --format=esm --minify --target=es2022 --outfile=codemirror.js
```

**What it is:**

| | |
|---|---|
| `codemirror` | 6.0.2 |
| `@codemirror/lang-markdown` | 6.5.2 |
| bundled | 2026-10-12 |
| size | 597 KB, 208 KB gzipped |

CodeMirror is MIT licensed. The licence is not copied here because the source it
covers is not either: what is in this directory is a build of it, and the build
is what this program ships.
