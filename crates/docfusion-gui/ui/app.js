// The window's half of the conversation.
//
// Everything of substance happens on the other side of `invoke`: this file
// draws a view, asks for another one, and does two things the webview is better
// at than Rust — copying to the clipboard and knowing what is selected.

const { invoke } = window.__TAURI__.core;

const el = (id) => document.getElementById(id);
const screen = el("screen");
const status = el("status");
const drawer = el("drawer");
const records = el("records");
const problem = el("problem");

let current = null;

// ── drawing ─────────────────────────────────────────────────────────────────

function draw(view) {
  current = view;

  problem.hidden = !view.problem;
  problem.textContent = view.problem ?? "";

  const state = view.screen;

  if (state.kind === "empty") {
    const where = document.createElement("dl");
    where.className = "where";
    for (const [term, value] of [
      ["Documents", view.knowledge_base],
      ["Settings", view.config_path],
    ]) {
      where.append(node("dt", "", term), node("dd", "", value));
    }

    screen.replaceChildren(
      describe("empty", [
        node("p", "lead", "Drop a document here, or add one."),
        node(
          "p",
          "hint",
          "Anything with a text layer is read as it is. A scan or a photo is read by the engine inside this app.",
        ),
        where,
      ]),
    );
  } else if (state.kind === "working") {
    const box = document.createElement("div");
    box.className = "working";
    box.append(
      node("div", "pulse", ""),
      node("p", "", `Reading ${state.name}…`),
      node("p", "where", "A scan takes a few seconds a page."),
    );
    screen.replaceChildren(box);
  } else if (state.kind === "ready") {
    const article = document.createElement("article");
    article.className = "document";
    // Rendered by the same Markdown pipeline the command line writes with, and
    // escaped there: extracted text is not trusted markup.
    article.innerHTML = state.html;
    screen.replaceChildren(article);
    screen.scrollTop = 0;
  } else {
    const box = document.createElement("div");
    box.className = "failure";
    box.append(
      node("h2", "", `${state.name} could not be read`),
      node("pre", "", state.message),
    );
    screen.replaceChildren(box);
  }

  drawStatus(state);
}

function drawStatus(state) {
  status.replaceChildren();

  if (state.kind !== "ready") {
    status.hidden = true;
    return;
  }

  status.hidden = false;
  status.append(node("span", "", state.engines.join(" · ") || "read"));

  if (state.reused) {
    status.append(node("span", "", "already in the knowledge base"));
  }

  if (state.conflicts > 0) {
    const many = state.conflicts === 1 ? "1 place" : `${state.conflicts} places`;
    status.append(node("span", "warn", `the engines disagreed in ${many}`));
  }

  status.append(node("span", "spacer", ""));
  status.append(button("Copy", "quiet", copyAll));
}

function drawRecords(view) {
  records.replaceChildren();

  if (view.records.length === 0) {
    records.append(node("li", "none", "Nothing here yet."));
    return;
  }

  for (const record of [...view.records].reverse()) {
    const item = document.createElement("li");
    const open = button("", "", () => openRecord(record.id));

    open.append(node("span", "name", record.name));
    open.append(
      node(
        "span",
        "meta",
        [record.status, record.conflicts ? `${record.conflicts} to review` : null, record.created_at]
          .filter(Boolean)
          .join(" · "),
      ),
    );

    item.append(open);
    records.append(item);
  }
}

// ── asking ──────────────────────────────────────────────────────────────────

async function add() {
  const path = await invoke("choose_file");
  if (!path) return;
  await run(() => invoke("process", { path }));
}

async function openRecord(id) {
  drawer.hidden = true;
  await run(() => invoke("open", { id }));
}

async function run(action) {
  busy(true);
  try {
    draw(await action());
  } catch (error) {
    // The view already carries the failure: `process` reports through it. This
    // is for the cases that never got that far, and says so rather than
    // silently doing nothing.
    draw({
      ...current,
      screen: { kind: "failed", name: "DocFusion", message: String(error) },
    });
  } finally {
    busy(false);
  }
}

function busy(on) {
  el("add").disabled = on;
}

async function copyAll() {
  const article = screen.querySelector(".document");
  if (!article) return;

  await navigator.clipboard.writeText(article.innerText);
  const label = status.lastElementChild;
  const was = label.textContent;
  label.textContent = "Copied";
  setTimeout(() => (label.textContent = was), 1200);
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
if (window.__TAURI__.webview?.getCurrentWebview) {
  window.__TAURI__.webview.getCurrentWebview().onDragDropEvent((event) => {
    if (event.payload?.type !== "drop") return;
    const path = event.payload.paths?.[0];
    if (path) run(() => invoke("process", { path }));
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
  element.addEventListener("click", onClick);
  return element;
}

function describe(className, children) {
  const box = document.createElement("div");
  box.className = className;
  box.append(...children);
  return box;
}

el("add").addEventListener("click", add);
el("records").addEventListener("click", async () => {
  drawer.hidden = false;
  drawRecords(await invoke("records"));
});
el("close-drawer").addEventListener("click", () => (drawer.hidden = true));

screen.replaceChildren();
draw(await invoke("view"));
