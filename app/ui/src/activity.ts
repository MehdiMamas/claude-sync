// Turns a group's raw commits into a friendly activity feed: who, from which device, what
// kind of change, grouped by day. Pure functions (no DOM), so `node --test` runs them.
//
// The app writes messages as `<login>@<host>: <what>` (engine.rs `message`): `create group`,
// `add member x`, `remove member x`, `share skills/x`, and sync summaries such as
// `update CLAUDE.md, skills/review`, `remove agents/old.md`, `update a; remove b` or
// `update 5 items`. People and Claude sessions also push `<login>@<host>: <area>: <text>`.
import type { CommitInfo } from "./api";

export type Kind =
  | "created"
  | "memberAdded"
  | "memberRemoved"
  | "skill"
  | "agent"
  | "command"
  | "style"
  | "mod"
  | "memory"
  | "settings"
  | "job"
  | "other";

export interface FeedContext {
  /** The signed-in GitHub login, shown as "You". */
  me?: string | null;
  /** Plugin and marketplace names the group ships, so `setup-scout: …` reads as a mod. */
  mods?: string[];
  skills?: string[];
  agents?: string[];
  commands?: string[];
}

export interface Activity {
  /** Newest commit of the entry. */
  sha: string;
  /** Every commit folded into this entry, newest first. */
  shas: string[];
  login: string;
  isYou: boolean;
  /** Lowercased host name, or null for commits without the `login@host:` prefix. */
  device: string | null;
  kind: Kind;
  title: string;
  /** Names the change is about (a skill, a mod, a member). */
  chips: string[];
  /** The rest of the message: the hand-written text and the commit body. */
  detail: string;
  atUnix: number;
  /** Oldest commit time when several were folded together. */
  firstAtUnix: number;
  /** The newest commit's full message. */
  message: string;
  /** Each folded commit as its own entry, newest first; a single entry holds itself. */
  members: Activity[];
}

const PREFIX = /^([A-Za-z0-9](?:[A-Za-z0-9-]{0,38}))@([^\s:]+):\s*([\s\S]*)$/;
/** Conventional-commit types are not areas: `fix: x` reads as "Fix x"-style free text. */
const CONVENTIONAL = new Set(["fix", "feat", "chore", "docs", "refactor", "ci", "test", "tests", "build", "perf", "style", "wip"]);

const cap = (s: string) => (s ? s[0].toUpperCase() + s.slice(1) : s);
const lower = (s: string | null | undefined) => (s ?? "").toLowerCase();

/** `DESKTOP-MEHDI`, `desktop-mehdi` and `Mehdis-MacBook.local` are one device each. */
export function deviceName(host: string): string {
  return host.toLowerCase().replace(/\.(local|lan|home)$/, "");
}

interface Target {
  kind: Kind;
  /** The chip: a skill or mod name, or "" when the kind says it all. */
  label: string;
}

/** What a repo path (or a summary unit such as `skills/review`) is about. */
export function classifyPath(path: string, ctx: FeedContext = {}): Target {
  const parts = path.replace(/^\/+/, "").split("/");
  const md = (f: string | undefined) => (f ?? "").replace(/\.md$/, "");
  switch (parts[0]) {
    case "skills":
      return { kind: "skill", label: parts[1] ?? "" };
    case "agents":
      return { kind: "agent", label: md(parts[1]) };
    case "commands":
      return { kind: "command", label: md(parts[1]) };
    case "output-styles":
      return { kind: "style", label: md(parts[1]) };
    case "plugins": {
      // plugins/<marketplace>/<…>/<plugin>/…: name the plugin when one segment is known.
      const known = new Set((ctx.mods ?? []).map(lower));
      const hit = parts.slice(2).find((p) => known.has(p.toLowerCase()));
      return { kind: "mod", label: hit ?? parts[1] ?? "" };
    }
    case "jobs":
    case "schedules":
      return { kind: "job", label: md(parts[1]) };
    case "CLAUDE.md":
      return { kind: "memory", label: "" };
    case "settings.shared.json":
      return { kind: "settings", label: "" };
    case "claude-sync.json":
      return { kind: "settings", label: "group file" };
  }
  return { kind: "other", label: path };
}

/** What a hand-written `<area>: <text>` area is about; null when it names nothing known. */
function classifyArea(area: string, ctx: FeedContext): Target | null {
  const a = area.trim();
  const l = a.toLowerCase();
  if (a.includes("/") || a === "CLAUDE.md" || a === "settings.shared.json" || a === "claude-sync.json") {
    const t = classifyPath(a, ctx);
    if (t.kind !== "other") return t;
  }
  if (/^(group )?memory$|^claude\.md$|^instructions$/.test(l)) return { kind: "memory", label: "" };
  if (/^(shared )?settings$|^permissions$/.test(l)) return { kind: "settings", label: "" };
  if (/^(scheduled )?jobs?$|^schedules?$|^scheduler$/.test(l)) return { kind: "job", label: "" };
  if (/^(mods?|plugins?|marketplace)$/.test(l)) return { kind: "mod", label: "" };
  const has = (xs: string[] | undefined) => (xs ?? []).some((x) => x.toLowerCase() === l);
  if (has(ctx.mods)) return { kind: "mod", label: a };
  if (has(ctx.skills)) return { kind: "skill", label: a };
  if (has(ctx.agents)) return { kind: "agent", label: a };
  if (has(ctx.commands)) return { kind: "command", label: a };
  return null;
}

const NOUN: Record<Kind, [string, string]> = {
  created: ["the group", "the group"],
  memberAdded: ["a member", "members"],
  memberRemoved: ["a member", "members"],
  skill: ["a skill", "skills"],
  agent: ["an agent", "agents"],
  command: ["a command", "commands"],
  style: ["an output style", "output styles"],
  mod: ["a mod", "mods"],
  memory: ["the group's CLAUDE.md", "the group's CLAUDE.md"],
  settings: ["shared settings", "shared settings"],
  job: ["a scheduled job", "scheduled jobs"],
  other: ["1 item", "items"],
};

function noun(kind: Kind, n: number): string {
  if (kind === "memory" || kind === "settings" || kind === "created") return NOUN[kind][0];
  return n === 1 ? NOUN[kind][0] : `${n} ${NOUN[kind][1]}`;
}

const VERB: Record<string, string> = { update: "Updated", remove: "Removed", share: "Shared", add: "Added" };

/** `update CLAUDE.md, skills/review; remove agents/old.md` and `update 5 items` from the engine. */
function parseSummary(text: string, ctx: FeedContext): Pick<Activity, "kind" | "title" | "chips"> | null {
  const clauses = text.split(/;\s*/);
  const parsed: { verb: string; targets: Target[]; count: number | null }[] = [];
  for (const clause of clauses) {
    const m = /^(update|remove|share)\s+(.+)$/.exec(clause.trim());
    if (!m) return null;
    const many = /^(\d+) items?$/.exec(m[2]);
    if (many) {
      parsed.push({ verb: m[1], targets: [], count: Number(many[1]) });
      continue;
    }
    const units = m[2].split(/,\s*/);
    // A summary lists paths; hand-written prose ("update the docs") is not one.
    if (!units.every((u) => /^[\w.@+-]+(\/[\w.@+ -]+)*$/.test(u) && (u.includes("/") || u.includes(".")))) return null;
    parsed.push({ verb: m[1], targets: units.map((u) => classifyPath(u, ctx)), count: null });
  }
  const all = parsed.flatMap((p) => p.targets);
  const kinds = new Set(all.map((t) => t.kind));
  const kind: Kind = kinds.size === 1 && parsed.every((p) => p.count === null) ? all[0].kind : "other";
  // One file nothing else describes: name it in the title ("Updated README.md").
  if (kind === "other" && parsed.length === 1 && all.length === 1) {
    return { kind, title: `${VERB[parsed[0].verb]} ${all[0].label}`, chips: [] };
  }
  const title = parsed
    .map((p, i) => {
      // Two files of one mod are one mod.
      const n = p.count ?? new Set(p.targets.map((t) => `${t.kind}/${t.label}`)).size;
      const phrase = `${VERB[p.verb]} ${noun(kind, n)}`;
      return i === 0 ? phrase : phrase[0].toLowerCase() + phrase.slice(1);
    })
    .join(", ");
  const chipOf = (t: Target) => t.label || (kind === "other" ? (t.kind === "memory" ? "CLAUDE.md" : t.kind === "settings" ? "settings" : "") : "");
  const chips = [...new Set(all.map(chipOf).filter(Boolean))];
  return { kind, title, chips };
}

/** One commit as a feed entry. */
export function parseCommit(c: CommitInfo, ctx: FeedContext = {}): Activity {
  const [first, ...bodyLines] = c.message.replace(/\r\n/g, "\n").split("\n");
  const body = bodyLines.join("\n").trim();
  const m = PREFIX.exec(first.trim());
  const login = m ? m[1] : c.author || "someone";
  const device = m ? deviceName(m[2]) : null;
  const text = (m ? m[3] : first).trim();
  const base = {
    sha: c.sha,
    shas: [c.sha],
    login,
    isYou: !!ctx.me && lower(ctx.me) === lower(login),
    device,
    atUnix: c.atUnix,
    firstAtUnix: c.atUnix,
    message: c.message,
  };
  const out = (kind: Kind, title: string, chips: string[] = [], detail = ""): Activity => {
    const a: Activity = {
      ...base,
      kind,
      title,
      chips,
      detail: [cap(detail.trim()), body].filter(Boolean).join("\n"),
      members: [],
    };
    a.members = [a];
    return a;
  };

  const created = /^create group\b\s*(.*)$/i.exec(text);
  if (created) return out("created", "Created the group", [], created[1].replace(/^\((.*)\)$/, "$1"));
  const member = /^(add|remove) member ([A-Za-z0-9-]+)\s*$/i.exec(text);
  if (member) {
    const who = ctx.me && lower(ctx.me) === lower(member[2]) ? "you" : member[2];
    return member[1].toLowerCase() === "add"
      ? out("memberAdded", `Added ${who} to the group`)
      : out("memberRemoved", `Removed ${who} from the group`);
  }
  const summary = parseSummary(text, ctx);
  if (summary) return out(summary.kind, summary.title, summary.chips);

  const area = /^([^:\n]{1,40}):\s+([\s\S]+)$/.exec(text);
  if (area && !CONVENTIONAL.has(area[1].trim().toLowerCase()) && area[1].trim().split(/\s+/).length <= 4) {
    const name = area[1].trim();
    let rest = area[2].trim();
    if (/^v?\d+(\.\d+){1,2}([-+][\w.]+)?$/.test(rest)) rest = `version ${rest.replace(/^v/, "")}`;
    const t = classifyArea(name, ctx);
    if (!t) return out("other", `Updated ${name}`, [], rest);
    const title: Record<Kind, string> = {
      created: "Created the group",
      memberAdded: "Added a member",
      memberRemoved: "Removed a member",
      skill: "Updated a skill",
      agent: "Updated an agent",
      command: "Updated a command",
      style: "Updated an output style",
      mod: "Updated a mod",
      memory: "Updated group memory",
      settings: "Changed shared settings",
      job: "Changed a scheduled job",
      other: `Updated ${name}`,
    };
    return out(t.kind, title[t.kind], t.label ? [t.label] : [], rest);
  }
  return out("other", cap(text) || "Changed the group");
}

/**
 * Folds runs of consecutive look-alike entries (same person, kind, title and names, each
 * within `windowSecs` of the next) into one, newest first. Input must be newest first.
 */
export function collapse(items: Activity[], windowSecs = 600): Activity[] {
  const out: Activity[] = [];
  const key = (a: Activity) => [lower(a.login), a.kind, a.title, a.chips.join("\u0000")].join("\u0001");
  for (const a of items) {
    const prev = out[out.length - 1];
    if (prev && key(prev) === key(a) && prev.firstAtUnix - a.atUnix <= windowSecs) {
      const details = new Set(prev.detail.split("\n").filter(Boolean));
      const extra = a.detail.split("\n").filter((d) => d && !details.has(d));
      out[out.length - 1] = {
        ...prev,
        shas: [...prev.shas, ...a.shas],
        firstAtUnix: a.firstAtUnix,
        members: [...prev.members, ...a.members],
        detail: [prev.detail, ...extra].filter(Boolean).join("\n"),
      };
    } else {
      out.push(a);
    }
  }
  return out;
}

export interface ChangeParts {
  subject: string;
  /** Issue numbers named in the message, in order, without duplicates. */
  issues: number[];
  bullets: string[];
  before: string | null;
  now: string | null;
}

const BEFORE = /^(?:before|was)\s*:\s*(.+)$/i;
const NOW = /^(?:now|after)\s*:\s*(.+)$/i;

/** `#12`, `Fixes #182/#183`, `closes #4, #5`: every issue number in the text. */
function issueRefs(text: string): number[] {
  const out: number[] = [];
  const add = (n: string) => {
    const v = Number(n);
    if (!out.includes(v)) out.push(v);
  };
  for (const m of text.matchAll(/(?<![\w&/])#(\d+)((?:\s*(?:\/|,|&|and)\s*#?\d+)*)/g)) {
    add(m[1]);
    for (const x of m[2].matchAll(/\d+/g)) add(x[0]);
  }
  return out;
}

/** Before/Now for a commit: its own body lines first, else the group's notes entry for its sha, else nothing. */
export function beforeNowFor(
  parts: Pick<ChangeParts, "before" | "now">,
  sha: string,
  notes: Record<string, { before?: string | null; now?: string | null }>,
): { before: string | null; now: string | null } {
  if (parts.before || parts.now) return { before: parts.before, now: parts.now };
  const n = notes[sha];
  return { before: n?.before || null, now: n?.now || null };
}

/**
 * A raw commit message split for display. Before/Now come only from `Before:`/`Now:` lines
 * (or `Was:`/`After:`) or a body line `X → Y`; nothing is made up when they are absent.
 */
export function changeParts(message: string): ChangeParts {
  const [first, ...rest] = message.replace(/\r\n/g, "\n").split("\n");
  let subject = first.trim();
  const p = PREFIX.exec(subject);
  if (p) subject = p[3].trim();
  const area = /^([^:\n]{1,40}):\s+([\s\S]+)$/.exec(subject);
  if (area && !CONVENTIONAL.has(area[1].trim().toLowerCase()) && area[1].trim().split(/\s+/).length <= 4) subject = area[2].trim();
  const issues = issueRefs(message);
  // A trailing "(Fixes #1/#2)" is shown as chips instead.
  subject = cap(subject.replace(/\s*\((?:(?:fix(?:es|ed)?|close[sd]?|resolve[sd]?|refs?|see)\s+)?#\d+[^)]*\)\s*$/i, "").trim());

  let before: string | null = null;
  let now: string | null = null;
  const bullets: string[] = [];
  for (const raw of rest) {
    const line = raw.trim().replace(/^[-*•]\s+/, "").trim();
    if (!line) continue;
    const b = BEFORE.exec(line);
    const n = NOW.exec(line);
    if (b && before === null) before = b[1].trim();
    else if (n && now === null) now = n[1].trim();
    else bullets.push(line);
  }
  if (before === null && now === null) {
    const i = bullets.findIndex((l) => /\S\s*(?:→|->)\s*\S/.test(l));
    if (i >= 0) {
      const [x, y] = bullets[i].split(/\s*(?:→|->)\s*/, 2);
      const b = x.replace(/^.*?:\s+/, "").trim();
      const n = y.trim().replace(/[.;]$/, "");
      if (b && n) {
        before = b;
        now = n;
        bullets.splice(i, 1);
      }
    }
  }
  return { subject, issues, bullets, before, now };
}

export interface Segment {
  text: string;
  strong?: boolean;
  code?: boolean;
}

const HIGHLIGHT =
  /`([^`\n]+)`|\b(\d+(?:[.,]\d+)?\s?(?:k|m|ms|s|sec|secs|seconds?|min|mins|minutes?|h|hrs?|hours?|days?|d|kb|mb|gb|tb|x)\b|\d+(?:[.,]\d+)?%)|\b(no longer|now|instead|never|always)\b/gi;

/** Splits text into plain, bold (numbers with units, key words) and code (`backticked`) runs. */
export function highlight(text: string): Segment[] {
  const out: Segment[] = [];
  let at = 0;
  for (const m of text.matchAll(HIGHLIGHT)) {
    const i = m.index ?? 0;
    if (i > at) out.push({ text: text.slice(at, i) });
    if (m[1] !== undefined) out.push({ text: m[1], code: true });
    else out.push({ text: m[0], strong: true });
    at = i + m[0].length;
  }
  if (at < text.length) out.push({ text: text.slice(at) });
  return out;
}

export interface Day {
  /** `YYYY-MM-DD` in local time. */
  key: string;
  label: string;
  items: Activity[];
}

const dayKey = (d: Date) =>
  `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

/** "Today", "Yesterday", a weekday within the last week, else a date. */
export function dayLabel(at: Date, now: Date, locale?: string): string {
  const startOf = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((startOf(now) - startOf(at)) / 86_400_000);
  if (days <= 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days < 7) return at.toLocaleDateString(locale, { weekday: "long" });
  return at.toLocaleDateString(locale, {
    weekday: "short",
    day: "numeric",
    month: "short",
    year: at.getFullYear() === now.getFullYear() ? undefined : "numeric",
  });
}

/** Entries (newest first) by local day, newest day first. */
export function groupByDay(items: Activity[], now: Date, locale?: string): Day[] {
  const days: Day[] = [];
  for (const a of items) {
    const at = new Date(a.atUnix * 1000);
    const key = dayKey(at);
    const last = days[days.length - 1];
    if (last && last.key === key) last.items.push(a);
    else days.push({ key, label: dayLabel(at, now, locale), items: [a] });
  }
  return days;
}

/** Relative today ("5 min ago"), the clock time on earlier days. */
export function timeLabel(atUnix: number, now: Date, locale?: string): string {
  const at = new Date(atUnix * 1000);
  if (dayKey(at) === dayKey(now)) {
    const s = Math.round(now.getTime() / 1000 - atUnix);
    if (s < 60) return "just now";
    if (s < 3600) return `${Math.round(s / 60)} min ago`;
    const h = Math.floor(s / 3600);
    return `${h} h ago`;
  }
  return at.toLocaleTimeString(locale, { hour: "numeric", minute: "2-digit" });
}

/** "MM" for "MehdiMamas", "IA" for "ibrahim-ahidar", "SD" for "sara-dev". */
export function initials(login: string): string {
  const parts = login.split(/[-_.]+/).filter(Boolean);
  if (parts.length > 1) return (parts[0][0] + parts[1][0]).toUpperCase();
  const caps = login.match(/[A-Z]/g);
  if (caps && caps.length > 1) return (caps[0] + caps[1]).toUpperCase();
  return login.slice(0, 2).toUpperCase();
}

/**
 * The GitHub avatar picture of a login (no redirect, allowed by the app's CSP img-src), or
 * null when the string is not a plain login: then the avatar shows initials only.
 */
export function avatarUrl(login: string, size = 64): string | null {
  if (!/^[A-Za-z0-9-]+$/.test(login)) return null;
  return `https://avatars.githubusercontent.com/${encodeURIComponent(login)}?s=${size}`;
}

/** A stable hue per login for the avatar background. */
export function hue(login: string): number {
  let n = 0;
  for (const ch of login.toLowerCase()) n = (n * 31 + ch.charCodeAt(0)) >>> 0;
  return n % 360;
}
