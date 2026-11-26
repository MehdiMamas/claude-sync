// The fake backend of the dev server and the Playwright runs: the same `send`, `snapshot` and
// `open_target` interface as the shell, over the generated types. Sample data only.
import type { Backend } from "./backend";
import type { Command } from "./generated/Command";
import type { Event } from "./generated/Event";
import type { GroupStatus } from "./generated/GroupStatus";
import type { GroupView } from "./generated/GroupView";
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
    repo: "alex-host/claude-personal",
    kindsOff: [],
    devices: [],
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
  desktopNotifications: true,
  startAtLogin: true,
});

const code = {
  kind: "code",
  user_code: "WDJB-MJHT",
  verification_uri: "https://github.com/login/device",
  expires_in: 900,
} as const;

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
    case "groups-all-off":
      s.groups = s.groups.slice(0, 2).map((g) => ({ ...g, status: "off" as const, off: true }));
      s.invitations = [];
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
  end(kind: "expired" | "denied" | "offline"): void;
};

export function fakeBackend(search: string): Backend {
  const q = new URLSearchParams(search);
  let name = q.get("scenario") ?? "groups-invite";
  if (q.has("signedOut")) name = "signed-out";
  if (q.has("empty")) name = "groups-empty";
  // `?noHistory` has no screen in this part: the groups list is the same.
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
    end(kind) {
      set({ signIn: { kind } });
    },
  };
  (window as unknown as { __fake: FakeControls }).__fake = controls;

  const done: Event = { kind: "done" };
  const addGroup = (repo: string, over: Partial<GroupView> = {}) =>
    set({ groups: [...snap.groups, group(repo, { members: many(3), ...over })] });

  async function send(cmd: Command): Promise<Event> {
    switch (cmd.kind) {
      case "signInStart":
        if (name === "sign-in-unavailable") return done;
        set({ signIn: { kind: "starting" } });
        await sleep(60);
        set({ signIn: code });
        return done;
      case "signInCancel":
        set({ signIn: { kind: "signedOut" } });
        return done;
      case "signOut":
        set({ login: null, groups: [] });
        return { kind: "signedOut" };
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
        return { kind: "created", data: { repo: cmd.data.repo, lock: "on" } };
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
    openTarget: async (t) => {
      (window as unknown as { __opened?: string[] }).__opened = [
        ...((window as unknown as { __opened?: string[] }).__opened ?? []),
        t,
      ];
    },
  };
}
