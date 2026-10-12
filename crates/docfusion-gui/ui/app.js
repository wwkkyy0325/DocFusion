// The window's half of the conversation.
//
// Everything of substance happens on the other side of `invoke`: this file
// draws a view, asks for another one, and does two things the webview is better
// at than Rust — copying to the clipboard and knowing what is selected.

// Everything this file says.
//
// A failure arrives as two halves, and the split is the point. `cause` is the
// kind of failure, which the window owns and so writes its own sentence for.
// `detail` is the account the engine or the operating system gave — somebody
// else's words, in whatever language they came in — and it is shown as it came,
// because rewriting a cause into a sentence of our own is inventing an
// explanation rather than reporting one.
const TEXT = {
    document: "文档",
    records: "记录",
    add: "添加文档",
    to_dark: "切换到暗色",
    to_light: "切换到亮色",

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

    export: "导出",
    export_about: "要拿走哪一样？",
    export_whole: "整份文档",
    export_block: (start) => (start ? `「${start}」这一块` : "这一块"),
    export_dialog: "从 DocFusion 导出",
    block_actions: "导出或复制这一块",
    copy_what: "只有文字，可以直接粘贴",
    format_md: "Markdown",
    format_md_what: "程序写出来的原文",
    format_docx: "Word",
    format_docx_what: "Word 能打开、能编辑的文档",
    format_html: "HTML",
    format_html_what: "浏览器能打开的网页",
    format_txt: "纯文本",
    format_txt_what: "只有文字，不带格式",
    format_xlsx: "Excel",
    format_xlsx_what: "工作簿，一张表一个工作表",
    format_csv: "逗号分隔",
    format_csv_what: "纯文本，什么软件都能打开",

    nothing: "还没有记录。",
    to_review: (n) => `${n} 处待确认`,
    status: { done: "完成", partial: "部分完成", failed: "失败" },

    search_placeholder: "在文档里搜",
    search_none: (query) => `没有文档包含「${query}」。`,
    search_gone: "它的 Markdown 已经不在磁盘上了。",

    view_record: "查看",
    delete_record: "删除",
    ask_title: "这份文档怎么处理？",
    ask_about: (name) => `「${name}」已经在知识库里。`,
    remove_record: "移出列表",
    ask_remove_what:
      "只把它从列表里去掉，Markdown 文件留在原处。把原文档再加进来，它就会回来。",
    ask_delete_what: (path) => `连 ${path} 一起删掉，无法恢复。`,
    ask_delete_nothing: "它的 Markdown 已经不在磁盘上了，没有文件可以删。",
    ask_source: "两种做法都不会动你选的那份原文档：它从来没有被复制进知识库。",
    cancel: "取消",

    dialog_title: "选择文档",

    settings: "设置",
    settings_title: "模型",
    endpoint: "端点地址",
    model_label: "模型",
    key: "密钥",
    save: "保存",
    health_untested: "未测试",
    health_works: "可用",
    health_failed: "不可用",
    health_note_untested: "还没有用这套设置读过任何东西。",
    health_note_works: "上次测试读出了测试图上的数字。",
    health_note_failed: "上次测试没有把数字读回来。",
    changed: "有改动还没保存。",
    saved: "已保存。",
    saving: "保存中…",
    save_failed: "保存失败。",
    fetch_models: "问端点",
    fetching_models: "正在问端点有哪些模型…",
    models_found: (n) => `端点报了 ${n} 个模型，点开就能选。`,
    models_none: "端点答了，但一个模型也没报。",
    models_blind: (n) => `另有 ${n} 个只收文字，没有列出来——它们看不了图片。`,
    none_can_see: "这些模型都不接受图片，没有一个能读扫描件。",
    models_unlisted: "端点能用，但它不提供模型列表，这个只能手填——它自己的文档里会写。",
    models_failed: "连不上端点：",
    endpoint_first: "先填端点地址。",
    model_hint: "不知道填什么？",
    test: "测试",
    testing: "正在让端点读一张测试图…",
    test_read: "数字读出来了，这个端点能用。",
    test_misread: (said, expected) =>
      `端点答了，但答的不是图上写的东西：它说「${said}」，图上写的是 ${expected}。这个模型多半看不见图片。`,
    test_failed: "端点没能回答：",
    show_key: "显示密钥",
    hide_key: "隐藏密钥",
    key_set: "已设置密钥，这个框留空即不改动。",
    key_unset: "还没有密钥。端点不要求的话可以空着。",
    enabled_label: "使用大模型",
    enabled_hint:
      "开着。不管这里怎么选，本机引擎都会读每一页——离线、不用配任何东西。这个开关决定的是：远端的模型是不是也读一遍，以及它读的与本机读的是不是要互相印证。关掉不会丢地址和密钥；清空地址才会连设置一起删掉。",
    reasoning_label: "让模型先想一遍再回答",
    reasoning_hint:
      "关着。同一页实测：开着要 1 分 55 秒，关着 1 分 13 秒，正文一个字都没变——它改的只是标题怎么分级。除非端点会拒绝它不认识的参数，不用打开。",
    settings_about:
      "让远端的模型读扫描件，再由本机内置的识别去印证它读了什么。任何 OpenAI 兼容的服务都行——vLLM、llama.cpp、Ollama，或者在线服务。",
    address_needs_scheme: "地址前面要有 http:// 或 https://。",
    model_needed: "填了端点就要填模型：一个端点上可以有多个模型。",
    endpoint_needed: "填了模型就要填端点，否则连不上。",

    local_label: "本机模型",
    local_name: "本机识别模型",
    local_hint:
      "跑在这台机器上，不是上面的端点：没有地址，没有密钥，也不会把页面发到任何地方。它读出页面上的字，找出页面的版面，再把表格的格子当格子读，而不是当一堆散落的数字。少了它，一整页都是图片的文字就读不出来。",
    local_installed: "已安装",
    local_need: (mb) => `需要下载 ${mb} MB`,
    local_get: "下载",
    local_fetching: "正在下载…",
    local_progress: (received, total, percent) =>
      `${received} MB / ${total} MB · ${percent}%`,
    local_failed: "下载失败：",

    shape_plain: (n) => `只有文字 · ${n} 段`,
    shape_structured: (headings, lists, tables, text) =>
      `${headings} 标题 · ${lists} 列表 · ${tables} 表格 · ${text} 段文字`,
    findings: "待确认的地方",
    findings_note:
      "两个引擎读了同一页，读出来的文字对不上。哪一份对，这个工具不判断——它没有依据判断——所以两份都按各自的原样放在下面，并标出各自把它读成了什么。本机那个引擎会认读页面上的文字，两个模型装好之后，也会把标题和表格一并认出来；读出来的是纯文字，就是它们还没装。",
    page: (n) => `第 ${n} 页`,
    kinds: {
      "the engines read it differently": "两份读法的文字不一样",
      "one engine found text the other did not": "一份读到了另一份没有的文字",
    },
    engine_names: {
      "openai-compatible": "云端模型",
      rapidocr: "本机识别",
      pdfium: "PDF 文字层",
      scan: "扫描件",
      docx: "Word 文档",
      xlsx: "Excel 表格",
      pptx: "PowerPoint 演示",
      "text-passthrough": "纯文本",
      layout: "版面",
      tables: "表格",
    },

    engine_with: (name, parts) => `${name}（${parts.join("、")}）`,

    causes: {
      unsupported: "没有能读这种文件的引擎。",
      "engine-failed": "读这种文件的引擎出错了。",
      unreadable: "文件读不开。",
      unwritable: "结果写不到磁盘上。",
      index: "知识库的索引用不了。",
      missing: "这个文档已经不在知识库里了。",
      settings: "设置文件用不了。",
      "nothing-to-read": "读出来了，但里面是空的。",
      download:
        "本机模型还没装好，或者没下载成功。到「设置 — 本机模型」把它下载下来。",
      delete: "这份文档没能从知识库里拿掉。",
      unknown: "出错了。",
    },
};

// Read defensively rather than destructured outright. If this is ever absent
// the module would throw on its first line and leave a blank window with
// nothing to read on it; this way the failure is drawn like any other.
const invoke = globalThis.__TAURI__?.core?.invoke;

const el = (id) => document.getElementById(id);

let current = null;
let running = false;
let revealed = false;
let ticker = 0;
let started = 0;

/// The button that opened the menu — the footer's, or a block's.
///
/// Kept because what comes of copying is said *on it*, and by then the dialog
/// that did the asking has closed and taken the focus with it. There is one of
/// these at a time because the dialog is modal.
let menuOpener = null;

// A read blocks for as long as it takes, and the window has to keep saying so.
// Long enough that switching to an already-read document does not flash a
// spinner for one frame, short enough that a real read shows one immediately.
const SPINNER_DELAY = 120;

const THEME_KEY = "docfusion.theme";

let theme = startingTheme();

/// Dark, unless somebody has said otherwise.
///
/// Not an empty default waiting to be filled in from the system: dark is the
/// answer, and light is the exception somebody asks for once. Which also means
/// no query to the system, no third state, and no way for the window to open in
/// one theme and change its mind.
function startingTheme() {
  try {
    const chosen = localStorage.getItem(THEME_KEY);
    if (chosen === "light" || chosen === "dark") return chosen;
  } catch {
    // Storage can be unavailable. Not worth failing to open over: the default
    // is still a working answer.
  }

  return "dark";
}

function toggleTheme() {
  theme = theme === "dark" ? "light" : "dark";

  try {
    localStorage.setItem(THEME_KEY, theme);
  } catch {
    // The window still switches, it just does not remember.
  }

  applyTheme();
}

/// The theme is one attribute on the root element: everything else is a colour
/// in the stylesheet that reads it.
///
/// The icon is the one pressing it gives you — a sun while the window is dark.
/// What the button is worth saying is the thing you cannot see, not the thing
/// you can.
function applyTheme() {
  document.documentElement.dataset.theme = theme;

  const going_to_light = theme === "dark";

  // `toggleAttribute`, not `.hidden`, and the difference is not a style
  // preference: `hidden` is an IDL property of HTML elements, and an `<svg>` is
  // not one. Assigning to it there quietly creates a JavaScript property on the
  // object and never writes the attribute, so the stylesheet's `[hidden]` never
  // matches and both shapes stay on screen — while reading `.hidden` back says
  // `true` and reports that all is well.
  el("icon-sun").toggleAttribute("hidden", !going_to_light);
  el("icon-moon").toggleAttribute("hidden", going_to_light);

  // An icon has no words, so the words it would have had go where they are
  // read out instead of looked at: the tooltip, and the name a screen reader
  // announces.
  const label = el("theme");
  label.title = going_to_light ? TEXT.to_light : TEXT.to_dark;
  label.setAttribute("aria-label", label.title);
}

/// The words that live in the markup rather than in the view.
///
/// Run once, at the end of the module. It was a language switch and it is not
/// one any more: there is one language, the markup already carries it, and this
/// only makes sure the two say the same thing.
function writeWords() {
  for (const [id, text] of [
    ["tab-document", TEXT.document],
    ["tab-records", TEXT.records],
    ["tab-settings", TEXT.settings],
    ["add", TEXT.add],
    ["set-title", TEXT.settings_title],
    ["lbl-enabled", TEXT.enabled_label],
    ["set-enabled-hint", TEXT.enabled_hint],
    ["lbl-endpoint", TEXT.endpoint],
    ["lbl-model", TEXT.model_label],
    ["lbl-key", TEXT.key],
    ["lbl-reasoning", TEXT.reasoning_label],
    ["set-reasoning-hint", TEXT.reasoning_hint],
    ["save", TEXT.save],
    ["test", TEXT.test],
    ["set-model-hint", TEXT.model_hint],
    ["fetch-models", TEXT.fetch_models],
    ["set-about", TEXT.settings_about],
    ["lbl-local", TEXT.local_label],
    ["local-name", TEXT.local_name],
    ["local-hint", TEXT.local_hint],
    ["get-local", TEXT.local_get],
    ["ask-title", TEXT.ask_title],
    ["ask-remove-label", TEXT.remove_record],
    ["ask-delete-label", TEXT.delete_record],
    ["ask-source", TEXT.ask_source],
    ["ask-cancel", TEXT.cancel],
    ["ask-remove", TEXT.remove_record],
    ["ask-delete", TEXT.delete_record],
    ["export-cancel", TEXT.cancel],
  ]) {
    el(id).textContent = text;
  }

  el("search").placeholder = TEXT.search_placeholder;

  // These three write their labels themselves.
  drawReveal();
  drawHealth();
  applyTheme();
}

// ── drawing ─────────────────────────────────────────────────────────────────

function draw(view) {
  current = view;

  drawProblem(view.problem);
  drawScreen(view);
  drawRecords(view.records);
  drawSettings(view.model);
  drawLocal(view.local);
  drawStatus(view.screen);
}

/// Fills the settings form from the view.
///
/// The key box is emptied rather than filled. What comes back from the other
/// side is how long the key is, not the key, and that length goes into a
/// placeholder: the dots then match the real key exactly, and a placeholder
/// cannot be submitted — which a field holding a row of dots is one accident
/// away from doing.
///
/// So the box is empty, and empty means "leave the key as it is" rather than
/// "delete it". Deleting it is what clearing the endpoint is for.
function drawSettings(model) {
  el("set-endpoint").value = model.endpoint;
  drawModelField(model);

  const key = el("set-key");
  key.value = "";
  key.type = "password";
  key.placeholder = "•".repeat(model.key_length);

  el("set-hint").textContent = model.key_length > 0 ? TEXT.key_set : TEXT.key_unset;
  el("set-reasoning").checked = model.reasoning;
  el("set-enabled").checked = model.enabled;
  drawEnabled();

  // A fresh view is what is on file, so nothing in the form is a change yet.
  clearFieldErrors();
  drawHealth();

  revealed = false;
  drawReveal();

  // Whatever the last save said is about a form that no longer exists.
  note("");
}

/// The eye, and which of the two shapes is on it.
///
/// It says what pressing it gives you, like the theme button: a plain eye
/// while the key is hidden. The tooltip says the action too,
/// because which of the two means "hidden" is a convention nobody agrees on.
function drawReveal() {
  el("icon-eye").toggleAttribute("hidden", revealed);
  el("icon-eye-off").toggleAttribute("hidden", !revealed);
  el("reveal").setAttribute("aria-pressed", String(revealed));
  el("reveal").title = revealed ? TEXT.hide_key : TEXT.show_key;
}

/// Shows or hides the key, fetching it on the way if it is the one on file.
///
/// The key is not in the view. It is asked for here and only here, because
/// somebody pressed a button that says what it does — rather than travelling
/// with every document the window reads.
async function toggleReveal() {
  const key = el("set-key");

  if (!revealed && key.value === "" && (current?.model?.key_length ?? 0) > 0) {
    try {
      key.value = (await invoke("reveal_key")) ?? "";
    } catch (error) {
      return note(String(error), false);
    }
  }

  revealed = !revealed;
  key.type = revealed ? "text" : "password";
  drawReveal();
  key.focus();
}

/// Something wrong with the installation rather than with a document, said once
/// at the top and left there whatever else is on screen.
function drawProblem(problem) {
  const banner = el("problem");

  banner.hidden = !problem;
  banner.replaceChildren();
  if (!problem) return;

  banner.append(node("strong", "", TEXT.causes[problem.cause] ?? TEXT.causes.unknown));
  if (problem.detail) banner.append(node("span", "detail", problem.detail));
}

/// The two halves of a failure, drawn where they are read.
function drawTrouble(box, trouble) {
  box.append(node("p", "cause", TEXT.causes[trouble.cause] ?? TEXT.causes.unknown));

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
      [TEXT.documents, view.knowledge_base],
      [TEXT.settings, view.config_path],
    ]) {
      where.append(node("dt", "", term), node("dd", "", value));
    }

    box.replaceChildren(
      describe("empty", [
        node("p", "lead", TEXT.empty_lead),
        node("p", "hint", TEXT.empty_hint),
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

    // Drawn by the other half, one element per block and each saying which
    // block it is — see `window_html`. What is added here is the pair of things
    // somebody wants from one block, which is furniture the document does not
    // have and therefore does not come with it.
    for (const block of article.querySelectorAll(".block")) {
      block.append(blockActions(block));
    }

    // The report about the document goes after it, not inside it. In the file
    // they are one thing, because a disagreement has to travel with what it is
    // about; on a screen they are two, because a report on a document sitting
    // in the middle of the document reads as though somebody had written it
    // into the page.
    const panes = [article];
    if (state.report?.length > 0) panes.push(findings(state.report));

    box.replaceChildren(...panes);
  } else {
    const failure = document.createElement("div");
    failure.className = "failure";
    failure.append(node("h2", "", TEXT.could_not_read(state.name)));
    drawTrouble(failure, state.trouble);
    box.replaceChildren(failure);
  }

  box.scrollTop = 0;
}

/// What a reading was made of, said in the words that answer the question.
///
/// "Only text" is not a fault to go hunting for: the engine inside this app is
/// a text detector and a recogniser, and neither of those knows what a table
/// is. But a reader who cannot see the difference between that and a table
/// reads the same paragraph off both and concludes something is broken.
function shapeLine(shapes) {
  if (!shapes) return "";

  const structured = shapes.headings + shapes.lists + shapes.tables;
  return structured === 0
    ? TEXT.shape_plain(shapes.text)
    : TEXT.shape_structured(shapes.headings, shapes.lists, shapes.tables, shapes.text);
}

/// The tool's report on the document: where two engines disagreed, and what
/// each of them read.
///
/// Neither reading is presented as the right one, because the tool has no basis
/// for preferring either — that is why it reports the disagreement instead of
/// settling it. Side by side is the only arrangement that makes the comparison
/// possible, and each reading is capped in height so that a disagreement about
/// a whole page does not become two pages of text on the screen.
function findings(report) {
  const box = document.createElement("section");
  box.className = "findings";
  box.id = "findings";

  box.append(node("h2", "", TEXT.findings));
  box.append(node("p", "note", TEXT.findings_note));

  for (const finding of report) {
    const entry = document.createElement("div");
    entry.className = "finding";
    entry.append(
      node("p", "where", `${TEXT.page(finding.page)} · ${TEXT.kinds[finding.kind] ?? finding.kind}`),
    );

    const readings = document.createElement("div");
    readings.className = "readings";

    for (const reading of finding.readings) {
      const side = document.createElement("div");
      side.className = "reading";

      // What it is called here, and what it is called everywhere else — in the
      // file, in the command line, in the settings — so that a person reading
      // one can find the other.
      const label = node("p", "engine", "");
      label.append(
        document.createTextNode(engineName(reading.engine)),
        node("span", "id", reading.engine),
      );

      // What it read, and what it read it as. The second line is the whole
      // answer to a question the first one cannot answer: two engines can
      // return the same words having read completely different things.
      const shape = shapeLine(reading.shapes);
      if (shape) side.append(node("p", "shape", shape));

      side.append(node("pre", "", reading.text));
      readings.append(side);
    }

    entry.append(readings);
    box.append(entry);
  }

  return box;
}

function drawStatus(state) {
  const status = el("status");
  status.replaceChildren();

  if (state.kind !== "ready") {
    status.hidden = true;
    return;
  }

  status.hidden = false;

  // What each engine is called here, with the name it is called everywhere else
  // kept one hover away. "openai-compatible" is the protocol its client speaks
  // and says nothing to anybody about what it is.
  const named = state.engines.map(engineName);
  const engines = node("span", "", named.join(" · ") || TEXT.read);
  engines.title = state.engines.join(" · ");
  status.append(engines);

  if (state.reused) {
    status.append(node("span", "", TEXT.already));
  }

  // A button, because a count of places to look at that cannot be clicked is a
  // count of places to go hunting for.
  if (state.conflicts > 0) {
    status.append(button(TEXT.disagreed(state.conflicts), "warn link", showFindings));
  }

  status.append(node("span", "spacer", ""));

  // Export rather than Copy, and the difference is what each of them keeps: a
  // copy gives the words, and every block already offers them from where
  // somebody is looking. What is left for the footer is the whole document, and
  // a document is not a thing to paste — it is a thing to keep, which means a
  // file, which means a format.
  status.append(button(TEXT.export, "quiet", (event) => askExport(null, event.currentTarget)));
}

/// Instant rather than smooth. A smooth scroll is a promise to arrive that a
/// person cannot see the end of, and the place it goes is one screen away.
function showFindings() {
  el("findings")?.scrollIntoView({ block: "start" });
}

/// The list of what has been read, newest first.
///
/// A row is not a button. It was one, which made clicking anywhere on it a way
/// of leaving the list — and the two things somebody actually wants from a row
/// here, look at it again and get rid of it, are not the same act and were not
/// worth sharing a target.
///
/// The buttons are drawn rather than hidden until hover, which is the opposite
/// of what the row's own clickability would have wanted. With the row itself
/// inert, a control that appears only under the pointer is a control that
/// cannot be found; the destructive one is quiet enough at rest not to shout.
function drawRecords(list) {
  const records = el("records");
  records.replaceChildren();

  if (!list || list.length === 0) {
    records.append(node("li", "none", TEXT.nothing));
    return;
  }

  for (const record of [...list].reverse()) {
    const item = document.createElement("li");
    item.className = "record";

    // A status this frontend does not know is shown as it arrived rather than
    // hidden: the three it does know are the three the index can hold, and
    // anything else is worth seeing.
    const status = TEXT.status[record.status] ?? record.status;

    // The row shows one line of it, so the rest has to be somewhere: a name cut
    // off with nothing to reveal it is a name that cannot be told from another.
    const name = node("span", "name", record.name);
    name.title = record.name;

    item.append(
      describe("what", [
        name,
        node(
          "span",
          "meta",
          [status, record.conflicts ? TEXT.to_review(record.conflicts) : null, record.created_at]
            .filter(Boolean)
            .join(" · "),
        ),
      ]),
      describe("row-actions", [
        button(TEXT.view_record, "small", () => openRecord(record)),
        button(TEXT.delete_record, "small danger", () => askAbout(record)),
      ]),
    );

    records.append(item);
  }
}

/// Around the matching words in a passage, as the other side sends them.
///
/// Control characters, and they cannot occur in a document — a document is text
/// off a page. A pair of brackets could have been typed by whoever wrote it,
/// and then this would be marking up a match nobody made.
const HIT_START = "\u0002";
const HIT_END = "\u0003";

/// Which search is the current one.
///
/// Typing is faster than searching, and every keystroke starts one. Only the
/// answer to the latest may be drawn: two searches in flight can come back in
/// either order, and the one that arrives last is not the one that was asked
/// for last.
let searching = 0;
let searchTimer = 0;

/// Runs the search in the box, or goes back to the whole list.
///
/// The list is asked for again rather than kept, because it is a file this
/// process is not the only writer of — and because a document can be deleted
/// while the box holds a query, which makes the answer to that query stale in a
/// way the panel would not show.
async function runSearch() {
    const query = el("search").value.trim();
    const mine = ++searching;

    if (!query) {
        try {
            const view = await invoke("records");
            if (mine !== searching) return;

            current = view;
            drawRecords(view.records);
        } catch (error) {
            el("records").replaceChildren(node("li", "none", String(error)));
        }

        return;
    }

    try {
        const matches = await invoke("search", { query });
        if (mine !== searching) return;

        drawMatches(matches, query);
    } catch (error) {
        if (mine !== searching) return;
        el("records").replaceChildren(node("li", "none", String(error)));
    }
}

/// Waits for typing to stop, since every keystroke would otherwise be a search.
const SEARCH_DELAY = 140;

function searchChanged() {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(runSearch, SEARCH_DELAY);
}

/// The answer to a search: which documents hold it, and where.
///
/// The passage is built out of nodes rather than written as markup. It is
/// somebody's document, and the only thing this side knows about it is where
/// the match was — which the other side marked with two characters that cannot
/// appear in a page of text, and which are turned into an element here rather
/// than handed to anything that parses.
function drawMatches(matches, query) {
    const records = el("records");
    records.replaceChildren();

    if (!matches || matches.length === 0) {
        records.append(node("li", "none", TEXT.search_none(query)));
        return;
    }

    for (const match of matches) {
        const item = document.createElement("li");
        item.className = "record";

        const name = node("span", "name", match.name);
        name.title = match.name;

        const what = describe("what", [
            name,
            match.excerpt === null
                ? node("span", "meta", TEXT.search_gone)
                : passage(match.excerpt),
        ]);

        item.append(
            what,
            describe("row-actions", [
                button(TEXT.view_record, "small", () => openMatch(match)),
            ]),
        );

        records.append(item);
    }
}

/// One passage, with the match marked.
function passage(excerpt) {
    const line = node("span", "excerpt");

    // Everything before the first match, then each match and the text after it.
    const parts = excerpt.split(HIT_START);
    line.append(document.createTextNode(parts.shift()));

    for (const part of parts) {
        const at = part.indexOf(HIT_END);
        // A marker without its pair would be a bug on the other side, and a
        // pass through the whole of the rest is a better answer to it than a
        // thrown exception and a blank panel.
        if (at < 0) {
            line.append(document.createTextNode(part));
            continue;
        }

        line.append(node("mark", "", part.slice(0, at)));
        line.append(document.createTextNode(part.slice(at + HIT_END.length)));
    }

    return line;
}

/// Opens a document a search found.
async function openMatch(match) {
    await read(() => invoke("open", { id: match.id, name: match.name }), match.name);
}

/// The record a question is being asked about, or `null`.
///
/// Deleting a file cannot be undone and taking a record out of the list can,
/// and they are two different sentences. So one button asks which was meant
/// rather than guessing between them: a button that guessed would be a button
/// that sometimes deleted a document.
let asking = null;

/// Opens the question, filled in for this record.
function askAbout(record) {
  asking = record;

  el("ask-about").textContent = TEXT.ask_about(record.name);
  el("ask-remove-what").textContent = TEXT.ask_remove_what;

  // Said in the row rather than left to a button that does nothing when
  // pressed, which is a thing people press twice.
  el("ask-delete-what").textContent = record.on_disk
    ? TEXT.ask_delete_what(record.output)
    : TEXT.ask_delete_nothing;
  el("ask-delete").disabled = !record.on_disk;

  el("record-question").showModal();
}

/// Does whichever of the two was asked for.
///
/// The command takes the id rather than the row's position, and the question is
/// not asked again on the other side: a command that popped its own
/// confirmation could not be driven by anything but a person.
async function answer(to) {
  const record = asking;
  if (!record) return;

  el("record-question").close();
  asking = null;

  try {
    const view = await invoke(to, { id: record.id });
    const was_showing = current?.screen?.kind === "ready" && current.screen.id === record.id;

    current = view;
    drawProblem(view.problem);
    drawRecords(view.records);

    // The pane only when what it was showing has just gone, which the other
    // side has already decided: redrawing it otherwise scrolls a long document
    // back to the top, and looking at the list was not meant to cost that.
    if (was_showing) {
      drawScreen(view);
      drawStatus(view.screen);
    }
  } catch (error) {
    failed(record.name, error);
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

  const clock = node("span", "elapsed", TEXT.seconds(0));
  clock.id = "elapsed";
  const progress = node("p", "progress", "");
  progress.append(clock, document.createTextNode(` · ${TEXT.scan_note}`));

  box.append(node("div", "pulse", ""), node("p", "", TEXT.reading(hint.name)), progress);
  return box;
}

// ── asking ──────────────────────────────────────────────────────────────────

async function add() {
  if (running) return;

  let path;
  try {
    // The dialog is the operating system's, but its title is ours and is
    // passed as words like everything else here.
    path = await invoke("choose_file", { title: TEXT.dialog_title });
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
    if (clock) clock.textContent = TEXT.seconds(Math.round((Date.now() - started) / 1000));
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

// ── taking a document, or a piece of one, out ───────────────────────────────

/// The one thing a block offers at its right edge.
///
/// **One glyph, not two buttons.** A pair has to be drawn beside the text to
/// stay off it, and a pair is more than twice as wide as a glyph and tall
/// enough to hang past a one-line paragraph — into the margin the next block's
/// pair is also hanging in, where the two overlap. One glyph is a row of its
/// own, and the two things it does are a list in the dialog.
function blockActions(block) {
  const where = Number(block.dataset.block);

  const more = button("⋯", "small", (event) => askExport(where, event.currentTarget));
  more.title = TEXT.block_actions;

  const actions = node("div", "block-actions");
  actions.append(more);

  return actions;
}

/// Says a block was copied, on the button that asked for the menu.
///
/// A tick rather than the word, because the button has no room for one: it is a
/// single glyph standing beside the text, and "Copied" written in its place
/// would push it into the paragraph it is meant to stay out of. The tick is the
/// same width as the dots it replaces, and green is the one colour in this
/// window that means a thing worked.
function tick(element) {
  if (!element) return;

  const was = element.textContent;
  element.classList.add("done");
  element.textContent = "✓";

  setTimeout(() => {
    element.classList.remove("done");
    element.textContent = was;
  }, 1200);
}

/// Asks what to take out of a block, or out of the whole document.
///
/// `where` is the number the pane put on a block, or `null` for the whole
/// document, and `opener` is the button that asked — what comes of it is said
/// on that button, because that is where whoever asked is looking.
///
/// What is offered is what the other half will write. `export::suits` is the
/// rule and it is one rule — a table can be a spreadsheet and nothing else can
/// — repeated here because this is where the list is drawn, and read off the
/// marker the other half put on the block rather than off a `<table>`, so that
/// the two are one fact rather than two that agree.
///
/// The other rule is about Copy rather than about a format: the words with no
/// structure are what a copy gives, so a copy is offered for a block and not
/// for a whole document — a document is not a thing to paste.
function askExport(where, opener) {
  const article = el("screen").querySelector(".document");
  if (!article) return;

  const block = where === null ? null : article.querySelector(`.block[data-block="${where}"]`);
  if (where !== null && !block) return;

  const inside = block ?? article;
  menuOpener = opener;

  // What it is about, as the heading: the backdrop covers the document, so the
  // question has to say which block it means, and a block's own words say it
  // better than its kind would — a document can hold three tables and only one
  // of them is this one.
  el("export-title").textContent =
    block === null
      ? TEXT.export_whole
      : TEXT.export_block(headline(block.querySelector(".block-body").innerText));
  el("export-about").textContent = TEXT.export_about;

  const lines = [];

  // Copy first, because it is the plainest thing to want from a block and the
  // only line that keeps nothing — which is the whole difference between it and
  // everything under it.
  if (block !== null) {
    lines.push(choice(TEXT.copy, TEXT.copy_what, () => copyBlock(inside)));
  }

  const formats = block === null ? ["md", "docx", "html", "txt"] : ["md", "docx", "html"];
  if (hasTable(inside)) formats.push("xlsx", "csv");

  for (const format of formats) {
    lines.push(choice(...formatWords(format), () => runExport(format, where)));
  }

  el("export-formats").replaceChildren(...lines);
  el("export-question").showModal();
}

/// One line of the dialog: what it is called, and what it keeps.
///
/// Every line does its thing and closes the dialog, because the choice *is* the
/// answer — a list of answers with a confirm under it asks the same question
/// twice.
function choice(name, keeps, onChoose) {
  const line = document.createElement("button");
  line.type = "button";
  line.append(node("span", "name", name), node("span", "what", keeps));

  // Closed before the work, so that the file dialog the other half opens is not
  // underneath this one.
  line.addEventListener("click", () => {
    el("export-question").close();
    onChoose();
  });

  return line;
}

/// What each format is called, and what it keeps.
///
/// The second half is the whole of the choice: somebody standing here is not
/// choosing between four file extensions, they are choosing between a table a
/// spreadsheet can open and a paragraph with its emphasis still on it.
function formatWords(format) {
  return {
    md: [TEXT.format_md, TEXT.format_md_what],
    docx: [TEXT.format_docx, TEXT.format_docx_what],
    html: [TEXT.format_html, TEXT.format_html_what],
    txt: [TEXT.format_txt, TEXT.format_txt_what],
    xlsx: [TEXT.format_xlsx, TEXT.format_xlsx_what],
    csv: [TEXT.format_csv, TEXT.format_csv_what],
  }[format];
}

/// The words of one block, with no structure.
///
/// The body rather than the block: the block holds the button that was pressed
/// as well, and copying the block would paste the dots.
async function copyBlock(block) {
  try {
    await navigator.clipboard.writeText(block.querySelector(".block-body").innerText);
    tick(menuOpener);
  } catch (error) {
    // Nothing on the block to say this with — the button that asked is one
    // glyph wide — and it is not a fault of the document, which is what the
    // banner at the top is for.
    drawProblem({ cause: "unknown", detail: String(error) });
  }
}

/// Whether this is a table, or holds one.
function hasTable(node) {
  return node.matches(".block-table") || node.querySelector(".block-table") !== null;
}

/// The first few words of a block, for the question to name it by.
const HEADLINE = 24;

function headline(text) {
  const line = text.replace(/\s+/g, " ").trim();
  return line.length > HEADLINE ? `${line.slice(0, HEADLINE)}…` : line;
}

/// Writes it out, having asked the operating system where to put it.
///
/// Both the asking and the writing happen on the other side — this half has no
/// filesystem. What is left here is what the window says about it, and a
/// dismissed save dialog is not a failure and says nothing.
async function runExport(format, where) {
  const screen = current?.screen;
  if (screen?.kind !== "ready") return;

  try {
    // A file that was written says nothing, and that is the whole of what this
    // window has to say about it: the save dialog closing *is* the answer, and
    // it is the operating system's. What is left to report is a failure.
    await invoke("export", {
      id: screen.id,
      title: TEXT.export_dialog,
      format,
      block: where,
    });
  } catch (trouble) {
    // Two halves, like every other failure here: the kind, which this side has
    // a sentence for, and the account the other side gave.
    drawProblem(trouble);
  }
}

// ── tabs ────────────────────────────────────────────────────────────────────

function showTab(which) {
  for (const name of ["document", "records", "settings"]) {
    el(`tab-${name}`).setAttribute("aria-selected", String(name === which));
    el(`panel-${name}`).hidden = name !== which;
  }
}

/// Writes the settings, having first refused the combinations that read
/// nothing.
///
/// Refused here rather than in the other half because this is a form, and a
/// form is the one place the person is looking when they get it wrong. The
/// other half refuses a half-written pair as well, but its answer is a string
/// of its own rather than a field marked in red.
/// Returns whether it got as far as writing, so that a caller which needs the
/// settings to be on file — the test does — can stop when they are not.
async function saveSettings(event) {
  event?.preventDefault();

  const endpoint = el("set-endpoint").value.trim();
  const model = chosenModel();

  clearFieldErrors();

  if (endpoint && !/^https?:\/\//i.test(endpoint)) {
    fieldError("set-endpoint", TEXT.address_needs_scheme);
    return false;
  }
  if (endpoint && !model) {
    fieldError("set-model", TEXT.model_needed);
    return false;
  }
  if (model && !endpoint) {
    fieldError("set-endpoint", TEXT.endpoint_needed);
    return false;
  }

  note(TEXT.saving, true);

  try {
    draw(
      await invoke("save_model", {
        endpoint,
        model,
        key: el("set-key").value,
        reasoning: el("set-reasoning").checked,
        enabled: el("set-enabled").checked,
      }),
    );
    note(TEXT.saved, true);
    return true;
  } catch (error) {
    // The other half had the last word, and it is the half that owns the file.
    note(`${TEXT.save_failed} ${error}`, false);
    return false;
  }
}

/// Greys out everything the switch is about.
///
/// Disabled rather than hidden, so that what is switched off can still be read
/// — an address somebody half typed is still theirs, and hiding it is how it
/// gets typed twice.
function drawEnabled() {
  const on = el("set-enabled").checked;

  for (const node of el("model-fields").querySelectorAll("input, select, button")) {
    node.disabled = !on;
  }

  // The one button under the switch that still works when the model is off.
  el("save").disabled = false;
  el("model-fields").dataset.off = on ? "no" : "yes";
}

/// The set that runs in this process, and how much of it is here.
///
/// Read from the view rather than kept here, like everything else, and the
/// other side throws its own copy away when a fetch finishes — so this is what
/// turns a finished download from "still offered" into "installed".
function drawLocal(local) {
  const here = local ?? {};
  const total = here.total_bytes ?? 0;

  el("local-size").textContent = `${megabytes(total)} MB`;

  // One of three faces, and the button belongs to two of them. The bar belongs
  // to the fetch and to nothing else: a bar that is always there and always
  // full is a bar nobody reads.
  const running = Boolean(here.fetching);
  const installed = Boolean(here.ready) && !running;

  el("local-card").dataset.state = installed ? "ready" : running ? "busy" : "absent";
  el("local-bar").hidden = !running;
  el("get-local").hidden = running || installed;

  if (installed) return void (el("local-state").textContent = TEXT.local_installed);
  if (!running) {
    return void (el("local-state").textContent = TEXT.local_need(
      megabytes(here.needed_bytes ?? 0),
    ));
  }

  // What is already here, rather than zero. The fetch resumes rather than
  // restarts, so a bar that began at nothing would be telling somebody their
  // half-downloaded set was gone.
  el("local-fill").style.width = `${percent({
    received: total - (here.needed_bytes ?? 0),
    total,
  })}%`;

  el("local-state").textContent = TEXT.local_fetching;
}

/// A fetch saying how far it has got.
///
/// The count goes on the same line the fetch ends on, so there is one sentence
/// about the state rather than two that can disagree.
function drawLocalProgress(progress) {
  if (!progress?.total) return;

  el("local-fill").style.width = `${percent(progress)}%`;
  el("local-state").textContent = TEXT.local_progress(
    megabytes(progress.received),
    megabytes(progress.total),
    percent(progress),
  );
}

/// How far along a fetch is, as a whole number of percent.
const percent = ({ received = 0, total = 0 } = {}) =>
  total > 0 ? Math.round((received / total) * 100) : 0;

/// What an engine is called here.
///
/// **Assembled names are taken apart rather than listed.** An engine gives
/// itself a name built out of the parts it had — `rapidocr+layout+tables` is
/// the recognition engine, reading with a layout model and a table model — so
/// the name is read back the same way it was built. Listing every combination
/// instead is what was here before, and it went wrong the moment somebody
/// installed all three models: the list held the base names, the lookup found
/// nothing, and the machine's own name for it was drawn in the status bar, in
/// Chinese, to somebody who could not read it.
///
/// The convention is the one both places that build a name follow: the first
/// part is what read the page, and the rest are what it had to read with.
///
/// A part nobody has words for is shown as itself, which is smaller than the
/// whole name and is the most a program can do about a name it was not told
/// about — and the test that watches this list is what stops it happening.
function engineName(id) {
  const [reader, ...using] = String(id).split("+");
  const called = (part) => TEXT.engine_names[part] ?? part;
  const name = called(reader);

  return using.length === 0 ? name : TEXT.engine_with(name, using.map(called));
}

/// Whole megabytes, rounded up.
///
/// The number is a warning about a download, so rounding to nothing and then
/// saying "0 MB" would be the one answer that is worse than the estimate.
const megabytes = (bytes) => Math.max(1, Math.ceil(bytes / (1024 * 1024)));

/// Fetches the set of models this app reads a page with.
async function getLocalModels() {
  // **The card takes its running face here, not when the other side answers** —
  // and the other side does not answer until the whole download is over. What
  // was drawn before this line existed: the button that had just been pressed
  // stayed on screen for the entire fetch, still saying Download, and **the
  // Stop beside it was never shown at all**, so the one control that can end a
  // download was unreachable for exactly as long as there was a download to
  // end. Progress kept arriving the whole time; nothing was listening to it
  // that could change a button.
  //
  // The numbers are the last the view had, which is what is still missing — a
  // fetch resumes rather than restarts, so the bar starts where it left off.
  drawLocal({ ...(current?.local ?? {}), fetching: true });

  try {
    // The answer is drawn the way the settings tab draws everything else: only
    // the parts that changed, so a fetch finishing does not scroll the document
    // on the other tab back to the top.
    const view = await invoke("fetch_local_models");
    current = view;
    drawProblem(view.problem);
    drawLocal(view.local);

  } catch (error) {
    drawLocal(current?.local);
    el("local-state").textContent = `${TEXT.local_failed} ${error}`;
  }
}

/// Asks the endpoint to read a page it has never seen.
///
/// Saves first, because the test uses what is on file — the address, the model
/// and the key the window is reading with are the ones the file holds, and a
/// test that quietly used what was typed would be answering about a different
/// setup from the one that will run.
async function testModel() {
  const button = el("test");
  button.disabled = true;
  note(TEXT.testing, true);

  try {
    // The message is already on screen when it does not save.
    if (!(await saveSettings())) return;
    showTest(await invoke("test_model"));
  } catch (error) {
    note(`${TEXT.test_failed} ${error}`, false);
  } finally {
    button.disabled = false;
  }
}

/// Three outcomes, said apart.
///
/// "It answered and the answer was wrong" is the one worth telling apart from a
/// plain failure and the one nothing but a picture can find: a model served
/// without its vision half answers every request in fluent text about nothing.
function showTest(result) {
  // Read, misread and failed all leave the badge, because all three are the
  // answer to "does this work" and only the first is a yes. A model that
  // answers something other than what it was shown is a model that cannot see,
  // which is not a working configuration however healthy the connection is.
  health = result.outcome === "read" ? "works" : "failed";
  drawHealth();

  if (result.outcome === "read") return note(TEXT.test_read, true);
  if (result.outcome === "misread") return note(TEXT.test_misread(result.said, result.expected), false);

  note(`${TEXT.test_failed} ${result.detail}`, false);
}

/// Asks the endpoint which models it serves, and puts them on the field.
///
/// Uses what is in the form rather than what is on file, because this is asked
/// while somebody is filling the form in — often before anything has been
/// saved, and often about an address they have just changed.
async function fetchModels() {
  const endpoint = el("set-endpoint").value.trim();

  if (!endpoint) return note(TEXT.endpoint_first, false);

  const button = el("fetch-models");
  button.disabled = true;
  note(TEXT.fetching_models, true);

  try {
    const remote = await invoke("probe_models", {
      endpoint,
      // Empty means the key already on file. The window does not hold it.
      key: el("set-key").value,
    });

    if (remote.outcome === "models") {
      offered = remote.names;
      drawModelField({ model: chosenModel() });

      // Nothing to offer is not the same as nothing reported: an endpoint whose
      // every model is text-only has answered, and answered that it cannot do
      // this job.
      const usable = offered.length > 0;
      const said = usable
        ? TEXT.models_found(offered.length)
        : remote.blind > 0
          ? TEXT.none_can_see
          : TEXT.models_none;

      note(usable && remote.blind > 0 ? `${said} ${TEXT.models_blind(remote.blind)}` : said, usable);
      return;
    }

    const said = remote.outcome === "unlisted" ? TEXT.models_unlisted : TEXT.models_failed;
    note(`${said} ${remote.detail}`, false);
  } catch (error) {
    note(String(error), false);
  } finally {
    button.disabled = false;
  }
}

// ── the model list ────────────────────────────────────────────────────

/// What the endpoint said it serves, or nothing if it was never asked.
let offered = [];

/// Whether these settings are known to work.
///
/// `untested`, `works`, or `failed`. Kept apart from whether they are *saved*,
/// because saving and working are different questions and the second is the one
/// somebody setting this up has. Nothing sets it to `works` except a model
/// reading the number off a test page.
///
/// It goes back to `untested` the moment anything it was about changes. A tick
/// that survived an edit would be a tick about the settings that used to be
/// there.
let health = "untested";

function drawHealth() {
  const words = {
    untested: TEXT.health_untested,
    works: TEXT.health_works,
    failed: TEXT.health_failed,
  };

  const badge = el("health");
  badge.dataset.state = health;
  badge.textContent = words[health];
  badge.title = TEXT[`health_note_${health}`];
}

/// A change to anything the test was about makes the answer about something
/// else.
function forgetHealth() {
  if (health !== "untested") {
    health = "untested";
    drawHealth();
  }

  // And the settings are no longer what is on file.
  if (dirty()) note(TEXT.changed, false);
}

/// Whether the form differs from what is on file.
function dirty() {
  const saved = current?.model;
  if (!saved) return false;

  return (
    el("set-endpoint").value.trim() !== saved.endpoint ||
    chosenModel() !== saved.model ||
    el("set-key").value !== ""
  );
}

/// A message under the field it is about, and the field marked.
function fieldError(id, message) {
  const field = el(id);
  const error = el(`${id}-error`);

  error.textContent = message ?? "";
  error.hidden = !message;

  if (message) field.setAttribute("aria-invalid", "true");
  else field.removeAttribute("aria-invalid");
}

function clearFieldErrors() {
  fieldError("set-endpoint", null);
  fieldError("set-model", null);
}

/// Shows the list when there is one to choose from, and the text field when
/// there is not.
///
/// Two controls rather than one that is both, because the two cases are not the
/// same job. Picking one of a list the endpoint just gave us is what a `select`
/// is for, and the browser has been getting that right for thirty years —
/// keyboard, screen readers, type-ahead, a popup that knows which way the
/// window opens. Naming a model nobody listed is a text field, because there is
/// nothing to pick from.
///
/// There was a made-up one here before, on the theory that a `datalist` looks
/// nothing like the rest of the page. It was replaced with a list of this
/// program's own, which rendered correctly in every check and was, on somebody
/// else's machine, dead to the pointer. Neither of those is a thing to fix
/// twice.
function drawModelField(model) {
  const field = el("set-model");
  const choice = el("set-model-choice");

  // The model that is set stays choosable even when the endpoint does not list
  // it — because it is blind, or because it was named before the endpoint was
  // asked. Dropping it from the list would silently change the setting.
  const names = offered.length === 0 || offered.includes(model.model) || model.model === ""
    ? offered
    : [model.model, ...offered];

  if (names.length === 0) {
    field.hidden = false;
    field.value = model.model;
    choice.hidden = true;
    el("model-mark").hidden = true;
    return;
  }

  choice.replaceChildren(
    ...names.map((name) => {
      const option = document.createElement("option");
      option.value = name;
      option.textContent = name;
      return option;
    }),
  );
  choice.value = model.model;

  field.hidden = true;
  choice.hidden = false;
  el("model-mark").hidden = false;
}

/// The model as it stands, from whichever of the two controls is showing.
function chosenModel() {
  return el("set-model-choice").hidden
    ? el("set-model").value.trim()
    : el("set-model-choice").value;
}


function note(text, ok = true) {
  const element = el("set-note");

  element.textContent = text;
  element.className = `hint ${ok ? "ok" : "bad"}`;
}

/// The settings are a file this process is not the only writer of, so they are
/// re-read when they are asked for rather than remembered — opening this tab
/// after editing the file by hand shows what the file now says.
///
/// Like the records, only this panel is redrawn: the document on the other tab
/// keeps its place.
async function openSettings() {
  showTab("settings");
  try {
    const view = await invoke("view");
    current = view;
    drawSettings(view.model);
  } catch (error) {
    note(String(error), false);
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
    // The box is the state the list is in, so it is left as it was: switching
    // to the tab is not asking for the whole list, and a search that emptied
    // the box would be a search thrown away by looking at another tab.
    if (!el("search").value.trim()) drawRecords(view.records);
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

// A download reports as it goes. Waiting until it is over to say anything would
// leave a quarter of a gigabyte as one silent minute after another, which is
// indistinguishable from a download that has stopped.
if (globalThis.__TAURI__?.event?.listen) {
  globalThis.__TAURI__.event.listen("local-models", (event) => drawLocalProgress(event.payload));
}

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
el("theme").addEventListener("click", toggleTheme);
el("tab-document").addEventListener("click", () => showTab("document"));
el("tab-records").addEventListener("click", openRecords);
el("tab-settings").addEventListener("click", openSettings);
el("settings").addEventListener("submit", saveSettings);
el("reveal").addEventListener("click", toggleReveal);
el("test").addEventListener("click", testModel);
el("fetch-models").addEventListener("click", fetchModels);
el("set-enabled").addEventListener("change", drawEnabled);
el("get-local").addEventListener("click", getLocalModels);

el("search").addEventListener("input", searchChanged);

el("ask-cancel").addEventListener("click", () => el("record-question").close());
el("ask-remove").addEventListener("click", () => answer("forget_record"));
el("ask-delete").addEventListener("click", () => answer("delete_record"));
el("export-cancel").addEventListener("click", () => el("export-question").close());

// Escape and a press outside both close a modal `dialog` without running any
// of the code above, so the row the question was about is dropped here instead
// — otherwise the next Delete would act on whatever was asked about last.
el("record-question").addEventListener("close", () => (asking = null));

// Anything the answer was about: changing it makes the answer about something
// that is no longer on screen.
for (const id of ["set-endpoint", "set-model", "set-model-choice", "set-key", "set-enabled"]) {
  el(id).addEventListener("input", forgetHealth);
  el(id).addEventListener("change", forgetHealth);
}


writeWords();

// Drawn once at the start. A window that cannot say anything at all is the one
// failure worth drawing by hand.
(async () => {
  try {
    draw(await invoke("view"));
  } catch (error) {
    failed("DocFusion", error);
  }
})();
