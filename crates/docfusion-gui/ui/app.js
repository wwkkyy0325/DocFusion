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
    to_dark: "Switch to the dark theme",
    to_light: "Switch to the light theme",

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

    settings: "Settings",
    settings_title: "Model",
    endpoint: "Endpoint",
    model_label: "Model",
    key: "Key",
    save: "Save",
    health_untested: "Not tested",
    health_works: "Working",
    health_failed: "Not working",
    health_note_untested: "Nothing has been read with these settings yet.",
    health_note_works: "The last test read the number off the test page.",
    health_note_failed: "The last test did not get the number back.",
    changed: "Changed since it was saved.",
    saved: "Saved.",
    saving: "Saving…",
    save_failed: "Could not save.",
    fetch_models: "Ask the endpoint",
    fetching_models: "Asking the endpoint which models it serves…",
    models_found: (n) => `The endpoint lists ${n}. Open the field to pick one.`,
    models_none: "The endpoint answered, and lists no models.",
    models_blind: (n) =>
      `${n} of them accept text only and are not listed — they cannot be shown a page.`,
    none_can_see: "None of these models accept images, so none of them can read a scan.",
    models_unlisted:
      "The endpoint works but does not list its models, so this one has to be typed. Its own documentation will name it.",
    models_failed: "Could not reach the endpoint:",
    endpoint_first: "Fill in the endpoint first.",
    model_hint: "Not sure what goes here?",
    test: "Test",
    testing: "Asking the endpoint to read a test page…",
    test_read: "It read the number. This endpoint works.",
    test_misread: (said, expected) =>
      `It answered, but not with what it was shown: it said “${said}” and the page says ${expected}. The model probably cannot see images.`,
    test_failed: "The endpoint did not answer:",
    show_key: "Show the key",
    hide_key: "Hide the key",
    key_set: "A key is set. Leave this box empty to keep it.",
    key_unset: "No key is set. One is only needed if the endpoint asks for it.",
    reasoning_label: "Let the model think before it answers",
    reasoning_hint:
      "Off. Measured on one page: 1m55s with it, 1m13s without, and not one word of the body different — all it changed was how the headings were classified. Turn it on only if your endpoint refuses a request it does not recognise.",
    settings_about:
      "Let a model on the other end read scanned pages, and have the engine inside this app check what it read. Clearing the endpoint turns it off and calls nothing.",
    address_needs_scheme: "The address needs http:// or https:// in front of it.",
    model_needed: "An endpoint needs a model: one endpoint can serve several.",
    endpoint_needed: "A model needs an endpoint to be reached through.",

    shape_plain: (n) => `Text only · ${n} passages`,
    shape_structured: (headings, lists, tables, text) =>
      `${headings} headings · ${lists} lists · ${tables} tables · ${text} passages`,
    findings: "Places to check",
    findings_note:
      "Both engines read the same page and their text does not match. Neither reading is marked as the right one — that is not a judgement this tool can make — so both are here, as each engine wrote them, with what each of them read it as. The engine inside this app detects text and then recognises it; neither of those knows what a table is, so its readings are always text and nothing else.",
    page: (n) => `Page ${n}`,
    kinds: {
      "the engines read it differently": "the two readings do not match",
      "one engine found text the other did not": "one engine found text the other did not",
    },
    engine_names: {
      "openai-compatible": "the configured endpoint",
      rapidocr: "the engine inside this app",
    },

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

    nothing: "还没有记录。",
    to_review: (n) => `${n} 处待确认`,
    status: { done: "完成", partial: "部分完成", failed: "失败" },

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
    reasoning_label: "让模型先想一遍再回答",
    reasoning_hint:
      "关着。同一页实测：开着要 1 分 55 秒，关着 1 分 13 秒，正文一个字都没变——它改的只是标题怎么分级。除非端点会拒绝它不认识的参数，不用打开。",
    settings_about:
      "让远端的模型读扫描件，再由本机内置的识别去印证它读了什么。清空端点地址就关掉它，不调用任何模型。",
    address_needs_scheme: "地址前面要有 http:// 或 https://。",
    model_needed: "填了端点就要填模型：一个端点上可以有多个模型。",
    endpoint_needed: "填了模型就要填端点，否则连不上。",

    shape_plain: (n) => `只有文字 · ${n} 段`,
    shape_structured: (headings, lists, tables, text) =>
      `${headings} 标题 · ${lists} 列表 · ${tables} 表格 · ${text} 段文字`,
    findings: "待确认的地方",
    findings_note:
      "两个引擎读了同一页，读出来的文字对不上。哪一份对，这个工具不判断——它没有依据判断——所以两份都按各自的原样放在下面，并标出各自把它读成了什么。本机那个引擎是先检测文字再认读它，两者都不知道表格是什么，所以它的读法永远是纯文字。",
    page: (n) => `第 ${n} 页`,
    kinds: {
      "the engines read it differently": "两份读法的文字不一样",
      "one engine found text the other did not": "一份读到了另一份没有的文字",
    },
    engine_names: {
      "openai-compatible": "云端模型",
      rapidocr: "本机识别",
    },

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
let revealed = false;
let ticker = 0;
let started = 0;

// A read blocks for as long as it takes, and the window has to keep saying so.
// Long enough that switching to an already-read document does not flash a
// spinner for one frame, short enough that a real read shows one immediately.
const SPINNER_DELAY = 120;

const LANG_KEY = "docfusion.lang";
const THEME_KEY = "docfusion.theme";

let lang = startingLang();
let theme = startingTheme();

const t = () => TEXT[lang];

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
    // Storage can be unavailable. Same as the language: the default is still a
    // working answer.
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
/// The icon is the one pressing it gives you — a sun while the window is dark —
/// which is what the language button does with its label too. What the button
/// is worth saying is the thing you cannot see, not the thing you can.
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
  label.title = going_to_light ? t().to_light : t().to_dark;
  label.setAttribute("aria-label", label.title);
}

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

  for (const [id, text] of [
    ["tab-document", words.document],
    ["tab-records", words.records],
    ["tab-settings", words.settings],
    ["add", words.add],
    ["lang", words.switch_lang],
    ["set-title", words.settings_title],
    ["lbl-endpoint", words.endpoint],
    ["lbl-model", words.model_label],
    ["lbl-key", words.key],
    ["lbl-reasoning", words.reasoning_label],
    ["set-reasoning-hint", words.reasoning_hint],
    ["save", words.save],
    ["test", words.test],
    ["set-model-hint", words.model_hint],
    ["fetch-models", words.fetch_models],
    ["set-about", words.settings_about],
  ]) {
    el(id).textContent = text;
  }

  el("lang").title = words.switch_title;

  // Both of these write words of their own, and both were written in the
  // language that was just replaced.
  drawReveal();
  drawHealth();

  // Last, because it writes a label of its own and that label is a word in the
  // language that has just been chosen.
  applyTheme();
}

// ── drawing ─────────────────────────────────────────────────────────────────

function draw(view) {
  current = view;

  drawProblem(view.problem);
  drawScreen(view);
  drawRecords(view.records);
  drawSettings(view.model);
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

  el("set-hint").textContent = model.key_length > 0 ? t().key_set : t().key_unset;
  el("set-reasoning").checked = model.reasoning;

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
/// It says what pressing it gives you, like the theme and the language buttons:
/// a plain eye while the key is hidden. The tooltip says the action too,
/// because which of the two means "hidden" is a convention nobody agrees on.
function drawReveal() {
  el("icon-eye").toggleAttribute("hidden", revealed);
  el("icon-eye-off").toggleAttribute("hidden", !revealed);
  el("reveal").setAttribute("aria-pressed", String(revealed));
  el("reveal").title = revealed ? t().hide_key : t().show_key;
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
    failure.append(node("h2", "", t().could_not_read(state.name)));
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
    ? t().shape_plain(shapes.text)
    : t().shape_structured(shapes.headings, shapes.lists, shapes.tables, shapes.text);
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

  box.append(node("h2", "", t().findings));
  box.append(node("p", "note", t().findings_note));

  for (const finding of report) {
    const entry = document.createElement("div");
    entry.className = "finding";
    entry.append(
      node("p", "where", `${t().page(finding.page)} · ${t().kinds[finding.kind] ?? finding.kind}`),
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
        document.createTextNode(t().engine_names[reading.engine] ?? reading.engine),
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
  const named = state.engines.map((id) => t().engine_names[id] ?? id);
  const engines = node("span", "", named.join(" · ") || t().read);
  engines.title = state.engines.join(" · ");
  status.append(engines);

  if (state.reused) {
    status.append(node("span", "", t().already));
  }

  // A button, because a count of places to look at that cannot be clicked is a
  // count of places to go hunting for.
  if (state.conflicts > 0) {
    status.append(button(t().disagreed(state.conflicts), "warn link", showFindings));
  }

  status.append(node("span", "spacer", ""));
  status.append(button(t().copy, "quiet", copyAll));
}

/// Instant rather than smooth. A smooth scroll is a promise to arrive that a
/// person cannot see the end of, and the place it goes is one screen away.
function showFindings() {
  el("findings")?.scrollIntoView({ block: "start" });
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
/// in a language this side cannot choose.
/// Returns whether it got as far as writing, so that a caller which needs the
/// settings to be on file — the test does — can stop when they are not.
async function saveSettings(event) {
  event?.preventDefault();

  const endpoint = el("set-endpoint").value.trim();
  const model = chosenModel();

  clearFieldErrors();

  if (endpoint && !/^https?:\/\//i.test(endpoint)) {
    fieldError("set-endpoint", t().address_needs_scheme);
    return false;
  }
  if (endpoint && !model) {
    fieldError("set-model", t().model_needed);
    return false;
  }
  if (model && !endpoint) {
    fieldError("set-endpoint", t().endpoint_needed);
    return false;
  }

  note(t().saving, true);

  try {
    draw(
      await invoke("save_model", {
        endpoint,
        model,
        key: el("set-key").value,
        reasoning: el("set-reasoning").checked,
      }),
    );
    note(t().saved, true);
    return true;
  } catch (error) {
    // The other half had the last word, and it is the half that owns the file.
    note(`${t().save_failed} ${error}`, false);
    return false;
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
  note(t().testing, true);

  try {
    // The message is already on screen when it does not save.
    if (!(await saveSettings())) return;
    showTest(await invoke("test_model"));
  } catch (error) {
    note(`${t().test_failed} ${error}`, false);
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

  if (result.outcome === "read") return note(t().test_read, true);
  if (result.outcome === "misread") return note(t().test_misread(result.said, result.expected), false);

  note(`${t().test_failed} ${result.detail}`, false);
}

/// Asks the endpoint which models it serves, and puts them on the field.
///
/// Uses what is in the form rather than what is on file, because this is asked
/// while somebody is filling the form in — often before anything has been
/// saved, and often about an address they have just changed.
async function fetchModels() {
  const endpoint = el("set-endpoint").value.trim();

  if (!endpoint) return note(t().endpoint_first, false);

  const button = el("fetch-models");
  button.disabled = true;
  note(t().fetching_models, true);

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
        ? t().models_found(offered.length)
        : remote.blind > 0
          ? t().none_can_see
          : t().models_none;

      note(usable && remote.blind > 0 ? `${said} ${t().models_blind(remote.blind)}` : said, usable);
      return;
    }

    const said = remote.outcome === "unlisted" ? t().models_unlisted : t().models_failed;
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
    untested: t().health_untested,
    works: t().health_works,
    failed: t().health_failed,
  };

  const badge = el("health");
  badge.dataset.state = health;
  badge.textContent = words[health];
  badge.title = t()[`health_note_${health}`];
}

/// A change to anything the test was about makes the answer about something
/// else.
function forgetHealth() {
  if (health !== "untested") {
    health = "untested";
    drawHealth();
  }

  // And the settings are no longer what is on file.
  if (dirty()) note(t().changed, false);
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
el("theme").addEventListener("click", toggleTheme);
el("tab-document").addEventListener("click", () => showTab("document"));
el("tab-records").addEventListener("click", openRecords);
el("tab-settings").addEventListener("click", openSettings);
el("settings").addEventListener("submit", saveSettings);
el("reveal").addEventListener("click", toggleReveal);
el("test").addEventListener("click", testModel);
el("fetch-models").addEventListener("click", fetchModels);

// Anything the answer was about: changing it makes the answer about something
// that is no longer on screen.
for (const id of ["set-endpoint", "set-model", "set-model-choice", "set-key"]) {
  el(id).addEventListener("input", forgetHealth);
  el(id).addEventListener("change", forgetHealth);
}


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
