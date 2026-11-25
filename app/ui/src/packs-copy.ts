// What the Optional packs card says for each state of a pack (docs/architecture.md §23; the
// screens of the blueprint canvas). No DOM: `packs.test.mjs` runs it in Node.
import type { PackView } from "./api";

export type Button = "setup" | "fix" | "check" | "turnOn" | "review" | "notNow" | "turnOff" | "turnOnAgain";

export interface Row {
  /** The small label next to the name. */
  tag: string;
  tagClass: "" | "off" | "warn";
  text: string;
  buttons: Button[];
}

export interface Context {
  /** This device is signed in as the host. */
  host: boolean;
  /** GitHub could not be reached at the last sync. */
  offline: boolean;
  /** `windows`, `macos` or `linux`. */
  os: string;
  nowMs: number;
  /** A check of this pack is running from this screen. */
  checking: boolean;
}

/** What "this computer" is called when a pack does not work on it. */
export function computerName(os: string): string {
  return os === "macos" ? "Mac" : os === "windows" ? "Windows PC" : "Linux computer";
}

const OS_NAMES: Record<string, string> = { windows: "Windows", macos: "macOS", linux: "Linux" };

/** "Windows and Linux" */
export function osList(os: string[]): string {
  const names = os.map((o) => OS_NAMES[o] ?? o);
  return names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** "just now", "2 min ago", "3 h ago", then the date. */
export function ago(ms: number, nowMs: number): string {
  const s = Math.max(0, Math.round((nowMs - ms) / 1000));
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return new Date(ms).toISOString().slice(0, 10);
}

/** "just now", "today", "yesterday", then the date: for "on since ...". */
export function since(ms: number, nowMs: number): string {
  if (nowMs - ms < 60_000) return "just now";
  const day = (t: number) => Math.floor(t / 86_400_000);
  const days = day(nowMs) - day(ms);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  return new Date(ms).toISOString().slice(0, 10);
}

/** The description under the name of a pack that is not set up: its summary, what it needs. */
export function description(p: PackView): string {
  const needs = p.needs.length > 0 ? ` Needs ${p.needs.map((n) => n.name).join(", ")}.` : "";
  const job = p.jobs.length > 0 ? " Runs on a schedule." : "";
  return `${p.summary}${needs}${job}`;
}

/** The sentence under the heading of the card. */
export function intro(c: Pick<Context, "host" | "offline">): string {
  const parts: string[] = [];
  if (c.offline) {
    parts.push("Setting up needs the internet, because Claude Code does. Check again, Turn on and Turn off work offline.");
  } else if (!c.host) {
    parts.push(
      "Extras for setups like the host's. Each one turns on for this computer only after Claude helps you set it up and its check passes. Nothing here asks you twice.",
    );
  }
  if (c.host) parts.push("You host this group. Packs in use on this computer when packs arrived stayed on.");
  return parts.join(" ");
}

/** "2 min ago: no agent (exit 1)" */
function failedCheck(p: PackView, c: Context): string {
  const r = p.lastCheck!;
  const why = r.tail ? `: ${r.tail} (exit ${r.exit})` : ` (exit ${r.exit})`;
  return `${ago(r.at, c.nowMs)}${why}`;
}

/** How one pack's row reads. A pack that is `ready` also gets a panel (see `screens/packs.ts`). */
export function row(p: PackView, c: Context): Row {
  if (p.state === "unavailable") {
    return {
      tag: `not for ${OS_NAMES[c.os] ?? c.os}`,
      tagClass: "off",
      text: `It works on ${osList(p.os)} only, so it can't be set up on this ${computerName(c.os)}.`,
      buttons: [],
    };
  }
  if (p.claudeClosed || c.checking) {
    return {
      tag: "checking",
      tagClass: "",
      text: p.claudeClosed
        ? `Claude Code closed. Running the pack's check (at most ${p.checkTimeout} s)…`
        : `Running the pack's check (at most ${p.checkTimeout} s)…`,
      buttons: [],
    };
  }
  if (p.settingUp) {
    return {
      tag: "setting up",
      tagClass: "",
      text: "Claude Code is open in a terminal. Finish there; claude-sync checks the pack when you close it.",
      buttons: [],
    };
  }
  switch (p.state) {
    case "ready":
      return {
        tag: "ready",
        tagClass: "",
        text: `The check passed ${ago(p.lastCheck?.at ?? c.nowMs, c.nowMs)}. Turning it on changes this computer only:`,
        buttons: ["turnOn", "notNow"],
      };
    case "on": {
      const job = p.jobs.length > 0;
      if (p.by === "carry-over" || p.by === "host-auto") {
        const day = p.since ? new Date(p.since).toISOString().slice(0, 10) : "";
        return {
          tag: "on",
          tagClass: "",
          text: `On since ${day} · kept on from before packs${job ? " · job accepted" : ""}`,
          buttons: ["turnOff"],
        };
      }
      return {
        tag: "on",
        tagClass: "",
        text: `On on this computer since ${since(p.since ?? c.nowMs, c.nowMs)}${job ? ", with its scheduled job." : ". Gets the pack's updates here only."}`,
        buttons: ["turnOff"],
      };
    }
    case "failing": {
      const r = p.lastCheck;
      const why = r ? `: ${r.tail ? `${r.tail} ` : ""}(exit ${r.exit})` : "";
      return {
        tag: "check fails",
        tagClass: "warn",
        text: `The pack changed and its check now fails${why}. It stays on until you fix it or turn it off.`,
        buttons: ["turnOff", "fix"],
      };
    }
    case "off":
      if (p.by === "carry-over") {
        return {
          tag: "off",
          tagClass: "off",
          text: "It was off on this computer before packs, so it stays off.",
          buttons: ["setup"],
        };
      }
      return {
        tag: "off",
        tagClass: "off",
        text: "You turned it off on this computer. It won't ask again and gets no updates here. Its settings were put back to the group's.",
        buttons: ["turnOnAgain"],
      };
    default:
      if (p.lastCheck && p.lastCheck.exit !== 0) {
        return {
          tag: "not set up",
          tagClass: "off",
          text: `Last check failed ${failedCheck(p, c)}. Nothing was turned on.`,
          buttons: ["check", "setup"],
        };
      }
      return { tag: "not set up", tagClass: "off", text: description(p), buttons: ["setup"] };
  }
}

/** The buttons that need Claude Code, and so the internet. */
export const NEEDS_INTERNET: Button[] = ["setup", "fix"];

/** The mod names a pack turns on: `passbridge-signin@claude-mods` -> `passbridge-signin`. */
export function modNames(p: PackView): string[] {
  return p.plugins.map((x) => x.split("@")[0]);
}

/** The lines of the panel of a pack that is ready: what turning it on changes here. */
export function readyLines(p: PackView): string[] {
  const out: string[] = [];
  const mods = modNames(p);
  if (mods.length > 0) out.push(`Turns on the mod${mods.length > 1 ? "s" : ""} ${mods.join(", ")}`);
  if (p.ruleLines > 0) {
    out.push(
      p.ruleLines === 1 && p.rulePreview
        ? `Adds one line to the rules Claude reads: "${p.rulePreview}"`
        : `Adds ${p.ruleLines} lines to the rules Claude reads${p.rulePreview ? `, starting with "${p.rulePreview}"` : ""}`,
    );
  }
  if (p.hasFiles) out.push("Adds the pack's skills or agents to ~/.claude");
  out.push(p.jobs.length === 0 ? "No scheduled jobs" : `Registers ${p.jobs.length === 1 ? "its scheduled job" : `its ${p.jobs.length} scheduled jobs`} on this computer (below)`);
  out.push("From then on this computer gets the pack's updates; your other computers don't");
  return out;
}
