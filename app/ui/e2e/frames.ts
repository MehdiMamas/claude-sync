import type { Page } from "@playwright/test";

/** How to reach each frame's state in the app (scenario URL plus the clicks a person makes). */
export type FrameCase = { frame: string; url: string; steps?: (p: Page) => Promise<void> };

const signInCode = async (p: Page) => {
  await p.getByRole("button", { name: "Sign in with GitHub" }).click();
  await p.getByText("WDJB-MJHT").waitFor();
};

const base = "/group/alex-host/claude-group";
const edits = `${base}/edits`;
const createGroup = async (p: Page) => {
  await p.getByRole("button", { name: "Create group" }).click();
  await p.getByText("Invite members").first().waitFor();
};

export const frames: FrameCase[] = [
  { frame: "SignIn", url: "/?scenario=signed-out" },
  { frame: "SignInCode", url: "/?scenario=signed-out", steps: signInCode },
  {
    frame: "SignInCodeCopied",
    url: "/?scenario=signed-out",
    steps: async (p) => {
      await signInCode(p);
      await p.getByRole("button", { name: "Copy code" }).click();
      await p.getByRole("button", { name: "Copied" }).waitFor();
    },
  },
  {
    frame: "SignInApproved",
    url: "/?scenario=signed-out",
    steps: async (p) => {
      await signInCode(p);
      await p.evaluate(() =>
        (window as unknown as { __fake: { approve(): void } }).__fake.approve(),
      );
      await p.getByRole("heading", { name: "Signed in as alex-host" }).waitFor();
    },
  },
  { frame: "SignInCodeExpired", url: "/?scenario=sign-in-code-expired" },
  { frame: "SignInDenied", url: "/?scenario=sign-in-denied" },
  { frame: "SignInExpired", url: "/?scenario=sign-in-expired" },
  { frame: "SignInOffline", url: "/?scenario=sign-in-offline" },
  { frame: "SignInUnavailable", url: "/?scenario=sign-in-unavailable" },
  { frame: "Tray", url: "/?view=tray" },
  { frame: "TrayQuit", url: "/?view=trayQuit" },
  { frame: "Groups", url: "/?scenario=groups-invite" },
  {
    frame: "GroupsAccepting",
    url: "/?scenario=groups-invite&hold",
    steps: async (p) => {
      await p.getByRole("button", { name: "Accept" }).click();
      await p.getByText("Joining sample-org/dev-rules-2").waitFor();
    },
  },
  { frame: "GroupsAllOff", url: "/?scenario=groups-all-off" },
  { frame: "GroupsCreate", url: "/?scenario=groups#/groups/new" },
  {
    frame: "GroupsCreateError",
    url: "/?scenario=groups#/groups/new",
    steps: async (p) => {
      await p.getByRole("button", { name: "Create group" }).click();
      await p.getByText("You already have a repo named claude-group").waitFor();
    },
  },
  { frame: "GroupsEmpty", url: "/?scenario=groups-empty" },
  { frame: "GroupsJoin", url: "/?scenario=groups-invite#/groups/join" },
  {
    frame: "GroupsJoinError",
    url: "/?scenario=groups#/groups/join",
    steps: async (p) => {
      await p.getByRole("button", { name: "Join" }).nth(1).click();
      await p.getByText("Could not join sample-org/ml-team").waitFor();
    },
  },
  { frame: "GroupsLoading", url: "/?scenario=groups-loading" },
  { frame: "GroupsOffline", url: "/?scenario=groups-offline" },
  { frame: "GroupsPlugins", url: "/?scenario=groups-plugins" },
  { frame: "GroupsRateLimited", url: "/?scenario=groups-rate-limited" },
  {
    frame: "GroupsReordered",
    url: "/?scenario=groups",
    steps: async (p) => {
      await p.getByRole("button", { name: "Move down" }).click();
      await p.getByText("Order saved on all your machines").waitFor();
    },
  },
  {
    frame: "GroupsSynced",
    url: "/?scenario=groups",
    steps: async (p) => {
      await p.getByRole("button", { name: "Sync now" }).click();
      await p.getByText("Synced: 2 files updated in alex-host/claude-group").waitFor();
    },
  },
  { frame: "GroupsSyncError", url: "/?scenario=groups-sync-error" },
  { frame: "GroupsSyncing", url: "/?scenario=groups-syncing" },
  { frame: "Group", url: "/?scenario=group#/group/alex-host/claude-group" },
  { frame: "GroupGuest", url: "/?scenario=group-member#/group/sample-org/dev-rules" },
  { frame: "GroupOff", url: "/?scenario=group-off#/group/alex-host/claude-group" },
  { frame: "GroupLeave", url: "/?scenario=group-member#/group/sample-org/dev-rules/leave" },
  { frame: "GroupCreated", url: "/?scenario=groups-empty#/groups/new", steps: createGroup },
  {
    frame: "GroupCreatedNoRuleset",
    url: "/?scenario=groups-empty&noLock#/groups/new",
    steps: createGroup,
  },
  { frame: "GroupLocalEdits", url: `/?scenario=group#${edits}` },
  {
    frame: "GroupEditKept",
    url: `/?scenario=group#${edits}`,
    steps: async (p) => {
      await p.getByRole("button", { name: "Keep mine" }).first().click();
      await p.getByText("Kept yours. It is not shared.").waitFor();
    },
  },
  {
    frame: "GroupEditUndone",
    url: `/?scenario=group#${edits}`,
    steps: async (p) => {
      await p.getByRole("button", { name: "Undo" }).first().click();
      await p.getByText("Undone: the group version of skills/release-notes is back").waitFor();
    },
  },
  { frame: "GroupShare", url: `/?scenario=group#${base}/share/skills/release-notes/SKILL.md` },
  { frame: "GroupShareBlocked", url: `/?scenario=group#${base}/share/settings.json` },
  {
    frame: "GroupShared",
    url: `/?scenario=group#${edits}`,
    steps: async (p) => {
      await p.getByRole("link", { name: "Share" }).first().click();
      await p.getByRole("button", { name: "Share with group" }).click();
      await p.getByText("Shared. Members get it on their next sync.").waitFor();
    },
  },
  { frame: "GroupContents", url: `/?scenario=group#${base}/contents` },
  { frame: "GroupCopy", url: `/?scenario=group#${base}/copy` },
];
