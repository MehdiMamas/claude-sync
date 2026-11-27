import { expect, test } from "@playwright/test";

const at = (scenario: string) => `/?scenario=${scenario}#/notifications`;
const nav = (p: import("@playwright/test").Page) => p.getByRole("link", { name: /^Notifications/ });

// Parity 47: mark read, mark all read, clear all.
test("notifications: mark one read, mark all read, clear all", async ({ page }) => {
  await page.goto(at("notifications-plain"));
  await expect(page.getByText("New", { exact: true })).toHaveCount(2);
  await expect(nav(page)).toContainText("2");
  await page.getByRole("button", { name: "Mark read" }).first().click();
  await expect(page.getByText("New", { exact: true })).toHaveCount(1);
  await expect(nav(page)).toContainText("1");
  await page.getByRole("button", { name: "Mark all read" }).click();
  await expect(page.getByText("New", { exact: true })).toHaveCount(0);
  await expect(nav(page)).not.toContainText("1");
  await expect(page.getByRole("button", { name: "Mark all read" })).toBeDisabled();
  await page.getByRole("button", { name: "Clear all" }).click();
  await expect(page.getByRole("heading", { name: "All caught up" })).toBeVisible();
  await page.getByRole("link", { name: "Back to groups" }).click();
  await expect(page.getByRole("heading", { name: "Groups" })).toBeVisible();
});

test("notifications: a button opens its screen and marks the notice read", async ({ page }) => {
  await page.goto(at("notifications"));
  await expect(page.getByText("alex-host/claude-group: 3 lines to review")).toBeVisible();
  await page.getByRole("link", { name: "Open" }).first().click();
  await expect(page.getByRole("heading", { name: "Join a group" })).toBeVisible();
  await page.goto(at("notifications"));
  await page.getByRole("link", { name: "Review" }).click();
  await expect(page).toHaveURL(/group\/alex-host\/claude-group\/review/);
  await expect(nav(page)).toContainText("1");
});
