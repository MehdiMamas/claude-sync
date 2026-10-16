// Picking parts of a CLAUDE.md to copy to another group, and how clashes between groups
// read (docs/architecture.md §7b). No DOM: `copy.test.mjs` runs it in Node.
import type { Clash, ClashSource, GroupId } from "./api";

/** One thing to pick: a line, or a whole code block. */
export interface Block {
  key: string;
  lines: string[];
}
/** A heading and the blocks under it. The text before the first heading has none. */
export interface Section {
  key: string;
  heading: string | null;
  blocks: Block[];
}

/** `text` cut at its headings, each line (or fenced code block) one block. */
export function sections(text: string): Section[] {
  const out: Section[] = [];
  let n = 0;
  let cur: Section = { key: "s0", heading: null, blocks: [] };
  let fence: string[] | null = null;
  const add = (lines: string[]) => cur.blocks.push({ key: `${cur.key}.${cur.blocks.length}`, lines });
  for (const line of text.replace(/\r\n/g, "\n").split("\n")) {
    const t = line.trim();
    if (fence) {
      fence.push(line);
      if (t.startsWith("```") || t.startsWith("~~~")) {
        add(fence);
        fence = null;
      }
    } else if (t.startsWith("```") || t.startsWith("~~~")) {
      fence = [line];
    } else if (/^#{1,6}\s/.test(t)) {
      if (cur.heading !== null || cur.blocks.length > 0) out.push(cur);
      cur = { key: `s${++n}`, heading: line, blocks: [] };
    } else if (t !== "") {
      add([line]);
    }
  }
  if (fence) add(fence);
  if (cur.heading !== null || cur.blocks.length > 0) out.push(cur);
  return out;
}

/** The picked blocks as text, each with its heading, sections apart by a blank line. */
export function compose(all: Section[], picked: ReadonlySet<string>): string {
  const parts: string[] = [];
  for (const s of all) {
    const lines = s.blocks.filter((b) => picked.has(b.key)).flatMap((b) => b.lines);
    if (lines.length === 0) continue;
    parts.push(s.heading === null ? lines.join("\n") : `${s.heading}\n\n${lines.join("\n")}`);
  }
  return parts.length === 0 ? "" : `${parts.join("\n\n")}\n`;
}

/** Every block key of `all`, or of one section. */
export function keys(all: Section[], section?: string): string[] {
  return all.filter((s) => section === undefined || s.key === section).flatMap((s) => s.blocks.map((b) => b.key));
}

export const sameGroup = (a: GroupId, b: GroupId) => a.owner === b.owner && a.repo === b.repo;

export const isGroup = (s: ClashSource, id: GroupId) => s.kind === "group" && sameGroup(s.id, id);

/** Clashes with a side in group `id`, that side first. */
export function clashesOf(all: Clash[], id: GroupId): Clash[] {
  return all
    .filter((c) => isGroup(c.a.source, id) || isGroup(c.b.source, id))
    .map((c) => (isGroup(c.a.source, id) ? c : { ...c, a: c.b, b: c.a }));
}

/** How an incoming line clashes with what Claude already reads, said plainly. */
export function warning(c: Clash): string {
  return c.kind === "contradiction"
    ? `"${c.a.text}" contradicts ${c.b.name}: "${c.b.text}"`
    : `"${c.a.text}" is already in ${c.b.name}: "${c.b.text}"`;
}

/** The clash's title, with `a` the side in the group on screen. */
export function clashTitle(c: Clash): string {
  if (c.kind === "setting") return `${c.a.name} and ${c.b.name} set ${c.pointer ?? "a setting"} differently`;
  if (c.kind === "duplicate") return `${c.b.name} says the same thing`;
  return `${c.b.name} says the opposite`;
}
