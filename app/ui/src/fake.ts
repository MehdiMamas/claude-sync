// The fake backend of the dev server and the Playwright runs: the same `send`, `snapshot` and
// `open_target` interface as the shell, over the generated types. Sample data only.
import type { Backend } from "./backend";
import type { Command } from "./generated/Command";
import type { EditView } from "./generated/EditView";
import type { Event } from "./generated/Event";
import type { GroupStatus } from "./generated/GroupStatus";
import type { GroupView } from "./generated/GroupView";
import type { JobView } from "./generated/JobView";
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

const job = (repo: string): JobView => ({
  repo,
  id: "nightly-lint",
  accepted: false,
  enabled: false,
  lastResult: "",
  title: "nightly-lint",
  cron: "0 3 * * *",
  prompt: "Lint the repo",
  cwd: "~/work",
  mode: "plan",
  timeoutMinutes: 10,
  args: [],
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
    case "group":
      s.jobs = [job(s.groups[0]?.repo ?? "")];
      setProblems(0, ["couldn't install big-pack@mods: registry unreachable"]);
      return s;
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
  };
  (window as unknown as { __fake: FakeControls }).__fake = controls;

  const done: Event = { kind: "done" };
  // Local edits per group: the host has two, the member one.
  const edits: Record<string, EditView[]> = {
    "alex-host/claude-group": [
      { path: "skills/release-notes/SKILL.md", kept: false },
      { path: "settings.json", kept: false },
    ],
    "sample-org/dev-rules": [{ path: "skills/release-notes/SKILL.md", kept: false }],
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
  let clash = q.has("clash");
  const patchGroup = (repo: string, p: Partial<GroupView>) =>
    set({ groups: snap.groups.map((g) => (g.repo === repo ? { ...g, ...p } : g)) });
  const secret = "secret found in settings.json:12 (API key); nothing was committed";
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
        clash = false;
        return done;
      case "useGroupVersion":
        return done;
      case "explain":
        if (q.has("noClaude"))
          throw "Claude is not signed in on this machine. Run claude, sign in, then try again.";
        return {
          kind: "text",
          data: "Adds two working rules and removes the tabs rule. Safe to accept.",
        };
      case "proposals": {
        const host = cmd.data.repo === "alex-host/claude-group";
        const p = (author: string, mine: boolean) => ({
          number: 12,
          branch: `proposal/${author}-20261126100000`,
          author,
          title: "Add test-runner skill",
          url: "https://github.com/sample-org/dev-rules/pull/12",
          files: [],
          approvals: [],
          needsUpdate: false,
          mine,
          approvedByMe: false,
          head: "abc1234",
        });
        return {
          kind: "proposals",
          data: host
            ? [p("sam-member", false)]
            : [{ ...p("kim-dev", false), number: 13 }, p("alex-host", true)],
        };
      }
      case "history":
        return {
          kind: "history",
          data: [
            {
              sha: "a1",
              summary: "sam-member changed CLAUDE.md",
              author: "sam-member",
              at: new Date((sec() - 3600) * 1000).toISOString(),
              issues: [],
              before: null,
              now: null,
            },
            {
              sha: "b2",
              summary: "you added skill release-notes",
              author: snap.login ?? "",
              at: new Date((sec() - 86400) * 1000).toISOString(),
              issues: [],
              before: null,
              now: null,
            },
          ],
        };
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
        return done;
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
    openTarget: async (t) => {
      (window as unknown as { __opened?: string[] }).__opened = [
        ...((window as unknown as { __opened?: string[] }).__opened ?? []),
        t,
      ];
    },
  };
}
