import { expect, test } from "@playwright/test";

const host = "#/group/alex-host/claude-group";
const member = "#/group/sample-org/dev-rules";

// v2-ScenarioGroupHappy, the parts on the group page
test("group page: created, invite, overview, turn off and on", async ({ page }) => {
  await page.goto("/?scenario=groups-empty#/groups/new");
  await page.getByRole("button", { name: "Create group" }).click();
  await expect(
    page.getByText("Group created. The repo is private and only you can push to main."),
  ).toBeVisible();
  await expect(page.getByText("1 member · just created")).toBeVisible();
  await page.getByLabel("GitHub name").fill("sam-member");
  await page.getByRole("button", { name: "Send invite" }).click();
  await expect(page).toHaveURL(/#\/group\/alex-host\/claude-group\/members$/);

  await page.goto("/?scenario=group#/groups");
  await page.getByRole("link", { name: "alex-host/claude-group" }).click();
  await expect(page.getByText("3 CLAUDE.md lines from sam-member")).toBeVisible();
  await expect(page.getByText('1 proposal: "Add test-runner skill"')).toBeVisible();
  await expect(page.getByText("2 of your local edits are not shared")).toBeVisible();
  await expect(page.getByText("1 job waits for your yes: nightly-lint")).toBeVisible();
  await page.getByRole("button", { name: "Turn off" }).click();
  await expect(page.getByText("This group is off for you.")).toBeVisible();
  await expect(page.getByRole("link", { name: "Overview" })).toHaveCount(0);
  await page.getByRole("button", { name: "Turn on" }).click();
  await expect(page.getByRole("link", { name: "Overview" })).toBeVisible();
});

test("group page: a member sees the member overview and can leave", async ({ page }) => {
  await page.goto(`/?scenario=group-member${member}`);
  await expect(page.getByText("host sam-member")).toBeVisible();
  await expect(page.getByText("1 CLAUDE.md line from sam-member")).toBeVisible();
  await expect(
    page.getByText("1 proposal by kim-dev needs another member's approval"),
  ).toBeVisible();
  await expect(page.getByText("1 of your local edits differs from the group")).toBeVisible();
  await expect(page.getByRole("button", { name: "Turn off" })).toHaveCount(0);

  await page.getByRole("link", { name: "Leave" }).click();
  await expect(page.getByRole("heading", { name: "Leave sample-org/dev-rules?" })).toBeVisible();
  await page.getByRole("link", { name: "Cancel" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("link", { name: "Leave" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Leave" }).click();
  await expect(page.getByRole("heading", { name: "Groups" })).toBeVisible();
  await expect(page.getByRole("link", { name: "sample-org/dev-rules" })).toHaveCount(0);
});

// v2-ScenarioGroupEdge, the parts on the group page
test("group page: lock refused keeps the host-only app rule", async ({ page }) => {
  await page.goto("/?scenario=groups-empty&noLock#/groups/new");
  await page.getByRole("button", { name: "Create group" }).click();
  await expect(
    page.getByText(
      "GitHub can't lock this repo on your plan; the app still only sends changes from your machine.",
    ),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Send invite" })).toBeDisabled();
});

test("group page: an invite that fails says why and keeps the name", async ({ page }) => {
  await page.goto("/?scenario=groups-empty#/groups/new");
  await page.getByRole("button", { name: "Create group" }).click();
  await page.getByLabel("GitHub name").fill("no-such-user");
  await page.getByRole("button", { name: "Send invite" }).click();
  await expect(
    page.getByText("No GitHub user named no-such-user. Nothing was sent."),
  ).toBeVisible();
  await expect(page.getByLabel("GitHub name")).toHaveValue("no-such-user");
});

// v2-ScenarioEditsHappy, the parts in the app
test("edits: keep mine, undo, share, then nothing is left", async ({ page }) => {
  await page.goto(`/?scenario=group${host}/edits`);
  await expect(page.getByText("skills/release-notes/SKILL.md")).toBeVisible();
  await page.getByRole("button", { name: "Keep mine" }).first().click();
  await expect(page.getByText("Kept yours. It is not shared.")).toBeVisible();
  await expect(page.getByText("Kept yours", { exact: true })).toBeVisible();
  await expect(
    page.getByText(
      "stays on your machines, not shared; a group change to this file asks you first",
    ),
  ).toBeVisible();

  await page.getByRole("button", { name: "Undo" }).nth(1).click();
  await expect(page.getByText("settings.json")).toHaveCount(0);

  await page.getByRole("link", { name: "Share" }).click();
  await expect(page.getByRole("heading", { name: "Share skills/release-notes" })).toBeVisible();
  await expect(page.getByText("+ Write release notes in plain words.")).toBeVisible();
  await expect(page.getByText("Checked for secrets: none found.")).toBeVisible();
  await page.getByRole("button", { name: "Share with group" }).click();
  await expect(page.getByText("Shared. Members get it on their next sync.")).toBeVisible();
  await expect(page.getByText("Nothing left to share.")).toBeVisible();
});

test("edits: undo says which group version is back", async ({ page }) => {
  await page.goto(`/?scenario=group${host}/edits`);
  await page.getByRole("button", { name: "Undo" }).first().click();
  await expect(
    page.getByText("Undone: the group version of skills/release-notes is back"),
  ).toBeVisible();
  await expect(page.getByText("skills/release-notes/SKILL.md")).toHaveCount(0);
});

test("copy CLAUDE.md lines to another group, then contents", async ({ page }) => {
  await page.goto(`/?scenario=group${host}/edits`);
  await page.getByRole("link", { name: "Copy to group" }).click();
  await expect(page.getByLabel("To group")).toHaveValue("sample-org/dev-rules");
  await expect(page.getByText("+ Prefer small pull requests.")).toBeVisible();
  await page.getByRole("button", { name: "Copy", exact: true }).click();
  await expect(page.getByText("Copied to sample-org/dev-rules")).toBeVisible();
  await expect(page.getByRole("heading", { name: "In this group" })).toBeVisible();
  await expect(page.getByText("Plugins 2")).toBeVisible();
  await expect(page.getByText("1 failed")).toBeVisible();
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByText("1 failed")).toHaveCount(0);
  await page.getByRole("link", { name: "Local edits (2)" }).click();
  await expect(page.getByRole("heading", { name: "Your local edits" })).toBeVisible();
});

// v2-ScenarioEditsEdge
test("share refused: a secret names the file and line, nothing is sent", async ({ page }) => {
  await page.goto(`/?scenario=group${host}/edits`);
  await page.getByRole("link", { name: "Share" }).nth(1).click();
  await expect(
    page.getByText(
      "Not shared: settings.json line 12 looks like a secret (API key). Remove it, or mark the line claude-sync: allow-secret.",
    ),
  ).toBeVisible();
  await expect(page.getByText("sk-…redacted")).toBeVisible();
  await expect(page.getByRole("button", { name: "Share with group" })).toHaveCount(0);
  await page.getByRole("link", { name: "Back", exact: true }).click();
  await expect(page.locator(".mono", { hasText: "settings.json" })).toBeVisible();
});
