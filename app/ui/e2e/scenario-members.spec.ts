import { expect, test } from "@playwright/test";
import { fake } from "./fake";

const room = "/?scenario=members#/group/alex-host/claude-group/members";
const guest = "/?scenario=members-guest#/group/sample-org/dev-rules/members";

// v2-ScenarioMembersHappy
test("members: invite, cancel, remove after a confirm", async ({ page }) => {
  await page.goto("/?scenario=members-solo#/group/alex-host/claude-group/members");
  await page.getByLabel("GitHub name").fill("kim-dev");
  await page.getByRole("button", { name: "Invite", exact: true }).click();
  await expect(page.getByText("Invitation sent to kim-dev")).toBeVisible();
  await expect(page.getByText("Invited", { exact: true })).toBeVisible();
  await expect(page.getByText("listed apart until GitHub says it was accepted")).toBeVisible();
  await page.getByRole("button", { name: "Cancel invite" }).click();
  await expect(page.getByText("Invitation to kim-dev canceled")).toBeVisible();
  await expect(page.getByText("kim-dev")).toHaveCount(1); // only in the toast

  await page.goto(room);
  await page.getByRole("link", { name: "Remove" }).click();
  await expect(page.getByRole("heading", { name: "Remove sam-member?" })).toBeVisible();
  await page.getByRole("link", { name: "Cancel" }).click();
  await expect(page.locator(".row", { hasText: "sam-member" })).toBeVisible();
  await page.getByRole("link", { name: "Remove" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Remove" }).click();
  await expect(page.getByText("sam-member removed")).toBeVisible();
  await expect(page.locator(".row", { hasText: "sam-member" })).toHaveCount(0);
});

test("members: transfer asks first, waits, then the group follows the new host", async ({
  page,
}) => {
  await page.goto(room);
  await page.getByRole("link", { name: "Make host" }).click();
  await expect(page.getByRole("heading", { name: "Make sam-member the host?" })).toBeVisible();
  await page.getByRole("button", { name: "Send request" }).click();
  await expect(page.getByText("Asked sam-member to accept the group.")).toBeVisible();
  await expect(page.getByText("transfer requested")).toBeVisible();
  await fake(page).transferred("sam-member");
  await expect(page).toHaveURL(/#\/group\/sam-member\/claude-group\/members\/transferred/);
  await expect(page.getByText("sam-member is now the host.")).toBeVisible();
  await expect(page.getByText("Host transferred")).toBeVisible();
  await expect(page.getByRole("button", { name: "Turn off" })).toHaveCount(0);
});

test("members: proposal mode and the CLAUDE.md default are host toggles", async ({ page }) => {
  await page.goto(room);
  await page.getByRole("button", { name: "Proposal mode" }).click();
  await expect(page.getByText("Proposal mode is on.")).toBeVisible();
  await page.getByRole("button", { name: "Members get CLAUDE.md changes" }).click();
  await expect(page.locator(".tg.on")).toHaveCount(1);
});

// v2-ScenarioMembersEdge
test("members: a transfer not accepted leaves the host as is and can be asked again", async ({
  page,
}) => {
  await page.goto(`${room}/expired/sam-member`);
  await expect(page.getByText("sam-member did not accept in time, or declined.")).toBeVisible();
  await page.getByRole("link", { name: "Ask again" }).click();
  await expect(page.getByRole("heading", { name: "Make sam-member the host?" })).toBeVisible();
});

test("members: an unknown name sends nothing and keeps what was typed", async ({ page }) => {
  await page.goto(room);
  await page.getByLabel("GitHub name").fill("kim-devv");
  await page.getByRole("button", { name: "Invite", exact: true }).click();
  await expect(page.getByText("No GitHub user named kim-devv. Nothing was sent.")).toBeVisible();
  await expect(page.getByText("Check the spelling")).toBeVisible();
  await expect(page.getByLabel("GitHub name")).toHaveValue("kim-devv");
});

test("members: a member sees the list, no host actions and can leave", async ({ page }) => {
  await page.goto(guest);
  await expect(page.getByText("alex-host (you)")).toBeVisible();
  await expect(page.getByText("Group settings (host only)")).toBeVisible();
  await expect(page.getByText("On: changes go to a proposal pull request")).toBeVisible();
  await expect(page.getByRole("link", { name: "Remove" })).toHaveCount(0);
  await expect(page.getByRole("link", { name: "Make host" })).toHaveCount(0);
  await expect(page.getByLabel("GitHub name")).toHaveCount(0);
  await page.getByRole("link", { name: "Leave group" }).click();
  await expect(page.getByRole("heading", { name: "Leave sample-org/dev-rules?" })).toBeVisible();
});
