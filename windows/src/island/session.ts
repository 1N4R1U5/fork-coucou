// The Claude Code session view's data: the step list (one entry per tool call,
// with its status) and the editor panel (the file or command of the latest call).
//
// Everything comes from the hook payload. The one thing it lacks is *where* an
// edit lands, so the file is read back (locally, a few lines) for line numbers
// and the lines around the change.

import { Bridge } from "../core/bridge";
import { State, type CodeLine, type SessionDetail } from "../core/state";

const MAX_ACTIVITIES = 6;
/** Lines the editor panel has room for. */
const PANEL_LINES = 9;

/** Bumped on every tool call so a slow file read never overwrites a newer one. */
let seq = 0;

// ── Step list ─────────────────────────────────────────────────────────────────

export function resetActivities() {
  State.activities = [];
}

export function startActivity(tool: string) {
  State.activities.push({ tool, status: "running" });
  if (State.activities.length > MAX_ACTIVITIES) {
    State.activities = State.activities.slice(-MAX_ACTIVITIES);
  }
}

export function finishActivity(tool: string, ok: boolean) {
  for (let i = State.activities.length - 1; i >= 0; i--) {
    const a = State.activities[i];
    if (a.status === "running" && (a.tool === tool || !tool)) {
      a.status = ok ? "done" : "failed";
      break;
    }
  }
  if (State.sessionDetail && State.sessionDetail.tool === tool) {
    State.sessionDetail.dirty = false;
  }
}

/** The turn is over: nothing is running any more, and the list says so. */
export function finishTurn() {
  for (const a of State.activities) if (a.status === "running") a.status = "done";
  startActivity("Done");
  finishActivity("Done", true);
  if (State.sessionDetail) State.sessionDetail.dirty = false;
}

export function clearSessionView() {
  seq++;
  State.activities = [];
  State.sessionDetail = null;
}

// ── Editor panel ──────────────────────────────────────────────────────────────

function str(input: Record<string, unknown>, key: string): string | null {
  const v = input[key];
  return typeof v === "string" && v.length > 0 ? v : null;
}

function num(input: Record<string, unknown>, key: string): number | null {
  const v = input[key];
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

function baseName(p: string): string {
  const parts = p.split(/[\\/]/);
  return parts[parts.length - 1] || p;
}

function extOf(p: string): string {
  const b = baseName(p);
  const dot = b.lastIndexOf(".");
  return dot > 0 ? b.slice(dot + 1).toLowerCase() : "";
}

function relative(path: string, cwd: string): string {
  if (cwd && path.startsWith(cwd.replace(/[\\/]+$/, "") + "/")) {
    return path.slice(cwd.replace(/[\\/]+$/, "").length + 1);
  }
  return path;
}

function splitLines(s: string): string[] {
  const lines = s.replace(/\r\n/g, "\n").split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

function fileDetail(tool: string, path: string, cwd: string, dirty: boolean, lines: CodeLine[]): SessionDetail {
  return {
    tool,
    title: baseName(path),
    subtitle: relative(path, cwd),
    ext: extOf(path),
    dirty,
    lines,
  };
}

/** An edit as a diff: a little context, the removed lines, the added lines. */
async function editLines(path: string, oldStr: string, newStr: string): Promise<CodeLine[]> {
  const oldLines = splitLines(oldStr);
  const newLines = splitLines(newStr);
  // Keep the change itself visible: at most this many of each kind.
  const room = Math.max(2, Math.floor((PANEL_LINES - 2) / 2));
  const del = oldLines.slice(0, room);
  const add = newLines.slice(0, room);
  const spare = Math.max(0, PANEL_LINES - del.length - add.length);
  const before = Math.min(2, Math.ceil(spare / 2));
  const after = Math.max(0, Math.min(3, spare - before));

  const snip = await Bridge.readSnippet(path, { needle: oldStr, context: Math.max(before, after) });
  if (!snip || snip.matchLine == null) {
    return [
      ...del.map((text) => ({ n: null, kind: "del" as const, text })),
      ...add.map((text) => ({ n: null, kind: "add" as const, text })),
    ];
  }
  const m = snip.matchLine;
  const out: CodeLine[] = [];
  // Context before the change.
  snip.lines.forEach((text, i) => {
    const n = snip.start + i;
    if (n < m && n >= m - before) out.push({ n, kind: "ctx", text });
  });
  del.forEach((text, i) => out.push({ n: m + i, kind: "del", text }));
  add.forEach((text, i) => out.push({ n: m + i, kind: "add", text }));
  // Context after, renumbered as it will read once the edit lands.
  const shift = newLines.length - oldLines.length;
  const oldEnd = m + oldLines.length; // first line after the replaced block
  snip.lines.forEach((text, i) => {
    const n = snip.start + i;
    if (n >= oldEnd && n < oldEnd + after) out.push({ n: n + shift, kind: "ctx", text });
  });
  return out;
}

async function buildDetail(
  tool: string,
  input: Record<string, unknown>,
  cwd: string,
): Promise<SessionDetail> {
  const file = str(input, "file_path") ?? str(input, "notebook_path");

  if ((tool === "Edit" || tool === "MultiEdit") && file) {
    let oldStr = str(input, "old_string") ?? "";
    let newStr = str(input, "new_string") ?? "";
    const edits = input.edits;
    if (tool === "MultiEdit" && Array.isArray(edits) && edits.length > 0) {
      const first = edits[0] as Record<string, unknown>;
      oldStr = str(first, "old_string") ?? "";
      newStr = str(first, "new_string") ?? "";
    }
    return fileDetail(tool, file, cwd, true, await editLines(file, oldStr, newStr));
  }

  if (tool === "Write" && file) {
    const content = str(input, "content") ?? "";
    const lines = splitLines(content)
      .slice(0, PANEL_LINES)
      .map((text, i) => ({ n: i + 1, kind: "add" as const, text }));
    return fileDetail(tool, file, cwd, true, lines);
  }

  if (tool === "Read" && file) {
    const offset = num(input, "offset") ?? 1;
    const snip = await Bridge.readSnippet(file, { offset, limit: PANEL_LINES });
    const lines: CodeLine[] = snip
      ? snip.lines.map((text, i) => ({ n: snip.start + i, kind: "ctx", text }))
      : [{ n: null, kind: "note", text: "Aperçu indisponible" }];
    return fileDetail(tool, file, cwd, false, lines);
  }

  if (tool === "Bash" || tool === "PowerShell") {
    const command = str(input, "command") ?? "";
    const lines: CodeLine[] = [];
    const desc = str(input, "description");
    if (desc) lines.push({ n: null, kind: "note", text: `# ${desc}` });
    splitLines(command)
      .slice(0, PANEL_LINES - lines.length)
      .forEach((text, i) => lines.push({ n: null, kind: "cmd", text: i === 0 ? `$ ${text}` : `  ${text}` }));
    return { tool, title: "Terminal", subtitle: cwd ? baseName(cwd) : "", ext: "sh", dirty: false, lines };
  }

  // Everything else: what the tool was asked for, one field per line.
  const fields: [string, string][] = [
    ["pattern", "motif"],
    ["path", "dossier"],
    ["glob", "fichiers"],
    ["query", "requête"],
    ["url", "url"],
    ["description", "tâche"],
    ["prompt", "consigne"],
  ];
  const lines: CodeLine[] = [];
  for (const [key, label] of fields) {
    const v = str(input, key);
    if (v) lines.push({ n: null, kind: "note", text: `${label}: ${splitLines(v)[0]}` });
  }
  if (lines.length === 0) lines.push({ n: null, kind: "note", text: tool });
  return { tool, title: tool, subtitle: cwd ? baseName(cwd) : "", ext: "", dirty: false, lines };
}

/** PreToolUse: show what this call is about. */
export function showToolCall(tool: string, input: Record<string, unknown>, cwd: string) {
  const mine = ++seq;
  void buildDetail(tool, input, cwd)
    .then((detail) => {
      if (mine !== seq) return; // a newer call already replaced it
      // The file read can outlast a quick tool: don't show it as in flight then.
      const last = [...State.activities].reverse().find((a) => a.tool === tool);
      if (last && last.status !== "running") detail.dirty = false;
      State.sessionDetail = detail;
      State.notify();
    })
    .catch(() => {});
}
