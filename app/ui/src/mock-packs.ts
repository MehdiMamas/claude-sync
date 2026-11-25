// The fake backend of the Optional packs card (`pnpm -C app/ui dev`). Nine packs as in the
// blueprint canvas. Query flags pick the screen:
//   (none)          a member, every pack not set up; Set up with Claude opens the chat, then the check
//   ?packsFail      the check fails ("PassBridge agent not reachable")
//   ?packsNoClaude  Claude Code is not installed
//   ?packsNoTerminal no terminal opens: the command to run by hand is shown
//   ?packsOn        PassBridge is on
//   ?packsChanged   PassBridge changed upstream and its check fails
//   ?packsOff       PassBridge was turned off by the member
//   ?packsHost      the host's machine: packs in use stayed on, reflect stays off
//   ?packsMac       a Mac: the busy switch is not for macOS
//   ?packsOffline   GitHub could not be reached
import type { GroupId, PackCheckReport, PackJob, PackView, PacksOverview, SetupLaunch } from "./api";

const q = new URLSearchParams(location.search);
const DAY = 86_400_000;

type Spec = Pick<PackView, "id" | "title" | "summary" | "needs" | "os" | "plugins" | "jobs" | "ruleLines" | "rulePreview" | "hasFiles">;

const scoutJob: PackJob = {
  id: "setup-scout",
  title: "Claude setup refresh",
  schedule: "Mon, Wed and Fri at 10:17",
  prompt: "/setup-scout:setup-scout unattended",
  cwd: "~",
  permissionMode: "bypassPermissions",
  timeoutMinutes: 60,
  args: ["--chrome"],
  report: "~/.claude/setup-scout/reports/latest.md",
  hash: "5d1c0b7e",
};

const SPECS: Spec[] = [
  { id: "passbridge", title: "PassBridge sign-in", summary: "Claude signs in to GitHub, Google, Vercel and Apple through your PassBridge app.", needs: [{ name: "the PassBridge app" }], os: ["windows", "macos", "linux"], plugins: ["passbridge-signin@claude-mods"], jobs: [], ruleLines: 1, rulePreview: "then PassBridge for sign-ins", hasFiles: false },
  { id: "keytray", title: "Keytray sign-in", summary: "Claude finishes emailed sign-in codes and magic links through Keytray.", needs: [{ name: "the Keytray app" }], os: ["windows", "macos", "linux"], plugins: ["keytray-signin@claude-mods"], jobs: [], ruleLines: 1, rulePreview: "or Keytray", hasFiles: false },
  { id: "busy-switch", title: "Busy switch", summary: "Pauses Claude on this computer while your busy switch is off.", needs: [{ name: "the busy switch" }], os: ["windows", "linux"], plugins: ["switch-enforce@claude-mods"], jobs: [], ruleLines: 0, rulePreview: null, hasFiles: false },
  { id: "setup-scout", title: "Setup scout", summary: "Mon, Wed and Fri: reviews your Claude setup and installs vetted improvements.", needs: [], os: ["windows", "macos", "linux"], plugins: ["setup-scout@claude-mods"], jobs: [scoutJob], ruleLines: 0, rulePreview: null, hasFiles: false },
  { id: "blueprint-workflow", title: "Blueprint workflow", summary: "An issue and a blueprint canvas before any change bigger than a small fix.", needs: [{ name: "gh signed in" }], os: ["windows", "macos", "linux"], plugins: [], jobs: [], ruleLines: 6, rulePreview: "Blueprint first", hasFiles: false },
  { id: "tidy", title: "Tidy", summary: "Trims build caches once a day and warns when under 20 GB are free.", needs: [], os: ["windows", "macos", "linux"], plugins: ["tidy@claude-mods"], jobs: [], ruleLines: 0, rulePreview: null, hasFiles: false },
  { id: "reflect", title: "Reflect", summary: "Collects your corrections; /reflect turns them into CLAUDE.md rules after review.", needs: [], os: ["windows", "macos", "linux"], plugins: ["reflect@claude-mods"], jobs: [], ruleLines: 0, rulePreview: null, hasFiles: false },
  { id: "boss-session", title: "Boss session", summary: "One chat per project, one task per session, and chat links for waiting-on-you lists.", needs: [], os: ["windows", "macos", "linux"], plugins: ["chat-links@claude-mods"], jobs: [], ruleLines: 2, rulePreview: "One chat per project", hasFiles: false },
  { id: "klawly", title: "Klawly", summary: "Project skill for the Klawly app.", needs: [{ name: "read access to its repo" }], os: ["windows", "macos", "linux"], plugins: [], jobs: [], ruleLines: 0, rulePreview: null, hasFiles: true },
];

const os = q.has("packsMac") ? "macos" : "windows";
const host = q.has("packsHost");

function fresh(s: Spec): PackView {
  return {
    ...s,
    state: s.os.includes(os) ? "available" : "unavailable",
    by: null,
    since: null,
    lastCheck: null,
    checkTimeout: 60,
    settingUp: false,
    claudeClosed: false,
    changed: false,
    groupEnabled: false,
  };
}

const packs: PackView[] = SPECS.map(fresh);
const byId = (id: unknown) => {
  const p = packs.find((x) => x.id === id);
  if (!p) throw `this group has no pack ${String(id)}`;
  return p;
};

if (host) {
  for (const p of packs) {
    Object.assign(p, p.id === "reflect" ? { state: "off", by: "carry-over", since: Date.now() - 3 * DAY } : { state: "on", by: "carry-over", since: Date.now() - 3 * DAY });
  }
}
if (q.has("packsOn") || q.has("packsChanged") || q.has("packsOff")) {
  const p = byId("passbridge");
  p.since = Date.now() - 2 * DAY;
  p.by = "setup";
  p.state = q.has("packsOn") ? "on" : q.has("packsOff") ? "off" : "failing";
  if (q.has("packsChanged")) p.lastCheck = { at: Date.now() - 60_000, exit: 1, tail: "PassBridge 2.0 or later is needed", hash: "x" };
}

const overview = (): PacksOverview => ({ packs: packs.map((p) => ({ ...p })), problems: [], host, os });

export function packHandlers(): Record<string, (a: Record<string, unknown>) => unknown> {
  const closeLater = (p: PackView) => {
    p.settingUp = true;
    setTimeout(() => {
      p.settingUp = false;
      p.claudeClosed = true;
    }, 2500);
  };
  return {
    list_packs: (a) => ((a.group as GroupId).owner === "acme" ? { packs: [], problems: [], host: false, os } : overview()),
    start_pack_setup: (a): SetupLaunch => {
      if (q.has("packsNoClaude")) return { started: false, problem: "Claude Code isn't installed or signed in", command: null };
      if (q.has("packsNoTerminal")) return { started: false, problem: "No terminal found", command: `cd '~/.claude-sync/groups/alice__claude-sync-data/packs/${String(a.id)}' && 'claude' 'Read setup.md in this folder and help me set up this pack.'` };
      const p = byId(a.id);
      if (p.state === "available" || p.state === "off") p.state = "tried";
      closeLater(p);
      return { started: true, problem: null, command: null };
    },
    check_pack: (a): PackCheckReport => {
      const p = byId(a.id);
      p.claudeClosed = false;
      const fail = q.has("packsFail");
      p.lastCheck = fail ? { at: Date.now(), exit: 1, tail: "PassBridge agent not reachable", hash: "x" } : { at: Date.now(), exit: 0, tail: "", hash: "x" };
      if (p.state === "on" || p.state === "failing") p.state = fail ? "failing" : "on";
      else p.state = fail ? "tried" : "ready";
      return { passed: !fail, exit: fail ? 1 : 0, tail: p.lastCheck.tail, state: p.state };
    },
    turn_on_pack: (a) => {
      const p = byId(a.id);
      if (p.state !== "ready") throw "the check has not passed against the current version; run it again first";
      Object.assign(p, { state: "on", by: "setup", since: Date.now() });
      return overview();
    },
    turn_off_pack: (a) => {
      const p = byId(a.id);
      Object.assign(p, { state: "off", by: "setup", since: Date.now() });
      return overview();
    },
  };
}
