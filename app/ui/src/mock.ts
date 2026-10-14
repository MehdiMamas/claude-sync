// A fake backend for `pnpm dev` in a plain browser (never bundled into the app).
// `?signedOut` shows the sign-in screen; `?empty` shows a user with no groups; `?plugins`
// shows the plugins reload notice; `?noSkills` shows a user with no skills to share;
// `?noHistory` shows a group with no recent changes. Avatars load from GitHub when
// online and fall back to initials offline, as in the app.
// `?noNotifications` starts with an empty notification history.
// `?noClaudeMd` shows a user whose CLAUDE.md has no text of their own.
// The first group has one job waiting for consent, one accepted job and a plugin warning.
import type { Backend, ClaudeMdShare, CommitInfo, Events, GroupContents, GroupId, GroupStatus, JobsOverview, JobView, Kinds, Notification, PersonalStatus } from "./api";

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

let notifications: Notification[] = q.has("noNotifications")
  ? []
  : [
      { id: 6, time: now - 40, title: "Invitation to a claude-sync group", body: "lina-k invited you to lina-k/prompts. Open claude-sync to accept.", kind: "group", group: { owner: "lina-k", repo: "prompts" }, read: false },
      { id: 5, time: now - 1500, title: "Plugins changed", body: "Plugins changed. Run /reload-plugins in open Claude Code sessions, or restart them.", kind: "plugins", group: groups[0]?.id ?? null, read: false },
      { id: 4, time: now - 3600, title: "Conflict in a shared file", body: "skills/review/SKILL.md (mehdimamas/claude-sync-data): the group's version won; yours was saved.", kind: "group", group: groups[0]?.id ?? null, read: true },
      { id: 3, time: now - 7200, title: "Local change to a shared file", body: "skills/review/SKILL.md was changed on this machine. Share it with mehdimamas/claude-sync-data?", kind: "local-edit", group: groups[0]?.id ?? null, read: true },
      { id: 2, time: now - 90000, title: "claude-sync update ready", body: "Version 0.2.0 installs when you quit, or restart now from Settings.", kind: "update", group: null, read: true },
      { id: 1, time: now - 200000, title: "Signed in", body: "claude-sync is signed in as mehdimamas.", kind: "other", group: null, read: true },
    ];
let nextNotification = 7;
let desktopNotifications = true;
const notificationListeners: ((p: Events["notifications-changed"]) => void)[] = [];
function notificationsChanged(): void {
  const unread = notifications.filter((n) => !n.read).length;
  for (const l of notificationListeners) l({ unread });
}

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
        { name: "setup-scout", description: "Reviews and refreshes this machine's Claude Code setup.", version: "0.1.0", enabled: true },
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

// `?personalOffer` shows a device that was offered personal memory; `?personalOff` one that has it off.
let personal: PersonalStatus = {
  enabled: !q.has("personalOffer") && !q.has("personalOff"),
  decided: q.has("personalOff"),
  offer: q.has("personalOffer"),
  kinds: { claudeMd: true, memory: true },
  repo: "mehdimamas/claude-sync-personal",
  repoUrl: "https://github.com/mehdimamas/claude-sync-personal",
  deviceId: "d1",
  devices: [
    { id: "d1", name: "desktop", os: "windows", lastSeenUnix: now - 60, thisDevice: true },
    { id: "d2", name: "macbook", os: "macos", lastSeenUnix: now - 7200, thisDevice: false },
  ],
  newDevicesJoin: q.has("personalOffer") ? false : true,
  lastSyncUnix: now - 90,
  lastError: null,
  conflicts: [{ path: "memory/home--projects-app/MEMORY.md", savedTo: "C:/Users/me/.claude-sync/app/conflicts/personal/1/MEMORY.md", atUnix: now - 600 }],
  skipped: ["memory/home--projects-app/dump.md: larger than 256 KB"],
};
// The user's own CLAUDE.md text, as `claude_md_preview` returns it (token already redacted).
const ownClaudeMd = `# How I work

- Answer in short, plain sentences.
- Run the tests before saying something works.
- My email is me@example.com; use it for store listings.
- Deploy key: [redacted:ghp]
`;
const sharedClaudeMd = new Set<string>();

const setPersonal = (patch: Partial<PersonalStatus>) => (personal = { ...personal, ...patch });

const mehdiJobs: JobsOverview = {
  unsupported: null,
  jobs: [
    {
      id: "setup-scout",
      title: "Claude setup refresh",
      cron: "17 10 * * 1,4",
      schedule: "Mon and Thu at 10:17",
      prompt: "/setup-scout:setup-scout unattended",
      cwd: "~",
      permissionMode: "bypassPermissions",
      timeoutMinutes: 60,
      report: "~/.claude/setup-scout/reports/latest.md",
      args: ["--chrome"],
      hash: "5d1c0b7e",
      accepted: false,
      changed: false,
      enabled: true,
      registered: false,
      error: null,
      lastRun: null,
      running: false,
      hasLog: false,
      hasReport: false,
    },
    {
      id: "weekly-digest",
      title: "Weekly digest of the group's changes",
      cron: "0 9 * * 1",
      schedule: "Mon at 09:00",
      prompt: "Summarise this week's commits in ~/.claude-sync/groups and write ~/digest.md.",
      cwd: "~",
      permissionMode: "acceptEdits",
      timeoutMinutes: 30,
      report: "~/digest.md",
      args: [],
      hash: "a90f22c1",
      accepted: true,
      changed: false,
      enabled: true,
      registered: true,
      error: null,
      lastRun: { startUnix: now - 86400 * 2, endUnix: now - 86400 * 2 + 140, exitCode: 0, status: "ok", manual: false, log: "C:/Users/me/.claude-sync/logs/jobs/weekly-digest/20261012T090000Z.log", report: "C:/Users/me/digest.md" },
      running: false,
      hasLog: true,
      hasReport: true,
    },
  ],
  problems: [{ group: { owner: "mehdimamas", repo: "claude-sync-data" }, id: "nightly", message: 'cron "0 3 1 * *": day of month and month must be *; only weekly schedules are supported' }],
  autoAccept: false,
};
const emptyJobs: JobsOverview = { unsupported: null, jobs: [], problems: [], autoAccept: false };
let pluginWarnings = ["couldn't install session-manager@claude-mods: it needs your confirmation (it declares commands); install it in a terminal with claude plugin install"];

function job(a: Record<string, unknown>): JobView {
  const j = mehdiJobs.jobs.find((x) => x.id === a.id);
  if (!j) throw `no job ${String(a.id)}`;
  return j;
}

// Newest first, as GitHub lists them. Messages are real ones from a group repo.
const H = 3600;
const D = 86400;
const commits: CommitInfo[] = (
  [
    ["MehdiMamas@DESKTOP-MEHDI: setup-scout: read X and LinkedIn through Chrome on scheduled runs too (read only); job passes --chrome so the scheduled run can use the browser extension when it is connected, and skips both sources quietly when it is not", "MehdiMamas", 600],
    ["sara-dev@Saras-MacBook.local: update CLAUDE.md", "sara-dev", 50 * 60],
    ["sara-dev@Saras-MacBook.local: update CLAUDE.md", "sara-dev", 54 * 60],
    ["sara-dev@saras-macbook.local: update CLAUDE.md", "sara-dev", 58 * 60],
    ["MehdiMamas@desktop-mehdi: add member ibrahim-ahidar", "MehdiMamas", 3 * H],
    ["yassine@yassine-mbp: share skills/pdf-tools", "yassine", D + 2 * H],
    ["MehdiMamas@DESKTOP-MEHDI: group memory: personal vs group scope, group-memory skill, mods are made claude-sync wide", "MehdiMamas", D + 5 * H],
    ["sara-dev@laptop: skills/review: tighten the checklist\n\nAsks for a test per bug fix and flags unused exports.", "sara-dev", 2 * D],
    ["yassine@yassine-mbp: update settings.shared.json", "yassine", 3 * D],
    ["MehdiMamas@DESKTOP-MEHDI: update plugins/claude-mods/mods/model-router/router.js, plugins/claude-mods/mods/model-router/README.md", "MehdiMamas", 4 * D],
    ["session-manager: 0.2.0", "MehdiMamas", 9 * D],
    ["yassine@yassine-mbp: remove agents/old-reviewer.md", "yassine", 10 * D],
    ["MehdiMamas@DESKTOP-MEHDI: create group (start the repo)", "MehdiMamas", 12 * D],
  ] as const
).map(([message, author, ago], i) => ({ sha: `${(0x3f2a9c1d + i * 7919).toString(16)}e0a7b55c`, message, author, atUnix: now - ago }));

const handlers: Record<string, (a: Record<string, unknown>) => unknown> = {
  personal_status: () => personal,
  personal_enable: () => setPersonal({ enabled: true, decided: true, offer: false }),
  personal_disable: () => setPersonal({ enabled: false, decided: true, offer: false }),
  personal_set_kinds: (a) => setPersonal({ kinds: a.kinds as Kinds }),
  personal_set_new_devices_join: (a) => setPersonal({ newDevicesJoin: a.join as boolean }),
  personal_remove_device: (a) => setPersonal({ devices: personal.devices.filter((d) => d.id !== a.id) }),
  personal_dismiss_offer: () => {
    setPersonal({ decided: true, offer: false });
  },
  app_info: () => ({ version: "0.1.0", os: "windows 11 (x86_64)", relayConfigured: true, clientIdMissing: false, stateDir: "C:/Users/me/.claude-sync/app" }),
  auth_status: () => ({ signedIn, login: signedIn ? "mehdimamas" : null, clientIdMissing: false }),
  start_sign_in: () => ({ userCode: "WDJB-MJHT", verificationUri: "https://github.com/login/device", expiresIn: 900 }),
  cancel_sign_in: () => null,
  sign_out: () => {
    signedIn = false;
  },
  status: () => ({ state: "ok", text: "Up to date", lastSyncUnix: now - 120, error: null }),
  groups: () => groups,
  create_group: (a) => {
    const share = a.claudeMd as ClaudeMdShare | undefined;
    if (share?.text.trim()) sharedClaudeMd.add(share.text.trim());
    const g: GroupStatus = {
      id: { owner: "mehdimamas", repo: (a.repo as string) || "claude-sync-data" },
      name: a.name as string,
      host: "mehdimamas",
      members: ["mehdimamas"],
      invited: [],
      lastSyncUnix: now,
      lastError: null,
      conflicts: [],
      collisions: [],
      pluginsChanged: false,
    };
    groups.push(g);
    return g;
  },
  invitations: () => (q.has("empty") ? [] : [{ id: 7, repo: { owner: "lina-k", repo: "prompts" }, inviter: "lina-k" }]),
  discover: () => (q.has("empty") ? [] : [{ owner: "yassine", repo: "claude-sync-data" }]),
  local_edits: () => (q.has("empty") ? [] : [{ group: groups[0].id, path: "skills/review/SKILL.md", deleted: false }]),
  local_skills: () => (q.has("noSkills") ? [] : ["my-notes", "pdf-tools", "release-checklist"]),
  claude_md_preview: () => (q.has("noClaudeMd") ? "" : ownClaudeMd),
  share_claude_md: (a) => {
    const text = (a.share as ClaudeMdShare).text.trim();
    if (!text) throw "there is no text to share";
    const appended = !sharedClaudeMd.has(text);
    sharedClaudeMd.add(text);
    return { appended, removedOwn: (a.share as ClaudeMdShare).removeOwn };
  },
  group_contents: (a) => ((a.group as GroupId).owner === "acme" ? acmeContents : mehdiContents),
  recent_commits: (a) => (q.has("noHistory") ? [] : commits.slice(0, Number(a.limit ?? 20))),
  sync_now: () => ({ groups: [] }),
  jobs: (a) => ((a.group as GroupId).owner === "acme" ? emptyJobs : mehdiJobs),
  accept_job: (a) => {
    Object.assign(job(a), { accepted: true, changed: false, enabled: true, registered: true });
    return mehdiJobs;
  },
  set_job_enabled: (a) => {
    Object.assign(job(a), { enabled: a.enabled, registered: a.enabled });
    return mehdiJobs;
  },
  set_jobs_auto_accept: (a) => {
    mehdiJobs.autoAccept = a.on as boolean;
    if (mehdiJobs.autoAccept) {
      for (const j of mehdiJobs.jobs.filter((x) => !x.accepted)) Object.assign(j, { accepted: true, changed: false, registered: j.enabled });
    }
    return mehdiJobs;
  },
  run_job_now: (a) => {
    const j = job(a);
    j.running = true;
    setTimeout(() => {
      Object.assign(j, { running: false, hasLog: true, lastRun: { startUnix: now, endUnix: now + 3, exitCode: 0, status: "ok", manual: true, log: "C:/Users/me/run.log", report: null } });
    }, 3000);
    return mehdiJobs;
  },
  open_job_file: () => null,
  plugin_warnings: (a) => ((a.group as GroupId).owner === "acme" ? [] : pluginWarnings),
  retry_plugins: () => {
    pluginWarnings = [];
    return pluginWarnings;
  },
  open_url: (a) => {
    window.open(String(a.url), "_blank", "noopener");
  },
  get_settings: () => ({ deviceName: "desktop", pollSeconds: 60, autostart: true, paused: false, desktopNotifications }),
  set_settings: (a) => {
    const s = a.settings as { desktopNotifications?: boolean };
    if (s.desktopNotifications !== undefined) desktopNotifications = s.desktopNotifications;
    return { deviceName: "desktop", pollSeconds: 60, autostart: true, paused: false, desktopNotifications, ...s };
  },
  notifications: () => notifications.map((n) => ({ ...n })),
  mark_notification_read: (a) => {
    for (const n of notifications) if (n.id === a.id) n.read = true;
    notificationsChanged();
  },
  mark_all_notifications_read: () => {
    for (const n of notifications) n.read = true;
    notificationsChanged();
  },
  clear_notifications: () => {
    notifications = [];
    notificationsChanged();
  },
  test_notification: () => {
    notifications.unshift({ id: nextNotification++, time: Math.floor(Date.now() / 1000), title: "Test notification", body: "Notifications from claude-sync show up like this.", kind: "other", group: null, read: false });
    notificationsChanged();
  },
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
    if (event === "notifications-changed") notificationListeners.push(cb as (p: Events["notifications-changed"]) => void);
    return () => {};
  },
};
