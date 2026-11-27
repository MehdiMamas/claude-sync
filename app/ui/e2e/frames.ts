import type { Page } from "@playwright/test";

/** How to reach each frame's state in the app (scenario URL plus the clicks a person makes). */
export type FrameCase = {
  frame: string;
  url: string;
  steps?: (p: Page) => Promise<void>;
  /** Percent of differing pixels allowed, when the frame leaves out a part the app keeps. */
  max?: number;
};

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

const members = `${base}/members`;
const host = (p: Page) => p.getByLabel("GitHub name");
const room = "/?scenario=members";
const review = `${base}/review`;
const guest = "/group/sample-org/dev-rules";

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
  { frame: "GroupMembers", url: `${room}#${members}` },
  {
    frame: "GroupMembersGuest",
    url: `/?scenario=members-guest#/group/sample-org/dev-rules/members`,
  },
  {
    frame: "GroupMembersProposalMode",
    url: `/?scenario=members-proposal#${members}`,
    max: 25, // the frame leaves out the Members card the app keeps
    steps: async (p) => {
      await p.getByRole("button", { name: "Proposal mode" }).click();
      await p.getByText("Proposal mode is on.").waitFor();
    },
  },
  { frame: "GroupMemberRemove", url: `${room}#${members}/remove/sam-member` },
  {
    frame: "GroupMemberRemoved",
    url: `${room}#${members}`,
    steps: async (p) => {
      await p.getByRole("link", { name: "Remove" }).click();
      await p.getByRole("button", { name: "Remove" }).click();
      await p.getByText("sam-member removed").waitFor();
    },
  },
  {
    frame: "GroupInvited",
    url: `/?scenario=members-solo#${members}`,
    steps: async (p) => {
      await host(p).fill("kim-dev");
      await p.getByRole("button", { name: "Invite", exact: true }).click();
      await p.getByText("Invitation sent to kim-dev").waitFor();
    },
  },
  {
    frame: "GroupInviteCanceled",
    url: `${room}#${members}`,
    steps: async (p) => {
      await p.getByRole("button", { name: "Cancel invite" }).click();
      await p.getByText("Invitation to kim-dev canceled").waitFor();
    },
  },
  {
    frame: "GroupInviteError",
    url: `${room}#${members}`,
    steps: async (p) => {
      await host(p).fill("kim-devv");
      await p.getByRole("button", { name: "Invite", exact: true }).click();
      await p.getByText("Check the spelling").waitFor();
    },
  },
  { frame: "GroupTransfer", url: `${room}#${members}/transfer/sam-member` },
  {
    frame: "GroupTransferPending",
    max: 8, // the frame's two demo buttons are not built
    url: `${room}#${members}/transfer/sam-member`,
    steps: async (p) => {
      await p.getByRole("button", { name: "Send request" }).click();
      await p.getByText("transfer requested").waitFor();
    },
  },
  { frame: "GroupTransferExpired", url: `${room}#${members}/expired/sam-member` },
  {
    frame: "GroupTransferred",
    url: `${room}#${members}/transfer/sam-member`,
    steps: async (p) => {
      await p.getByRole("button", { name: "Send request" }).click();
      await p.getByText("transfer requested").waitFor();
      await p.evaluate(() =>
        (window as unknown as { __fake: { transferred(l: string): void } }).__fake.transferred(
          "sam-member",
        ),
      );
      await p.getByText("sam-member is now the host.").waitFor();
    },
  },
  { frame: "GroupReview", url: `/?scenario=group#${review}` },
  {
    frame: "GroupReviewLines",
    url: `/?scenario=group#${review}`,
    steps: async (p) => {
      await p.getByRole("button", { name: "Choose lines" }).click();
      await p.getByRole("button", { name: "Apply my choices" }).waitFor();
    },
  },
  { frame: "GroupReviewClash", url: `/?scenario=group&clash#${review}` },
  {
    frame: "GroupReviewDeclined",
    url: `/?scenario=group#${review}`,
    steps: async (p) => {
      await p.getByRole("button", { name: "Decline" }).click();
      await p.getByText("3 lines declined").waitFor();
    },
  },
  {
    frame: "GroupReviewDone",
    url: `/?scenario=group#${review}`,
    steps: async (p) => {
      await p.getByRole("button", { name: "Accept all" }).click();
      await p.getByText("3 lines accepted").waitFor();
    },
  },
  {
    frame: "GroupReviewGuest",
    url: `/?scenario=review-guest#/group/sample-org/dev-rules/review`,
  },
  { frame: "GroupProposals", url: `/?scenario=proposals#${base}/proposals` },
  { frame: "GroupProposalsGuest", url: `/?scenario=proposals-guest#${guest}/proposals` },
  { frame: "GroupProposeForm", url: `/?scenario=propose#${guest}/propose` },
  {
    frame: "GroupProposalApproved",
    url: `/?scenario=proposals-one#${base}/proposals`,
    steps: async (p) => {
      await p.getByRole("button", { name: "Merge" }).click();
      await p.getByText("No open proposals.").waitFor();
    },
  },
  {
    frame: "GroupProposalApprovedGuest",
    url: `/?scenario=proposals-guest-one#${guest}/proposals`,
    steps: async (p) => {
      await p.getByRole("button", { name: "Approve" }).click();
      await p.getByText("Approved by you").waitFor();
    },
  },
  {
    frame: "GroupProposalDeclined",
    url: `/?scenario=proposals-one#${base}/proposals`,
    steps: async (p) => {
      await p.getByRole("button", { name: "Decline", exact: true }).click();
      await p.getByText("No open proposals.").waitFor();
    },
  },
  { frame: "GroupHistory", url: `/?scenario=history#${base}/history` },
  { frame: "GroupNoHistory", url: `/?scenario=no-history#${base}/history` },
  {
    frame: "GroupRevert",
    url: `/?scenario=history#${base}/history`,
    steps: async (p) => {
      await p.getByRole("link", { name: "Revert" }).first().click();
      await p.getByRole("dialog").getByText("Makes a new commit").waitFor();
    },
  },
  {
    frame: "GroupHelper",
    url: `/?scenario=history#${base}/history`,
    steps: async (p) => {
      await p.getByRole("link", { name: "Explain" }).click();
      await p.getByText("The tabs rule was removed.").waitFor();
    },
  },
  {
    frame: "GroupHelperUnavailable",
    url: `/?scenario=helper-unavailable#${base}/history`,
    steps: async (p) => {
      await p.getByRole("link", { name: "Explain" }).click();
      await p.getByText("so it cannot explain changes.").waitFor();
    },
  },
  { frame: "GroupJobs", url: `/?scenario=jobs#${base}/jobs` },
  { frame: "GroupJobAccept", url: `/?scenario=jobs-edge#${base}/jobs/risky` },
  {
    frame: "GroupJobScheduled",
    url: `/?scenario=jobs#${base}/jobs/nightly-lint`,
    steps: async (p) => {
      await p.getByRole("button", { name: "Accept and schedule" }).click();
      await p.getByText("Scheduled: nightly-lint").waitFor();
    },
  },
  {
    frame: "GroupJobRan",
    url: `/?scenario=jobs#${base}/jobs`,
    steps: async (p) => {
      await p.getByRole("button", { name: "Run now" }).click();
      await p.getByText("weekly-report finished").waitFor();
    },
  },
  ...[
    ["Settings", "groups-invite", ""],
    ["SettingsPaused", "settings-paused", ""],
    ["SettingsOff", "groups-invite", "/personal"],
    ["SettingsOffer", "settings-offer", "/personal"],
    ["SettingsPersonal", "settings-personal", "/personal"],
    ["SettingsPersonalPublic", "settings-public", "/personal"],
    ["SettingsDeviceRemove", "settings-personal", "/personal/remove/d2"],
    ["SettingsNotifications", "groups-invite", "/notifications"],
    ["SettingsAccount", "groups-invite", "/account"],
    ["SettingsAccountApp", "settings-app", "/account"],
    ["SettingsSignOut", "groups-invite", "/account/signout"],
    ["SettingsUpdates", "groups-invite", "/updates"],
    ["SettingsUpdateReady", "update-ready", "/updates"],
    ["SettingsUpdateError", "update-error", "/updates"],
    ["SettingsUpdated", "update-updated", "/updates"],
    ["SettingsAdvanced", "groups-invite", "/advanced"],
    ["SettingsPacks", "groups-invite", "/advanced/packs"],
    ["SettingsSsh", "settings-ssh", "/advanced/ssh"],
    ["SettingsConflicts", "groups-invite", "/advanced/conflicts"],
  ].map(([frame, scenario, path]) => ({
    frame: frame as string,
    url: `/?scenario=${scenario}#/settings${path}`,
  })),
  {
    frame: "SettingsNotifyBlocked",
    url: "/?scenario=groups-invite#/settings/notifications",
    steps: async (p) => {
      await p.getByRole("button", { name: "Test" }).click();
      await p.getByText("blocks notifications from claude-sync").waitFor();
    },
  },
];
