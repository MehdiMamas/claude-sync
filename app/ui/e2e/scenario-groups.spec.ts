import { expect, test } from "@playwright/test";
import { fake } from "./fake";

// v2-ScenarioGroupHappy, the parts on the Groups list
test("create a group: happy path", async ({ page }) => {
  await page.goto("/?empty");
  await expect(page.getByRole("heading", { name: "No groups yet" })).toBeVisible();
  await page.getByRole("link", { name: "New group" }).first().click();
  await expect(page.getByText("Creates a private GitHub repo under your account.")).toBeVisible();
  await expect(page.getByLabel("Owner")).toHaveValue("alex-host (you)");

  await page.getByLabel("Name").fill("team setup");
  await page.getByRole("button", { name: "Create group" }).click();
  await expect(
    page.getByText("Use letters, numbers, dots, dashes and underscores only."),
  ).toBeVisible();

  await page.getByLabel("Name").fill("team-setup");
  await page.getByRole("button", { name: "Create group" }).click();
  await expect(page).toHaveURL(/#\/group\/alex-host\/team-setup$/);
  await page.getByRole("link", { name: "Groups", exact: true }).click();
  await expect(page.getByText("alex-host/team-setup")).toBeVisible();
  await expect(page.getByText("Host", { exact: true })).toBeVisible();
});

test("groups list: accept an invitation, reorder, turn a group on", async ({ page }) => {
  await page.goto("/?scenario=groups-invite&hold");
  await expect(page.getByText("from sam-member")).toBeVisible();
  await page.getByRole("button", { name: "Accept" }).click();
  await expect(
    page.getByText("Joining sample-org/dev-rules-2 and placing its files…"),
  ).toBeVisible();
  await fake(page).release();
  await expect(page.getByText("Joining sample-org/dev-rules-2")).toHaveCount(0);
  await expect(page.getByRole("link", { name: "sample-org/dev-rules-2" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Invitations" })).toHaveCount(0);

  const names = () => page.locator(".row b").allTextContents();
  expect((await names())[0]).toBe("alex-host/claude-group");
  await page.getByRole("button", { name: "Move down" }).first().click();
  await expect(page.getByText("Order saved on all your machines")).toBeVisible();
  expect((await names())[0]).toBe("sample-org/dev-rules");

  await page.getByRole("button", { name: "Turn on" }).click();
  await expect(page.getByText("Turned off on all your machines")).toHaveCount(0);
});

test("groups list: all turned off, then all on", async ({ page }) => {
  await page.goto("/?scenario=groups-all-off");
  await expect(
    page.getByText(
      "All groups are off on all your machines. Their files left ~/.claude; your own files stayed.",
    ),
  ).toBeVisible();
  await page.getByRole("button", { name: "Turn all on" }).click();
  await expect(page.getByText("Their files left")).toHaveCount(0);
  await expect(page.getByRole("link", { name: "alex-host/claude-group" })).toBeVisible();
});

// v2-ScenarioGroupEdge, the parts on the Groups list
test("create a group: name taken keeps the text", async ({ page }) => {
  await page.goto("/?scenario=groups#/groups/new");
  await page.getByRole("button", { name: "Create group" }).click();
  await expect(
    page.getByText("You already have a repo named claude-group. Pick another name."),
  ).toBeVisible();
  await expect(page.getByLabel("Name")).toHaveValue("claude-group");
  await expect(page.getByLabel("Name")).toHaveAttribute("aria-invalid", "true");
  await page.getByRole("link", { name: "Cancel" }).click();
  await expect(page.getByRole("heading", { name: "Groups" })).toBeVisible();
});

test("join: no access writes nothing", async ({ page }) => {
  await page.goto("/?scenario=groups-invite#/groups/join");
  await expect(
    page.getByText(
      "Plugins in this group run code on every member's machine; only join groups whose members you trust.",
    ),
  ).toBeVisible();
  await expect(page.getByText("9 members")).toHaveCount(0);
  await page.getByRole("button", { name: "Join" }).nth(1).click();
  await expect(
    page.getByText("Could not join sample-org/ml-team: GitHub says you have no access (404)."),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: "sample-org/ml-team" })).toHaveCount(0);
  await page.getByRole("link", { name: "Details" }).click();
  await expect(page.getByRole("heading", { name: "Join a group" })).toBeVisible();
});

test("join: decline hides the invitation", async ({ page }) => {
  await page.goto("/?scenario=groups-invite");
  await page.getByRole("button", { name: "Decline" }).click();
  await expect(page.getByRole("heading", { name: "Invitations" })).toHaveCount(0);
});
