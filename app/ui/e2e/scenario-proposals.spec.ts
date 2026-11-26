import { expect, test } from "@playwright/test";
import { fake } from "./fake";

const host = "#/group/alex-host/claude-group";
const member = "#/group/sample-org/dev-rules";

// v2-ScenarioEditsHappy: propose, approve, merge
test("proposals: the host merges an approved one and the list empties", async ({ page }) => {
  await page.goto(`/?scenario=proposals-one${host}/proposals`);
  await expect(page.getByText("Approved by kim-dev")).toBeVisible();
  await expect(page.getByText("skills/test-runner/SKILL.md")).toBeVisible();
  await page.getByRole("button", { name: "Merge" }).click();
  await expect(
    page.getByText("Approved and merged. Members get it on their next sync."),
  ).toBeVisible();
  await expect(page.getByText("No open proposals.")).toBeVisible();
});

test("proposals: three states in one list, own proposal has no buttons", async ({ page }) => {
  await page.goto(`/?scenario=proposals${host}/proposals`);
  await expect(page.getByText("Needs an update")).toBeVisible();
  await expect(page.getByText("kim-dev updates it from the app")).toBeVisible();
  const mine = page.locator(".row", { hasText: "Add lint agent" });
  await expect(mine.getByText("you cannot approve your own")).toBeVisible();
  await expect(mine.getByRole("button")).toHaveCount(0);
  const stale = page.locator(".row", { hasText: "Shorter commit rule" });
  await expect(stale.getByRole("button", { name: "Merge" })).toHaveCount(0);
  await expect(stale.getByRole("button", { name: "Decline" })).toBeVisible();
});

test("proposals: another member approves, never their own", async ({ page }) => {
  await page.goto(`/?scenario=proposals-guest${member}/proposals`);
  const own = page.locator(".row", { hasText: "Add test-runner skill" });
  await expect(own.getByText("Waiting for another member")).toBeVisible();
  await expect(own.getByRole("button")).toHaveCount(0);
  await page.getByRole("button", { name: "Approve" }).click();
  await expect(page.getByText("Approved. The host can merge it now.")).toBeVisible();
  await expect(page.getByText("Approved by you")).toBeVisible();
  await expect(page.getByRole("button", { name: "Approve" })).toHaveCount(0);
});

// v2-ScenarioEditsEdge: host declines, or declines all
test("proposals: decline closes it, decline all clears the list", async ({ page }) => {
  await page.goto(`/?scenario=proposals-one${host}/proposals`);
  await page.getByRole("button", { name: "Decline", exact: true }).click();
  await expect(
    page.getByText("Declined. The pull request is closed; the author keeps the change locally."),
  ).toBeVisible();
  await expect(page.getByText("No open proposals.")).toBeVisible();

  await page.goto(`/?scenario=proposals${host}/proposals`);
  await page.getByRole("button", { name: "Decline all" }).click();
  await expect(page.getByText("No open proposals.")).toBeVisible();
});

test("proposals: accept all merges what is ready and leaves one behind main", async ({ page }) => {
  await page.goto(`/?scenario=proposals${host}/proposals`);
  await page.getByRole("button", { name: "Accept all approved" }).click();
  await expect(page.getByText("Add test-runner skill")).toHaveCount(0);
  await expect(page.getByText("Shorter commit rule")).toBeVisible();
});

test("proposals: a member proposes a local skill and the toast names the number", async ({
  page,
}) => {
  await page.goto(`/?scenario=propose${member}/propose`);
  await expect(page.getByLabel("What to propose")).toContainText(
    "skills/test-runner (your local skill)",
  );
  await page.getByLabel("Note for the host").fill("Runs the tests before each commit.");
  await page.getByRole("button", { name: "Send proposal" }).click();
  await expect(page.getByText("Proposed as #14. A member must approve it.")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Needs you" })).toBeVisible();
});

// v2-ScenarioEditsHappy: History tab, revert after a preview
test("history: changes with notes, issue chip and explain only for others", async ({ page }) => {
  await page.goto(`/?scenario=history${host}/history`);
  await expect(page.getByText("Prefer small pull requests")).toBeVisible();
  await expect(page.getByText("#12")).toBeVisible();
  await expect(page.getByText("Before: no rule on pull request size.")).toBeVisible();
  await expect(page.getByText("Now: keep pull requests small.")).toBeVisible();
  await expect(page.getByRole("link", { name: "Explain" })).toHaveCount(1);
  await page.getByRole("button", { name: "View on GitHub" }).click();
  expect(await fake(page).opened()).toEqual([
    "https://github.com/alex-host/claude-group/commit/a1",
  ]);
  await expect(page.getByText("Initial commit")).toHaveCount(0);
  await page.getByRole("button", { name: "Show more" }).click();
  await expect(page.getByText("Initial commit")).toBeVisible();
  await expect(page.getByRole("button", { name: "Show more" })).toHaveCount(0);
});

test("history: the overview shows the same notes instead of raw text", async ({ page }) => {
  await page.goto(`/?scenario=history${host}`);
  await expect(page.getByText("Now: keep pull requests small.")).toBeVisible();
});

test("history: revert asks first, cancel changes nothing, revert makes a commit", async ({
  page,
}) => {
  await page.goto(`/?scenario=history${host}/history`);
  await page.getByRole("link", { name: "Revert" }).first().click();
  const dlg = page.getByRole("dialog");
  await expect(
    dlg.getByRole("heading", { name: 'Revert "Prefer small pull requests"?' }),
  ).toBeVisible();
  await expect(dlg.getByText("Makes a new commit that undoes it.")).toBeVisible();
  await dlg.getByRole("link", { name: "Cancel" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("link", { name: "Revert" }).first().click();
  await page.getByRole("dialog").getByRole("button", { name: "Revert" }).click();
  await expect(page.getByText("Reverted. Members get it on their next sync.")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Changes" })).toBeVisible();
});

test("history: a member sees the list without Revert", async ({ page }) => {
  await page.goto(`/?scenario=group-member${member}/history`);
  await expect(page.getByRole("heading", { name: "Changes" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Revert" })).toHaveCount(0);
});

test("history: a new group says there are no changes", async ({ page }) => {
  await page.goto(`/?scenario=group&noHistory${host}/history`);
  await expect(page.getByRole("heading", { name: "No changes yet" })).toBeVisible();
  await page.getByRole("link", { name: "Share a file" }).click();
  await expect(page.getByRole("heading", { name: "Your local edits" })).toBeVisible();
});

// v2-ScenarioReviewEdge, the helper
test("helper: explain answers in words, close goes back", async ({ page }) => {
  await page.goto(`/?scenario=history${host}/history`);
  await page.getByRole("link", { name: "Explain" }).click();
  await expect(
    page.getByRole("heading", { name: "What changed (from your Claude login)" }),
  ).toBeVisible();
  await expect(page.getByText("The tabs rule was removed.")).toBeVisible();
  await expect(page.getByText("No API key is used.")).toBeVisible();
  await page.getByRole("link", { name: "Close" }).click();
  await expect(page.getByRole("heading", { name: "Changes" })).toBeVisible();
});

test("helper: not signed in says so and the list still works", async ({ page }) => {
  await page.goto(`/?scenario=helper-unavailable${host}/history`);
  await page.getByRole("link", { name: "Explain" }).click();
  await expect(
    page.getByText("Claude Code is not signed in on this machine, so it cannot explain changes."),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: "Revert" })).toBeVisible();
  await page.getByRole("link", { name: "Close" }).click();
  await expect(page.getByRole("link", { name: "Explain" })).toBeVisible();
});
