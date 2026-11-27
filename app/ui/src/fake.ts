// The fake backend of the dev server and the Playwright runs: the same `send`, `snapshot` and
// `open_target` interface as the shell, over the generated types. Sample data only.
import type { Backend } from "./backend";
import type { Command } from "./generated/Command";
import type { EditView } from "./generated/EditView";
import type { Event } from "./generated/Event";
import type { GroupStatus } from "./generated/GroupStatus";
import type { GroupView } from "./generated/GroupView";
import type { HistoryItem } from "./generated/HistoryItem";
import type { JobView } from "./generated/JobView";
import type { PackView } from "./generated/PackView";
import type { ProposalView } from "./generated/ProposalView";
import type { ReviewLine } from "./generated/ReviewLine";
import type { Snapshot } from "./generated/Snapshot";

const sec = () => Math.floor(Date.now() / 1000);

const group = (repo: string, over: Partial<GroupView> = {}): GroupView => ({
  repo,
  name: repo.split("/")[1] ?? repo,
  host: repo.split("/")[0] ?? "",
  isHost: false,
  status: "synced",
  error: null,
  lastSync: sec() - 120,
  members: [],
  proposalMode: false,
  autoAcceptClaudeMd: true,
  jobsAutoAccept: false,
  hostLock: null,
  reviewChoice: "followGroup",
  pendingReview: 0,
  pluginProblems: [],
  syncing: false,
  failedAt: null,
  off: false,
  pendingInvites: [],
  edits: [],
  ...over,
});
const many = (n: number) => Array.from({ length: n }, (_, i) => `member-${i + 1}`);

const baseGroups = (): GroupView[] => [
  group("alex-host/claude-group", {
    isHost: true,
    members: many(4),
    pendingReview: 3,
    hostLock: "on",
  }),
  group("sample-org/dev-rules", { members: many(9) }),
  group("sample-org/old-tools", { status: "off", off: true, lastSync: null }),
];

const base = (): Snapshot => ({
  login: "alex-host",
  signIn: { kind: "signedOut" },
  sessionExpired: null,
  appMode: false,
  paused: false,
  intervalSecs: 60,
  syncing: false,
  rateLimitedUntil: null,
  groups: baseGroups(),
  invitations: [
    { id: 7, repo: "sample-org/dev-rules-2", inviter: "sam-member", createdAt: sec() - 86400 },
  ],
  discoverable: [
    { repo: "sample-org/dev-rules", members: 9 },
    { repo: "sample-org/ml-team", members: 3 },
  ],
  personalOn: false,
  personal: {
    on: false,
    offer: false,
    repo: "alex-host/claude-sync-personal",
    newDevicesJoin: true,
    kindsOff: [],
    devices: [
      { id: "d1", name: "Laptop", os: "macOS", lastSeen: sec(), this: true },
      { id: "d2", name: "Desktop", os: "Windows", lastSeen: sec() - 3600, this: false },
    ],
    stays: [],
    error: null,
    lastSync: null,
  },
  unread: 2,
  notifications: [],
  jobs: [],
  sshOn: false,
  activityOn: false,
  update: "",
  updateChecked: sec() - 3600,
  appUncovered: [],
  desktopNotifications: true,
  helper: true,
  startAtLogin: true,
  install: null,
});

const code = {
  kind: "code",
  user_code: "WDJB-MJHT",
  verification_uri: "https://github.com/login/device",
  expires_in: 900,
} as const;

const job = (repo: string, over: Partial<JobView> = {}): JobView => ({
  repo,
  id: "nightly-lint",
  accepted: false,
  enabled: false,
  lastResult: "",
  lastRun: 0,
  title: "nightly-lint",
  cron: "0 3 * * *",
  prompt: "Lint the repo",
  cwd: "~/work",
  mode: "plan",
  timeoutMinutes: 10,
  args: [],
  ...over,
});

const note = (id: number, kind: string, text: string, repo: string | null) => ({
  id,
  kind,
  text,
  read: false,
  time: sec() - 600,
  repo,
});

/** The state each `?scenario=` starts in. */
function scenario(name: string): Snapshot {
  const s = base();
  if (name !== "groups-invite") s.invitations = [];
  const setStatus = (i: number, status: GroupStatus, over: Partial<GroupView> = {}) => {
    s.groups[i] = { ...(s.groups[i] as GroupView), status, ...over };
  };
  const setProblems = (i: number, pluginProblems: string[]) =>
    setStatus(i, (s.groups[i] as GroupView).status, { pluginProblems });
  switch (name) {
    case "signed-out":
      return { ...s, login: null, groups: [], unread: 0 };
    case "sign-in-code":
      return { ...s, login: null, groups: [], signIn: code };
    case "sign-in-expired":
      return { ...s, login: null, sessionExpired: "alex-host" };
    case "sign-in-code-expired":
      return { ...s, login: null, signIn: { kind: "expired" } };
    case "sign-in-denied":
      return { ...s, login: null, signIn: { kind: "denied" } };
    case "sign-in-offline":
      return { ...s, login: null, signIn: { kind: "offline" } };
    case "sign-in-unavailable":
      return { ...s, login: null, signIn: { kind: "unavailable" } };
    case "install-import-failed":
      return {
        ...s,
        login: null,
        groups: [],
        unread: 0,
        install: { kind: "skipped", files: ["jobs.json"], total: 9 },
      };
    case "install-v1-daemon":
      return {
        ...s,
        login: null,
        groups: [],
        unread: 0,
        install: { kind: "daemon", pid: 4182, version: "0.1.27" },
      };
    case "notifications":
      s.notifications = [
        note(1, "review", "alex-host/claude-group: 3 lines to review", "alex-host/claude-group"),
        note(2, "invitation", "sample-org/dev-rules invited you", null),
        note(3, "update", "Version 2.0.1 installs when the app is idle", null),
      ];
      s.unread = 3;
      return s;
    case "notifications-plain":
      s.notifications = [
        note(1, "feedback", '"Group did not sync" was filed as #41.', null),
        note(2, "signin", "GitHub no longer accepts your sign-in.", null),
      ];
      s.unread = 2;
      return s;
    case "notifications-empty":
      return { ...s, unread: 0 };
    case "groups-empty":
      return { ...s, groups: [], invitations: [], unread: 0 };
    case "groups-loading":
      return { ...s, groups: [], syncing: true };
    case "groups-syncing":
      s.syncing = true;
      setStatus(0, "syncing");
      return s;
    case "groups-offline":
      setStatus(0, "offline", { lastSync: sec() - 40 * 60 });
      s.invitations = [];
      return s;
    case "groups-rate-limited": {
      const t = new Date();
      t.setHours(14, 5, 0, 0);
      s.rateLimitedUntil = Math.floor(t.getTime() / 1000);
      setStatus(0, "rateLimited");
      s.invitations = [];
      return s;
    }
    case "groups-sync-error":
      setStatus(0, "failed", { error: "GitHub answered 500", lastSync: sec() - 60 });
      s.invitations = [];
      return s;
    case "groups-plugins":
      s.invitations = [];
      setProblems(0, [
        "couldn't install repo-tools@mods: registry unreachable",
        "plugins/mods/big-pack.bin is over the 1 MiB limit and was not placed",
      ]);
      return s;
    case "history":
    case "helper":
    case "helper-unavailable":
    case "proposals":
    case "proposals-one":
    case "group":
      s.jobs = [job(s.groups[0]?.repo ?? "")];
      setProblems(0, ["couldn't install big-pack@mods: registry unreachable"]);
      return s;
    case "jobs":
      s.jobs = [
        job(s.groups[0]?.repo ?? ""),
        job(s.groups[0]?.repo ?? "", {
          id: "weekly-report",
          title: "weekly-report",
          accepted: true,
          enabled: true,
          cron: "0 9 * * 1",
          lastResult: "ok",
          lastRun: sec() - 7200,
        }),
      ];
      return s;
    case "jobs-edge": {
      const r = s.groups[0]?.repo ?? "";
      s.jobs = [
        job(r, {
          id: "risky",
          title: "nightly-lint",
          cron: "0 2 * * *",
          timeoutMinutes: 30,
          prompt: "Run the linter on this repo and list what fails.",
          mode: "bypassPermissions",
          args: ["--chrome", "--model", "sonnet"],
        }),
        job(r, {
          id: "flaky",
          title: "flaky",
          accepted: true,
          enabled: true,
          lastResult: "failed",
          lastRun: sec() - 600,
        }),
        job(r, {
          id: "offline",
          title: "offline",
          accepted: true,
          enabled: true,
          lastResult: "skipped: offline",
          lastRun: sec() - 3600,
        }),
        job(r, { id: "idle", title: "idle", accepted: true, enabled: false }),
      ];
      return s;
    }
    case "members":
    case "members-proposal":
      setStatus(0, "synced", {
        members: ["alex-host", "sam-member", "kim-dev"],
        pendingInvites: ["kim-dev"],
      });
      return s;
    case "members-solo":
      setStatus(0, "synced", { members: ["alex-host"] });
      return s;
    case "members-guest":
    case "review-guest":
      setStatus(1, "synced", {
        host: "sam-member",
        members: ["sam-member", "alex-host", "kim-dev"],
        proposalMode: name === "members-guest",
        pendingReview: 1,
      });
      return s;
    case "no-history":
      setStatus(0, "synced", { members: ["alex-host"], pendingReview: 0 });
      return s;
    case "propose":
      setStatus(1, "synced", { host: "sam-member", pendingReview: 1, proposalMode: true });
      return s;
    case "proposals-guest":
    case "proposals-guest-one":
    case "group-member":
      setStatus(1, "synced", { host: "sam-member", pendingReview: 1 });
      s.jobs = [job(s.groups[1]?.repo ?? "")];
      return s;
    case "group-off":
      setStatus(0, "off", { off: true, lastSync: null });
      return s;
    case "groups-all-off":
      s.groups = s.groups.slice(0, 2).map((g) => ({ ...g, status: "off" as const, off: true }));
      s.invitations = [];
      return s;
    case "settings-paused":
      s.paused = true;
      return s;
    case "settings-offer":
      s.personal.offer = true;
      return s;
    case "settings-personal":
    case "settings-public":
      s.personalOn = true;
      s.personal.on = true;
      s.personal.lastSync = sec() - 120;
      s.personal.stays = ["memory/big-notes.md: not synced, it is over 256 KiB"];
      if (name === "settings-public")
        s.personal.error =
          "Stopped: alex-host/claude-sync-personal is public now. Nothing more is pushed until it is private again.";
      return s;
    case "settings-app":
      s.appMode = true;
      s.appUncovered = ["sample-org/dev-rules"];
      return s;
    case "settings-ssh":
      s.sshOn = true;
      return s;
    case "update-ready":
      s.update = "ready:2.0.1";
      return s;
    case "update-error":
      s.update = "failed:2.0.1";
      return s;
    case "update-updated":
      s.update = "updated:2.0.1";
      return s;
    default:
      return s;
  }
}

export type FakeControls = {
  /** Finish a held command (see `?hold`). */
  release(): void;
  /** What the engine does on a tick: change the snapshot and tell the window. */
  patch(p: Partial<Snapshot>): void;
  /** The person approves, or the sign-in ends another way. */
  approve(): void;
  /** The new owner accepted the transfer on GitHub: the repo moves to them. */
  transferred(login: string): void;
  end(kind: "expired" | "denied" | "offline"): void;
  /** The personal repo is private again. */
  fixed(): void;
};

export function fakeBackend(search: string): Backend {
  const q = new URLSearchParams(search);
  let name = q.get("scenario") ?? "groups-invite";
  if (q.has("signedOut")) name = "signed-out";
  if (q.has("empty")) name = "groups-empty";
  if (q.has("noHistory")) name = "no-history";
  let snap = scenario(name);
  const subs = new Set<(s: Snapshot) => void>();
  const emit = () => {
    for (const f of subs) f(structuredClone(snap));
  };
  const set = (p: Partial<Snapshot>) => {
    snap = { ...snap, ...p };
    emit();
  };

  let holding = q.has("hold");
  const waiting: Array<() => void> = [];
  const gate = () => (holding ? new Promise<void>((r) => waiting.push(r)) : sleep(150));
  const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

  const controls: FakeControls = {
    release() {
      holding = false;
      for (const r of waiting.splice(0)) r();
    },
    patch: set,
    approve() {
      set({ login: "alex-host", signIn: { kind: "signedOut" }, groups: [], syncing: true });
      setTimeout(() => set({ syncing: false, groups: baseGroups() }), 1500);
    },
    transferred(login) {
      set({
        groups: snap.groups.map((g) =>
          g.isHost
            ? {
                ...g,
                repo: `${login}/${g.name}`,
                host: login,
                isHost: false,
                pendingReview: 0,
                lastSync: sec(),
              }
            : g,
        ),
      });
    },
    end(kind) {
      set({ signIn: { kind } });
    },
    fixed() {
      set({ personal: { ...snap.personal, error: null } });
    },
  };
  (window as unknown as { __fake: FakeControls }).__fake = controls;

  const done: Event = { kind: "done" };
  // Local edits per group: the host has two, the member one.
  const edits: Record<string, EditView[]> = {
    "alex-host/claude-group": [
      { path: "skills/release-notes/SKILL.md", kept: false },
      { path: "settings.json", kept: false },
    ],
    "sample-org/dev-rules": [
      {
        path: `skills/${name === "propose" ? "test-runner" : "release-notes"}/SKILL.md`,
        kept: false,
      },
    ],
  };
  // The lines waiting for review per group: the host's three, a member's one.
  const line = (
    id: string,
    add: boolean,
    text: string,
    author: string,
    ago: number,
  ): ReviewLine => ({
    id,
    add,
    lines: [text],
    author,
    at: sec() - ago,
  });
  const review: Record<string, ReviewLine[]> = {};
  for (const g of snap.groups) {
    review[g.repo] =
      g.pendingReview === 3
        ? [
            line("l1", true, "Prefer small pull requests.", "sam-member", 3600),
            line("l2", true, "Run the tests before a commit.", "sam-member", 3600),
            line("l3", false, "Use tabs.", "sam-member", 3600),
          ]
        : g.pendingReview === 1
          ? [
              name === "review-guest"
                ? line("l1", true, "Answer in short bullets.", "alex-host", 3 * 3600)
                : line("l1", true, "Prefer small pull requests.", "sam-member", 3600),
            ]
          : [];
  }
  let conflicts = [
    { repo: "alex-host/claude-group", path: "skills/notes/SKILL.md", savedAt: sec() - 86400 },
  ];
  let clash = q.has("clash");
  // Open proposals per group: the host's list and the member's list, as the scenario names them.
  const prop = (number: number, author: string, title: string, path: string, over = {}) => ({
    number,
    branch: `proposal/${author}-20261126100000`,
    author,
    title,
    url: `https://github.com/sample-org/dev-rules/pull/${number}`,
    files: [{ path, patch: "" }],
    approvals: [] as string[],
    needsUpdate: false,
    mine: author === "alex-host",
    approvedByMe: false,
    head: "abc1234",
    created: null as string | null,
    ...over,
  });
  const props: Record<string, ProposalView[]> = {};
  if (name.startsWith("proposals")) {
    props["alex-host/claude-group"] = [
      prop(12, "sam-member", "Add test-runner skill", "skills/test-runner/SKILL.md", {
        approvals: ["kim-dev"],
        created: new Date((sec() - 7200) * 1000).toISOString(),
      }),
      prop(13, "kim-dev", "Shorter commit rule", "CLAUDE.md", { needsUpdate: true }),
      prop(14, "alex-host", "Add lint agent", "agents/lint.md"),
    ].slice(0, name === "proposals-one" ? 1 : 3);
    props["sample-org/dev-rules"] = [
      prop(13, "kim-dev", "Shorter commit rule", "CLAUDE.md"),
      prop(12, "alex-host", "Add test-runner skill", "skills/test-runner/SKILL.md"),
    ].slice(0, name === "proposals-guest-one" ? 1 : 2);
  } else {
    props["alex-host/claude-group"] = [
      prop(12, "sam-member", "Add test-runner skill", "skills/test-runner/SKILL.md"),
    ];
    props["sample-org/dev-rules"] = [
      prop(13, "kim-dev", "Shorter commit rule", "CLAUDE.md"),
      prop(12, "alex-host", "Add test-runner skill", "skills/test-runner/SKILL.md"),
    ];
  }
  /** Approve (a member approves, the host merges) or decline one proposal, or every other's. */
  const decideProposals = (repo: string, approve: boolean, number?: number): Event => {
    const host = snap.groups.find((g) => g.repo === repo)?.isHost;
    props[repo] = (props[repo] ?? []).flatMap((p) => {
      if (!approve) return number === undefined || p.number === number ? [] : [p];
      if ((number !== undefined && p.number !== number) || p.mine) return [p];
      if (p.needsUpdate && number === undefined) return [p];
      if (host) return [];
      return [{ ...p, approvals: [...p.approvals, snap.login ?? ""], approvedByMe: true }];
    });
    return done;
  };
  const patchGroup = (repo: string, p: Partial<GroupView>) =>
    set({ groups: snap.groups.map((g) => (g.repo === repo ? { ...g, ...p } : g)) });
  const secret = "secret found in settings.json:12 (API key); nothing was committed";
  const addGroup = (repo: string, over: Partial<GroupView> = {}) =>
    set({ groups: [...snap.groups, group(repo, { members: many(3), ...over })] });

  const pack = (id: string, over: Partial<PackView>): PackView => ({
    repo: "alex-host/claude-group",
    id,
    title: id,
    summary: "",
    state: "off",
    unsupported: false,
    os: ["windows", "macos", "linux"],
    checkedAt: null,
    tail: "",
    ...over,
  });
  const packs: PackView[] = [
    pack("browser-tools", { state: "on", tail: "ready", checkedAt: 1_700_000_000 }),
    pack("mac-signing", { unsupported: true, os: ["macos"] }),
    pack("gpu-tools", { state: "failed", tail: "step 2 of 3: download timed out" }),
  ];

  async function send(cmd: Command): Promise<Event> {
    switch (cmd.kind) {
      case "signInStart":
        if (name === "sign-in-unavailable") return done;
        set({ signIn: { kind: "starting" } });
        await sleep(60);
        set({ signIn: code });
        return done;
      case "appCoverage":
        return done;
      case "signInCancel":
        set({ signIn: { kind: "signedOut" } });
        return done;
      case "signOut":
        set({ login: null, groups: [] });
        return { kind: "signedOut" };
      case "pause":
      case "resume":
        set({ paused: cmd.kind === "pause" });
        return done;
      case "setInterval":
        set({ intervalSecs: cmd.data.secs });
        return done;
      case "startAtLogin":
        set({ startAtLogin: cmd.data.on });
        return done;
      case "testNotification":
        set({
          unread: snap.unread + 1,
          notifications: [
            {
              ...note(
                Date.now(),
                "test",
                "Notifications from claude-sync show up like this.",
                null,
              ),
              time: sec(),
            },
            ...snap.notifications,
          ],
        });
        return done;
      case "markRead":
      case "markAllRead":
      case "clearNotifications": {
        const list =
          cmd.kind === "clearNotifications"
            ? []
            : snap.notifications.map((n) =>
                cmd.kind === "markAllRead" || n.id === cmd.data.id ? { ...n, read: true } : n,
              );
        set({ notifications: list, unread: list.filter((n) => !n.read).length });
        return done;
      }
      case "refresh":
        set({ install: null });
        return done;
      case "recentLogs":
        return { kind: "text", data: "INFO sync pass done: nothing changed" };
      case "sendFeedback": {
        await sleep(150);
        const w = window as unknown as { __sent?: unknown[] };
        w.__sent = [...(w.__sent ?? []), cmd.data];
        if (name === "feedback-queued" || name === "feedback-limited")
          return {
            kind: "feedback",
            data: { queued: true, limited: name === "feedback-limited", number: null, url: null },
          };
        if (name === "feedback-refused")
          throw new Error(
            "GitHub refused the issue (403: your account cannot open issues on this repo)",
          );
        if (name === "feedback-large") throw new Error("request body is too large");
        return {
          kind: "feedback",
          data: { queued: false, limited: false, number: 41, url: "https://example.test/i/41" },
        };
      }
      case "helper":
        set({ helper: cmd.data.on });
        return done;
      case "desktopNotifications":
        set({ desktopNotifications: cmd.data.on });
        return done;
      case "personalOn":
        set({
          personalOn: true,
          personal: { ...snap.personal, on: true, offer: false, lastSync: sec() },
        });
        return done;
      case "personalOff":
        set({ personalOn: false, personal: { ...snap.personal, on: false } });
        return done;
      case "personalDismiss":
        set({ personal: { ...snap.personal, offer: false } });
        return done;
      case "personalJoinAuto":
        set({ personal: { ...snap.personal, newDevicesJoin: cmd.data.on } });
        return done;
      case "personalKind": {
        const off = snap.personal.kindsOff.filter((k) => k !== cmd.data.kind);
        set({
          personal: { ...snap.personal, kindsOff: cmd.data.on ? off : [...off, cmd.data.kind] },
        });
        return done;
      }
      case "deviceRename":
      case "deviceRemove": {
        const c = cmd;
        set({
          personal: {
            ...snap.personal,
            devices: snap.personal.devices.flatMap((d) =>
              d.id !== c.data.id
                ? [d]
                : c.kind === "deviceRemove"
                  ? []
                  : [{ ...d, name: c.data.name }],
            ),
          },
        });
        return done;
      }
      case "conflicts":
        return { kind: "conflicts", data: conflicts };
      case "conflictOpen":
        return {
          kind: "path",
          data: `/home/me/.claude-sync/state/conflicts/${cmd.data.path}`,
        };
      case "conflictDelete":
        conflicts = conflicts.filter(
          (c) => !(c.repo === cmd.data.repo && c.path === cmd.data.path),
        );
        return done;
      case "sshSet":
        set({ sshOn: cmd.data.on });
        return done;
      case "activitySet":
        set({ activityOn: cmd.data.on });
        return done;
      case "packs":
        return { kind: "packs", data: structuredClone(packs) };
      case "packOn":
      case "packOff":
      case "packSetup": {
        const d = cmd.data;
        const p = packs.find((x) => x.id === d.id);
        if (p) {
          p.state = cmd.kind === "packOff" ? "off" : "on";
          p.tail = cmd.kind === "packOff" ? "" : "ready";
        }
        return done;
      }
      case "checkUpdate": {
        // Up to date finds 2.0.1, a failed download tries again and gets it.
        const v =
          snap.update === "" || snap.update.startsWith("failed") ? "ready:2.0.1" : snap.update;
        set({ update: v, updateChecked: sec() });
        return { kind: "text", data: v };
      }
      case "syncNow": {
        set({
          syncing: true,
          groups: snap.groups.map((g) =>
            g.status === "off" ? g : { ...g, status: "syncing" as const },
          ),
        });
        await gate();
        set({
          syncing: false,
          groups: snap.groups.map((g, i) =>
            g.status === "off"
              ? g
              : {
                  ...g,
                  status: "synced" as const,
                  error: null,
                  lastSync: i === 0 ? sec() : g.lastSync,
                },
          ),
        });
        return {
          kind: "synced",
          data: { files: 2, failed: 0, repos: [snap.groups[0]?.repo ?? ""] },
        };
      }
      case "pluginsRetry":
        set({
          groups: snap.groups.map((g) =>
            g.repo === cmd.data.repo ? { ...g, pluginProblems: [] } : g,
          ),
        });
        return done;
      case "reorderGroups":
        set({
          groups: cmd.data
            .map((r) => snap.groups.find((g) => g.repo === r))
            .filter((g): g is GroupView => g !== undefined),
        });
        return done;
      case "setGroupOff":
        set({
          groups: snap.groups.map((g) =>
            g.repo === cmd.data.repo
              ? {
                  ...g,
                  off: cmd.data.off,
                  status: cmd.data.off ? "off" : "synced",
                  lastSync: cmd.data.off ? null : sec(),
                }
              : g,
          ),
        });
        return done;
      case "createGroup":
        if (snap.groups.some((g) => g.repo === cmd.data.repo))
          throw `You already have a repo named ${cmd.data.name}. Pick another name.`;
        addGroup(cmd.data.repo, { isHost: true, members: [snap.login ?? ""] });
        return {
          kind: "created",
          data: { repo: cmd.data.repo, lock: q.has("noLock") ? "unavailable" : "on" },
        };
      case "discover":
        return { kind: "discovered", data: snap.discoverable };
      case "join":
        await gate();
        if (cmd.data.repo.endsWith("/ml-team")) throw "GitHub says you have no access (404).";
        set({ discoverable: snap.discoverable.filter((d) => d.repo !== cmd.data.repo) });
        addGroup(cmd.data.repo);
        return { kind: "joined", data: { repo: cmd.data.repo, warning: "" } };
      case "acceptInvitation": {
        const inv = snap.invitations.find((v) => v.id === cmd.data.id);
        await gate();
        set({ invitations: snap.invitations.filter((v) => v.id !== cmd.data.id) });
        if (inv) addGroup(inv.repo);
        return { kind: "joined", data: { repo: inv?.repo ?? "", warning: "" } };
      }
      case "leave":
        set({ groups: snap.groups.filter((g) => g.repo !== cmd.data.repo) });
        return done;
      case "addMember": {
        const who = cmd.data.login;
        if (who === "no-such-user" || who === "kim-devv")
          throw `No GitHub user named ${who}. Nothing was sent.`;
        const g = snap.groups.find((x) => x.repo === cmd.data.repo);
        if (g)
          patchGroup(g.repo, {
            members: [...g.members, who],
            pendingInvites: [...g.pendingInvites, who],
          });
        return { kind: "message", data: `Invited ${who}` };
      }
      case "removeMember": {
        const g = snap.groups.find((x) => x.repo === cmd.data.repo);
        if (g)
          patchGroup(g.repo, {
            members: g.members.filter((m) => m !== cmd.data.login),
            pendingInvites: g.pendingInvites.filter((m) => m !== cmd.data.login),
          });
        return done;
      }
      case "transfer":
        return {
          kind: "message",
          data: `Asked ${cmd.data.new_owner} to accept the group. Nothing changes until they do.`,
        };
      case "setProposalMode":
        patchGroup(cmd.data.repo, { proposalMode: cmd.data.on });
        return done;
      case "setAutoAccept":
        patchGroup(cmd.data.repo, { autoAcceptClaudeMd: cmd.data.on });
        return done;
      case "setReviewChoice":
        patchGroup(cmd.data.repo, { reviewChoice: cmd.data.choice });
        return done;
      case "reviewList":
        return { kind: "review", data: structuredClone(review[cmd.data.repo] ?? []) };
      case "reviewDecide": {
        const left = (review[cmd.data.repo] ?? []).filter(
          (l) => cmd.data.ids !== null && !cmd.data.ids.includes(l.id),
        );
        review[cmd.data.repo] = left;
        patchGroup(cmd.data.repo, { pendingReview: left.length });
        return done;
      }
      case "clashes":
        return {
          kind: "clashes",
          data: clash
            ? [
                {
                  kind: "contradiction",
                  target: null,
                  a: { source: null, name: "Your CLAUDE.md", text: "Use two spaces." },
                  b: {
                    source: snap.groups[0]?.repo ?? "",
                    name: snap.groups[0]?.name ?? "",
                    text: "Use tabs.",
                  },
                },
              ]
            : [],
        };
      case "dropLine":
      case "editLine":
        clash = false;
        return done;
      case "useGroupVersion":
        return done;
      case "explain":
        if (q.has("noClaude") || name === "helper-unavailable")
          throw "Claude is not signed in on this machine. Run claude, sign in, then try again.";
        return {
          kind: "text",
          data:
            cmd.data.task === "activity"
              ? "sam-member added two working rules: keep pull requests small and run the tests before committing. The tabs rule was removed."
              : "Adds two working rules and removes the tabs rule. Safe to accept.",
        };
      case "proposals":
        return { kind: "proposals", data: structuredClone(props[cmd.data.repo] ?? []) };
      case "proposalApprove":
      case "proposalDecline":
        return decideProposals(cmd.data.repo, cmd.kind === "proposalApprove", cmd.data.number);
      case "proposalsAcceptAll":
      case "proposalsDeclineAll":
        return decideProposals(cmd.data.repo, cmd.kind === "proposalsAcceptAll");
      case "history": {
        const t = (ago: number) => new Date((sec() - ago) * 1000).toISOString();
        const item = (sha: string, summary: string, author: string, ago: number): HistoryItem => ({
          sha,
          summary,
          author,
          at: t(ago),
          issues: [],
          before: null,
          now: null,
        });
        const all: HistoryItem[] = [
          {
            ...item("a1", "Prefer small pull requests", "sam-member", 3600),
            issues: name === "helper-unavailable" ? [] : [12],
            before: name === "helper-unavailable" ? null : "no rule on pull request size.",
            now: name === "helper-unavailable" ? null : "keep pull requests small.",
          },
          item("b2", "Add release-notes skill", snap.login ?? "", 86400),
          item("c3", "Create group", snap.login ?? "", 3 * 86400),
          item("d4", "Initial commit", snap.login ?? "", 4 * 86400),
        ];
        const some = name === "helper-unavailable" ? all.slice(0, 1) : all;
        return {
          kind: "history",
          data: name === "no-history" ? [] : some.slice(0, cmd.data.limit),
        };
      }
      case "revertPreview":
        return {
          kind: "revertPlan",
          data: {
            sha: cmd.data.sha,
            summary: "Prefer small pull requests",
            files: [{ path: "CLAUDE.md", action: "restore", blocked: null }],
          },
        };
      case "revert":
        return done;
      case "editsList":
        return { kind: "edits", data: structuredClone(edits[cmd.data.repo] ?? []) };
      case "keepMine":
        for (const e of edits[cmd.data.repo] ?? []) if (e.path === cmd.data.path) e.kept = true;
        return done;
      case "undoEdit":
        edits[cmd.data.repo] = (edits[cmd.data.repo] ?? []).filter((e) => e.path !== cmd.data.path);
        return done;
      case "sharePreview":
        return cmd.data.path === "settings.json"
          ? {
              kind: "sharePreview",
              data: { added: ['"env": { "SAMPLE_KEY": "sk-…redacted" }'], secret },
            }
          : {
              kind: "sharePreview",
              data: {
                added: ["Write release notes in plain words.", "One line per change."],
                secret: null,
              },
            };
      case "shareEdit":
        if (cmd.data.path === "settings.json") throw secret;
        await gate();
        edits[cmd.data.repo] = (edits[cmd.data.repo] ?? []).filter((e) => e.path !== cmd.data.path);
        return snap.groups.find((g) => g.repo === cmd.data.repo)?.proposalMode
          ? { kind: "proposed", data: 14 }
          : done;
      case "jobsAutoAccept":
        await gate();
        patchGroup(cmd.data.repo, { jobsAutoAccept: cmd.data.on });
        return done;
      case "jobAccept":
      case "jobEnable": {
        await gate();
        const { repo, id } = cmd.data;
        const on = cmd.kind === "jobAccept" ? true : cmd.data.on;
        set({
          jobs: snap.jobs.map((j) =>
            j.repo === repo && j.id === id ? { ...j, accepted: true, enabled: on } : j,
          ),
        });
        return done;
      }
      case "jobRunNow": {
        const d = cmd.data;
        setTimeout(
          () =>
            set({
              jobs: snap.jobs.map((j) =>
                j.id === d.id ? { ...j, lastResult: "ok", lastRun: sec() } : j,
              ),
            }),
          400,
        );
        return done;
      }
      case "jobFile":
        return {
          kind: "path",
          data: cmd.data.id === "idle" ? null : `/home/me/.claude-sync/jobs/${cmd.data.id}.log`,
        };
      case "contents": {
        const it = (kind: string, name: string) => ({ kind, name });
        return {
          kind: "contents",
          data: [
            it("claudeMd", "CLAUDE.md"),
            ...["a", "b", "c"].map((n) => it("skill", n)),
            it("agent", "reviewer.md"),
            it("command", "ship.md"),
            it("command", "triage.md"),
            it("plugin", "repo-tools"),
            it("plugin", "big-pack"),
            it("jobs", "schedules.json"),
            it("settings", "model, permissions"),
          ],
        };
      }
      case "copyPreview":
        return {
          kind: "copyPreview",
          data: {
            to: cmd.data.to,
            toName: cmd.data.to.split("/")[1] ?? "",
            host: cmd.data.to.split("/")[0] ?? "",
            canPush: true,
            syncsClaudeMd: true,
            syncsSkills: true,
            syncsAgents: true,
            claudeMd: "Prefer small pull requests.",
            skills: [],
            agents: [],
          },
        };
      case "copy":
        return { kind: "copied", data: { files: 0, claude_md: true, proposed: null } };
      default:
        return done;
    }
  }

  return {
    send,
    snapshot: async () => structuredClone(snap),
    subscribe(cb) {
      subs.add(cb);
      return () => void subs.delete(cb);
    },
    // `?notify=denied` plays a system that blocks notifications
    notifyPermission: async () => (q.get("notify") === "denied" ? "denied" : "granted"),
    // a 1 x 1 PNG
    captureScreen: async () =>
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
    // the restart is played as its result: the new version is running
    installUpdate: async () => {
      set({ update: snap.update.replace(/^ready:/, "updated:") });
    },
    openTarget: async (t) => {
      // the shell (open_target in main.rs) opens only these; a path is checked there on disk
      const ok =
        /^https?:\/\//.test(t) ||
        t === "ms-settings:notifications" ||
        t === "x-apple.systempreferences:com.apple.Notifications-Settings.extension" ||
        /^(\/|~|[A-Za-z]:[\\/])/.test(t);
      if (!ok) throw new Error("open_target refuses " + t);
      (window as unknown as { __opened?: string[] }).__opened = [
        ...((window as unknown as { __opened?: string[] }).__opened ?? []),
        t,
      ];
    },
  };
}
