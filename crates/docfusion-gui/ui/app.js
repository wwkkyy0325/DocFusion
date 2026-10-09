// The window's half of the conversation.
//
// Everything of substance happens on the other side of `invoke`: this file
// draws a view, asks for another one, and does two things the webview is better
// at than Rust — copying to the clipboard and knowing what is selected.

// Everything this file says, in the two languages it says it in.
//
// A failure arrives as two halves, and the split is why this can be translated
// at all. `cause` is the kind of failure, which the window owns and says in the
// reader's language. `detail` is the account the engine or the operating system
// gave, which is somebody else's words in somebody else's language: it is shown
// as it came, because translating a cause by guessing at it would be inventing
// an explanation rather than reporting one.
const TEXT = {
  en: {
    document: "Document",
    records: "Records",
    add: "Add a document",
    switch_lang: "中文",
    switch_title: "用中文显示界面",

    empty_lead: "Drop a document here, or add one.",
    empty_hint:
      "Anything with a text layer is read as it is. A scan or a photo is read by the engine inside this app.",
    documents: "Documents",
    settings: "Settings",

    reading: (name) => `Reading ${name}…`,
    scan_note: "a scan takes a few seconds a page",
    seconds: (n) => (n < 60 ? `${n}s` : `${Math.floor(n / 60)}m ${n % 60}s`),
    could_not_read: (name) => `${name} could not be read`,

    read: "read",
    already: "already in the knowledge base",
    disagreed: (n) => `the engines disagreed in ${n === 1 ? "1 place" : `${n} places`}`,
    copy: "Copy",
    copied: "Copied",
    copy_failed: "Could not copy",

    nothing: "Nothing here yet.",
    to_review: (n) => `${n} to review`,
    status: { done: "done", partial: "partial", failed: "failed" },

    dialog_title: "Choose a document",

    causes: {
      unsupported: "Nothing here can read this kind of file.",
      "engine-failed": "The engine that reads this kind of file failed.",
      unreadable: "The file could not be read.",
      unwritable: "The result could not be written.",
      index: "The index of the knowledge base is not usable.",
      missing: "That document is no longer in the knowledge base.",
      settings: "The settings file could not be used.",
      "nothing-to-read": "It was read, and there was nothing in it.",
      unknown: "Something went wrong.",
    },
  },

  zh: {
    document: "文档",
    records: "记录",
    add: "添加文档",
    switch_lang: "EN",
    switch_title: "Show the interface in English",

    empty_lead: "把文档拖到这里，或者点「添加文档」。",
    empty_hint: "带文字层的文件直接读出文字。扫描件和照片由程序内置的引擎识别。",
    documents: "文档目录",
    settings: "设置文件",

    reading: (name) => `正在读取 ${name}…`,
    scan_note: "扫描件大约每页几秒",
    seconds: (n) => (n < 60 ? `${n} 秒` : `${Math.floor(n / 60)} 分 ${n % 60} 秒`),
    could_not_read: (name) => `${name} 读不了`,

    read: "已读取",
    already: "知识库里已有",
    disagreed: (n) => `引擎在 ${n} 处不一致`,
    copy: "复制",
    copied: "已复制",
    copy_failed: "复制失败",

    nothing: "还没有记录。",
    to_review: (n) => `${n} 处待确认`,
    status: { done: "完成", partial: "部分完成", failed: "失败" },

    dialog_title: "选择文档",

    causes: {
      unsupported: "没有能读这种文件的引擎。",
      "engine-failed": "读这种文件的引擎出错了。",
      unreadable: "文件读不开。",
      unwritable: "结果写不到磁盘上。",
      index: "知识库的索引用不了。",
      missing: "这个文档已经不在知识库里了。",
      settings: "设置文件用不了。",
      "nothing-to-read": "读出来了，但里面是空的。",
      unknown: "出错了。",
    },
  },
};

// Read defensively rather than destructured outright. If this is ever absent
// the module would throw on its first line and leave a blank window with
// nothing to read on it; this way the failure is drawn like any other.
const invoke = globalThis.__TAURI__?.core?.invoke;

const el = (id) => document.getElementById(id);

let current = null;
let running = false;
let ticker = 0;
let started = 0;

// A read blocks for as long as it takes, and the window has to keep saying so.
// Long enough that switching to an already-read document does not flash a
// spinner for one frame, short enough that a real read shows one immediately.
const SPINNER_DELAY = 120;

const LANG_KEY = "docfusion.lang";

let lang = startingLang();

const t = () => TEXT[lang];

/// The language to open in: the last one chosen, or the one the machine is set
/// to. Somebody who has never touched the switch gets their own language
/// without being asked, which is the whole point of asking the machine.
function startingLang() {
  try {
    const chosen = localStorage.getItem(LANG_KEY);
    if (chosen && TEXT[chosen]) return chosen;
  } catch {
    // Storage can be unavailable. It is not worth failing to open over: the
    // machine still knows what language it is in.
  }

  return (navigator.language ?? "").toLowerCase().startsWith("zh") ? "zh" : "en";
}

function toggleLang() {
  lang = lang === "zh" ? "en" : "zh";

  try {
    localStorage.setItem(LANG_KEY, lang);
  } catch {
    // As above: the interface still switches, it just does not remember.
  }

  applyLang();

  // Everything on screen was drawn in the other language, so it is drawn again.
  // Nothing has to be told what changed: the whole view is a function of it.
  if (current) draw(current);
}

/// The words that live in the markup rather than in the view.
function applyLang() {
  const words = t();

  document.documentElement.lang = lang === "zh" ? "zh-Hans" : "en";

  el("tab-document").textContent = words.document;
  el("tab-records").textContent = words.records;
  el("add").textContent = words.add;
  el("lang").textContent = words.switch_lang;
  el("lang").title = words.switch_title;
}

// ── drawing ─────────────────────────────────────────────────────────────────

function draw(view) {
  current = view;

  drawProblem(view.problem);
  drawScreen(view);
  drawRecords(view.records);
  drawStatus(view.screen);
}

/// Something wrong with the installation rather than with a document, said once
/// at the top and left there whatever else is on screen.
function drawProblem(problem) {
  const banner = el("problem");

  banner.hidden = !problem;
  banner.replaceChildren();
  if (!problem) return;

  banner.append(node("strong", "", t().causes[problem.cause] ?? t().causes.unknown));
  if (problem.detail) banner.append(node("span", "detail", problem.detail));
}

/// The two halves of a failure, drawn where they are read.
function drawTrouble(box, trouble) {
  box.append(node("p", "cause", t().causes[trouble.cause] ?? t().causes.unknown));

  // Left out when there is nothing in it: a frame with nothing inside reads as
  // something that failed to load.
  if (trouble.detail) box.append(node("pre", "detail", trouble.detail));
}

function drawScreen(view) {
  const box = el("screen");
  const state = view.screen;

  if (state.kind === "empty") {
    const where = document.createElement("dl");
    where.className = "where";
    for (const [term, value] of [
      [t().documents, view.knowledge_base],
      [t().settings, view.config_path],
    ]) {
      where.append(node("dt", "", term), node("dd", "", value));
    }

    box.replaceChildren(
      describe("empty", [
        node("p", "lead", t().empty_lead),
        node("p", "hint", t().empty_hint),
        where,
      ]),
    );
  } else if (state.kind === "working") {
    box.replaceChildren(working({ name: state.name }));
  } else if (state.kind === "ready") {
    const article = document.createElement("article");
    article.className = "document";
    // Rendered by the same Markdown pipeline the command line writes with, and
    // escaped there: extracted text is not trusted markup. Not translated, and
    // not translatable — it is the document.
    article.innerHTML = state.html;
    box.replaceChildren(article);
  } else {
    const failure = document.createElement("div");
    failure.className = "failure";
    failure.append(node("h2", "", t().could_not_read(state.name)));
    drawTrouble(failure, state.trouble);
    box.replaceChildren(failure);
  }

  box.scrollTop = 0;
}

function drawStatus(state) {
  const status = el("status");
  status.replaceChildren();

  if (state.kind !== "ready") {
    status.hidden = true;
    return;
  }

  status.hidden = false;
  status.append(node("span", "", state.engines.join(" · ") || t().read));

  if (state.reused) {
    status.append(node("span", "", t().already));
  }

  if (state.conflicts > 0) {
    status.append(node("span", "warn", t().disagreed(state.conflicts)));
  }

  status.append(node("span", "spacer", ""));
  status.append(button(t().copy, "quiet", copyAll));
}

function drawRecords(list) {
  const records = el("records");
  records.replaceChildren();

  if (!list || list.length === 0) {
    records.append(node("li", "none", t().nothing));
    return;
  }

  for (const record of [...list].reverse()) {
    const item = document.createElement("li");
    const open = button("", "", () => openRecord(record));

    // A status this frontend does not know is shown as it arrived rather than
    // hidden: the three it does know are the three the index can hold, and
    // anything else is worth seeing.
    const status = t().status[record.status] ?? record.status;

    open.append(node("span", "name", record.name));
    open.append(
      node(
        "span",
        "meta",
        [status, record.conflicts ? t().to_review(record.conflicts) : null, record.created_at]
          .filter(Boolean)
          .join(" · "),
      ),
    );

    item.append(open);
    records.append(item);
  }
}

/// The screen a read shows while it is happening, drawn here rather than waited
/// for.
///
/// The other side sets the same state, but it only reaches this side when the
/// call returns — and the call is the thing that takes minutes. A window that
/// says nothing for the length of a scan is indistinguishable from one that has
/// stopped, so the waiting is drawn before the work is asked for.
function working(hint) {
  const box = document.createElement("div");
  box.className = "working";

  const clock = node("span", "elapsed", t().seconds(0));
  clock.id = "elapsed";
  const progress = node("p", "progress", "");
  progress.append(clock, document.createTextNode(` · ${t().scan_note}`));

  box.append(node("div", "pulse", ""), node("p", "", t().reading(hint.name)), progress);
  return box;
}

// ── asking ──────────────────────────────────────────────────────────────────

async function add() {
  if (running) return;

  let path;
  try {
    // The dialog is the operating system's, but its title is ours, so it is
    // asked for in the language the rest of the window is in.
    path = await invoke("choose_file", { title: t().dialog_title });
  } catch (error) {
    return failed("DocFusion", error);
  }
  if (!path) return;

  await read(() => invoke("process", { path }), fileLabel(path));
}

async function openRecord(record) {
  // The name goes with the id because a failure has to name what failed, and
  // the record is the only place either side has it.
  await read(() => invoke("open", { id: record.id, name: record.name }), record.name);
}

/// Runs one job, with the window saying what it is doing throughout.
///
/// One at a time: the two sides share a single view, so a second job started
/// while the first is in flight would have them writing over each other.
async function read(action, name) {
  if (running) return;

  // Every read ends with a document on screen, so it starts by showing the
  // panel that document goes in — including when the file was dropped while
  // the records were open, which is the case that would otherwise work
  // invisibly.
  showTab("document");

  running = true;
  el("add").disabled = true;

  const spinner = setTimeout(() => showWorking(name), SPINNER_DELAY);

  try {
    draw(await action());
  } catch (error) {
    failed(name, error);
  } finally {
    clearTimeout(spinner);
    stopClock();
    running = false;
    el("add").disabled = false;
  }
}

function showWorking(name) {
  el("screen").replaceChildren(working({ name }));
  drawStatus({ kind: "working" });

  started = Date.now();
  stopClock();
  ticker = setInterval(() => {
    const clock = el("elapsed");
    if (clock) clock.textContent = t().seconds(Math.round((Date.now() - started) / 1000));
  }, 1000);
}

function stopClock() {
  clearInterval(ticker);
  ticker = 0;
}

/// A failure the window can draw. Everything the pipeline reports comes back
/// through the view; this is for what never got that far, which is the one case
/// where the account is the whole of what there is to say.
function failed(name, error) {
  const view = current ?? { knowledge_base: "", config_path: "", problem: null, records: [] };
  draw({
    ...view,
    screen: { kind: "failed", name, trouble: { cause: "unknown", detail: String(error) } },
  });
}

async function copyAll() {
  const article = el("screen").querySelector(".document");
  if (!article) return;

  const label = el("status").lastElementChild;
  const was = label.textContent;

  try {
    await navigator.clipboard.writeText(article.innerText);
    label.textContent = t().copied;
  } catch (error) {
    label.textContent = t().copy_failed;
  }

  setTimeout(() => (label.textContent = was), 1200);
}

// ── tabs ────────────────────────────────────────────────────────────────────

function showTab(which) {
  for (const name of ["document", "records"]) {
    el(`tab-${name}`).setAttribute("aria-selected", String(name === which));
    el(`panel-${name}`).hidden = name !== which;
  }
}

/// The list is a file that this process is not the only writer of, so it is
/// re-read when it is asked for rather than remembered.
///
/// Only the list is redrawn. The answer also carries the document, and drawing
/// that too would be harmless except for what comes with it: the document pane
/// is scrolled back to the top, so glancing at the records would lose your place
/// in a long document. Switching to a tab should change that tab.
async function openRecords() {
  showTab("records");
  try {
    const view = await invoke("records");
    current = view;
    drawRecords(view.records);
  } catch (error) {
    // Said inside the list rather than over the document: the panel that failed
    // to fill is the panel that should explain itself.
    el("records").replaceChildren(node("li", "none", String(error)));
  }
}

// ── what the webview is better at ───────────────────────────────────────────

// Copying from a rendered document normally puts the markup on the clipboard as
// well as the words, so pasting somewhere that understands it brings the
// formatting along. A knowledge base is the words: this keeps only those.
document.addEventListener("copy", (event) => {
  const selection = window.getSelection();
  if (!selection || selection.isCollapsed) return;

  event.clipboardData.setData("text/plain", selection.toString());
  event.preventDefault();
});

// Tauri hands the window a dropped path directly, because the webview's own
// drop event does not carry one.
if (globalThis.__TAURI__?.webview?.getCurrentWebview) {
  window.__TAURI__.webview.getCurrentWebview().onDragDropEvent((event) => {
    if (event.payload?.type !== "drop") return;
    const path = event.payload.paths?.[0];
    if (path) read(() => invoke("process", { path }), fileLabel(path));
  });
}

// ── wiring ──────────────────────────────────────────────────────────────────

function node(tag, className, text) {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text) element.textContent = text;
  return element;
}

function button(label, className, onClick) {
  const element = document.createElement("button");
  if (className) element.className = className;
  if (label) element.textContent = label;
  if (onClick) element.addEventListener("click", onClick);
  return element;
}

function describe(className, children) {
  const box = document.createElement("div");
  box.className = className;
  box.append(...children);
  return box;
}

/// The last component of a path, whatever the separator, so a read can be named
/// before the other side has been asked anything about it.
function fileLabel(path) {
  return path.split(/[/\\]/).filter(Boolean).pop() ?? path;
}

el("add").addEventListener("click", add);
el("lang").addEventListener("click", toggleLang);
el("tab-document").addEventListener("click", () => showTab("document"));
el("tab-records").addEventListener("click", openRecords);

applyLang();

// Drawn once at the start. A window that cannot say anything at all is the one
// failure worth drawing by hand.
(async () => {
  try {
    draw(await invoke("view"));
  } catch (error) {
    failed("DocFusion", error);
  }
})();
