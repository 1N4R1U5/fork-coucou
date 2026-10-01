// Claude Code session in detail (docs/media/claude-code.png): Mochi and the
// step list on the left, the file or command of the latest tool call on the
// right, as an editor tab with line numbers and the diff of an edit.

import { State, type Activity, type CodeLine, type SessionDetail } from "../core/state";
import { clear, h, svg } from "./dom";
import { ICONS } from "./icons";
import type { ViewActions, ViewHost } from "./views";

// ── File badge ────────────────────────────────────────────────────────────────

const BADGES: Record<string, [string, string, string]> = {
  // ext: [label, background, text]
  ts: ["TS", "#3178c6", "#fff"],
  tsx: ["TSX", "#3178c6", "#fff"],
  js: ["JS", "#f0db4f", "#1b1b1b"],
  jsx: ["JSX", "#f0db4f", "#1b1b1b"],
  mjs: ["JS", "#f0db4f", "#1b1b1b"],
  rs: ["RS", "#dea584", "#1b1b1b"],
  py: ["PY", "#3776ab", "#fff"],
  swift: ["SW", "#f05138", "#fff"],
  go: ["GO", "#00add8", "#fff"],
  java: ["JV", "#e76f00", "#fff"],
  kt: ["KT", "#7f52ff", "#fff"],
  c: ["C", "#5c6bc0", "#fff"],
  h: ["H", "#5c6bc0", "#fff"],
  cpp: ["C++", "#00599c", "#fff"],
  cs: ["C#", "#68217a", "#fff"],
  rb: ["RB", "#cc342d", "#fff"],
  php: ["PHP", "#777bb4", "#fff"],
  html: ["<>", "#e34c26", "#fff"],
  css: ["CSS", "#2965f1", "#fff"],
  json: ["{}", "#4b5563", "#f5f6f8"],
  md: ["MD", "#4b5563", "#f5f6f8"],
  toml: ["TOML", "#4b5563", "#f5f6f8"],
  yml: ["YML", "#4b5563", "#f5f6f8"],
  yaml: ["YML", "#4b5563", "#f5f6f8"],
  sh: [">_", "#2b2f36", "#9ee493"],
};

function badge(ext: string): HTMLElement {
  const [label, bg, fg] = BADGES[ext] ?? [ext ? ext.slice(0, 3).toUpperCase() : "··", "#4b5563", "#f5f6f8"];
  return h("b", { class: "ed-badge", style: `background:${bg};color:${fg}`, text: label });
}

// ── Highlighting ──────────────────────────────────────────────────────────────

const KEYWORDS = new Set(
  (
    "import export from as default const let var function return if else for while do " +
    "switch case break continue new class extends implements interface type enum " +
    "async await try catch finally throw typeof instanceof in of yield static public " +
    "private protected readonly this super true false null undefined void " +
    "fn mut pub use mod struct impl trait where match loop move ref self Self crate dyn " +
    "def lambda pass with elif None True False not and or is global nonlocal raise except " +
    "func var guard let defer go package chan select range"
  ).split(" "),
);

const HASH_COMMENT = new Set(["py", "sh", "rb", "toml", "yml", "yaml"]);

/**
 * One line, coloured. Strings, comments, numbers, keywords and capitalised
 * names — enough to read code at a glance, built as text nodes so nothing in
 * the file is ever parsed as HTML.
 */
function highlight(text: string, ext: string): DocumentFragment {
  const frag = document.createDocumentFragment();
  const comment = HASH_COMMENT.has(ext) ? "#" : "//";
  const re = /("(?:[^"\\]|\\.)*"?|'(?:[^'\\]|\\.)*'?|`(?:[^`\\]|\\.)*`?)|(\d[\d_.]*)|([A-Za-z_$][\w$]*)|(\s+)|(.)/g;
  let m: RegExpExecArray | null;
  let i = 0;
  const push = (t: string, cls?: string) => {
    if (!t) return;
    if (cls) frag.append(h("span", { class: cls, text: t }));
    else frag.append(document.createTextNode(t));
  };
  while ((m = re.exec(text))) {
    if (text.startsWith(comment, m.index) && !m[1]) {
      push(text.slice(m.index), "tk-c");
      return frag;
    }
    i = re.lastIndex;
    if (m[1]) push(m[1], "tk-s");
    else if (m[2]) push(m[2], "tk-n");
    else if (m[3]) {
      if (KEYWORDS.has(m[3])) push(m[3], "tk-k");
      else if (/^[A-Z]/.test(m[3])) push(m[3], "tk-t");
      else if (text[i] === "(") push(m[3], "tk-f");
      else push(m[3]);
    } else push(m[0]);
  }
  return frag;
}

// ── Pieces ────────────────────────────────────────────────────────────────────

function statusIcon(a: Activity): HTMLElement {
  if (a.status === "running") return h("i", { class: "st-spin" });
  if (a.status === "failed") return h("i", { class: "st-fail" }, svg(ICONS.xmark, 9));
  if (a.tool === "Bash" || a.tool === "PowerShell") {
    return h("i", { class: "st-term", text: ">_" });
  }
  return h("i", { class: "st-done" }, svg(ICONS.check, 9, { stroke: 3 }));
}

function stepRow(a: Activity, isLast: boolean): HTMLElement {
  const cls = `sess-step ${a.status}${isLast ? " last" : ""}${a.tool === "Done" ? " end" : ""}`;
  return h("li", { class: cls }, statusIcon(a), h("span", { text: a.tool }));
}

function codeRow(line: CodeLine, ext: string): HTMLElement {
  const sign = line.kind === "del" ? "-" : line.kind === "add" ? "+" : "";
  const txt = h("span", { class: "ln-txt" });
  if (line.kind === "cmd" || line.kind === "note") txt.textContent = line.text;
  else txt.append(highlight(line.text, ext));
  return h(
    "div",
    { class: `ln ${line.kind}` },
    h("span", { class: "ln-num", text: line.n == null ? "" : String(line.n) }),
    h("span", { class: "ln-sign", text: sign }),
    txt,
  );
}

// ── View ──────────────────────────────────────────────────────────────────────

export function buildSession(actions: ViewActions): ViewHost {
  const name = h("div", { class: "sess-name" });
  const steps = h("ul", { class: "sess-steps" });
  const left = h(
    "div",
    { class: "sess-left" },
    name,
    h("div", { class: "sess-tool", text: "Claude Code" }),
    steps,
  );

  const tabTitle = h("span", { class: "ed-title" });
  const tabDot = h("i", { class: "ed-dot" });
  const tabBadge = h("span");
  const tabPath = h("span", { class: "ed-path" });
  const jump = h(
    "button",
    { class: "icon-btn ed-jump", title: "Ouvrir le terminal", onclick: () => actions.openTarget() },
    svg(ICONS.arrowUpRight, 8),
  );
  const code = h("div", { class: "ed-code" });
  const editor = h(
    "div",
    { class: "sess-editor" },
    h("div", { class: "ed-bar" }, h("span", { class: "ed-tab" }, tabBadge, tabTitle, tabDot), tabPath, jump),
    code,
  );

  const el = h("div", { class: "view session" }, h("div", { class: "card" }, left, editor));

  let key = "";
  return {
    el,
    sync() {
      const task = State.tasks.find((t) => t.id === "integration_claude");
      const d: SessionDetail | null = State.sessionDetail;
      const acts = State.activities;
      const next = JSON.stringify([task?.name, acts, d]);
      if (next === key) return;
      key = next;

      name.textContent = task?.name ?? "Claude Code";

      clear(steps);
      const shown = acts.slice(-4);
      shown.forEach((a, i) => steps.append(stepRow(a, i === shown.length - 1)));

      clear(tabBadge);
      clear(code);
      if (!d) {
        tabTitle.textContent = "En attente";
        tabDot.style.display = "none";
        tabPath.textContent = "";
        code.append(codeRow({ n: null, kind: "note", text: "La prochaine action de Claude s'affichera ici." }, ""));
        return;
      }
      tabBadge.append(badge(d.ext));
      tabTitle.textContent = d.title;
      tabDot.style.display = d.dirty ? "" : "none";
      tabPath.textContent = d.subtitle;
      for (const line of d.lines) code.append(codeRow(line, d.ext));
    },
  };
}
