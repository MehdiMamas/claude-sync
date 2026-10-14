// A fake backend for `pnpm dev` in a plain browser (never bundled into the app).
// `?signedOut` shows the sign-in screen; `?empty` shows a user with no groups; `?plugins`
// shows the plugins reload notice; `?noSkills` shows a user with no skills to share.
import type { Backend, Events, GroupContents, GroupId, GroupStatus } from "./api";

const q = new URLSearchParams(location.search);
const now = Math.floor(Date.now() / 1000);
let signedIn = !q.has("signedOut");

const groups: GroupStatus[] = q.has("empty")
  ? []
  : [
      {
        id: { owner: "mehdimamas", repo: "claude-sync-data" },
        name: "Mehdi & friends",
        host: "mehdimamas",
        members: ["mehdimamas", "sara-dev", "yassine"],
        invited: ["octocat"],
        lastSyncUnix: now - 120,
        lastError: null,
        conflicts: [{ path: "skills/review/SKILL.md", savedTo: "C:/Users/me/.claude/skills/review/SKILL.conflict-laptop.md", atUnix: now - 3600 }],
        collisions: [{ kind: "userFile", path: "agents/scout.md" }],
        pluginsChanged: false,
      },
      {
        id: { owner: "acme", repo: "claude-team" },
        name: "Acme team",
        host: "acme-bot",
        members: ["acme-bot", "mehdimamas"],
        invited: [],
        lastSyncUnix: now - 400,
        lastError: null,
        conflicts: [],
        collisions: [],
        pluginsChanged: false,
      },
    ];

const mehdiContents: GroupContents = {
  marketplaces: [
    {
      name: "claude-mods",
      description: "Mehdi's Claude Code mods: model routing and managed worker sessions.",
      path: "C:/Users/me/.claude-sync/marketplaces/claude-mods",
      heldBy: null,
      plugins: [
        { name: "model-router", description: "Routes turns, subagents and worker sessions to cheaper models when the work allows it.", version: "0.1.0", enabled: true },
        { name: "session-manager", description: "Replaces subagents with a managed hierarchy of background sessions.", version: "0.3.2", enabled: false },
      ],
    },
  ],
  skills: [{ name: "review", description: "Reviews a diff for bugs before a PR." }],
  agents: [],
  commands: [],
  outputStyles: [],
  settings: {
    keys: ["permissions", "enabledPlugins", "model"],
    enabledPlugins: [
      ["model-router@claude-mods", true],
      ["session-manager@claude-mods", false],
    ],
    keptLocal: false,
  },
  claudeMd: "C:/Users/me/.claude-sync/groups/mehdimamas__claude-sync-data/CLAUDE.md",
};

const acmeContents: GroupContents = {
  marketplaces: [{ name: "claude-mods", description: null, path: null, heldBy: { owner: "mehdimamas", repo: "claude-sync-data" }, plugins: [] }],
  skills: [],
  agents: [],
  commands: [],
  outputStyles: [],
  settings: { keys: [], enabledPlugins: [], keptLocal: false },
  claudeMd: null,
};

const handlers: Record<string, (a: Record<string, unknown>) => unknown> = {
  app_info: () => ({ version: "0.1.0", os: "windows 11 (x86_64)", relayConfigured: true, clientIdMissing: false, stateDir: "C:/Users/me/.claude-sync/app" }),
  auth_status: () => ({ signedIn, login: signedIn ? "mehdimamas" : null, clientIdMissing: false }),
  start_sign_in: () => ({ userCode: "WDJB-MJHT", verificationUri: "https://github.com/login/device", expiresIn: 900 }),
  cancel_sign_in: () => null,
  sign_out: () => {
    signedIn = false;
  },
  status: () => ({ state: "ok", text: "Up to date", lastSyncUnix: now - 120, error: null }),
  groups: () => groups,
  invitations: () => (q.has("empty") ? [] : [{ id: 7, repo: { owner: "lina-k", repo: "prompts" }, inviter: "lina-k" }]),
  discover: () => (q.has("empty") ? [] : [{ owner: "yassine", repo: "claude-sync-data" }]),
  local_edits: () => (q.has("empty") ? [] : [{ group: groups[0].id, path: "skills/review/SKILL.md", deleted: false }]),
  local_skills: () => (q.has("noSkills") ? [] : ["my-notes", "pdf-tools", "release-checklist"]),
  group_contents: (a) => ((a.group as GroupId).owner === "acme" ? acmeContents : mehdiContents),
  recent_commits: () => [
    { sha: "3f2a9c1d0e", message: "skills/review: tighten checklist", author: "sara-dev (laptop)", atUnix: now - 900 },
    { sha: "a81be22f77", message: "share skill pdf-tools", author: "mehdimamas (desktop)", atUnix: now - 7200 },
    { sha: "09cd11aa42", message: "settings: add permissions", author: "yassine (mac)", atUnix: now - 90000 },
  ],
  sync_now: () => ({ groups: [] }),
  get_settings: () => ({ deviceName: "desktop", pollSeconds: 60, autostart: true, paused: false }),
  set_settings: (a) => ({ deviceName: "desktop", pollSeconds: 60, autostart: true, paused: false, ...(a.settings as object) }),
  feedback_logs: () => "2026-10-13T10:00:01Z INFO sync: 2 groups up to date\n2026-10-13T10:01:01Z INFO sync: pulled mehdimamas/claude-sync-data",
  update_status: () => ({ current: "0.1.0", available: !q.has("empty"), version: "0.2.0", notes: null, downloaded: true }),
  check_update: () => ({ current: "0.1.0", available: false, version: null, notes: null, downloaded: false }),
};

export const backend: Backend = {
  async invoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
    await new Promise((r) => setTimeout(r, 150));
    const f = handlers[cmd];
    if (!f) throw `${cmd}: not available in the browser preview`;
    return f(args) as T;
  },
  async listen<K extends keyof Events>(event: K, cb: (p: Events[K]) => void) {
    if (event === "plugins-changed" && q.has("plugins")) setTimeout(() => cb(null as Events[K]), 300);
    return () => {};
  },
};
