import { expect, test } from "@playwright/test";

// v2-ScenarioInstallHappy and v2-ScenarioInstallEdge: the sign-in window, the tray menu and Quit,
// the system's unsigned warning, the import banners, the v1 daemon wait and the build without a
// client id. Putting the CLI on PATH has no window.
test("first start opens Sign in; the tray menu is there", async ({ page }) => {
  await page.goto("/?signedOut");
  await expect(page.getByRole("heading", { name: "Share your Claude Code setup" })).toBeVisible();
  await page.goto("/?view=tray");
  await expect(page.getByRole("link", { name: "Quit" })).toBeVisible();
});

test("install edge: a build without a client id says so", async ({ page }) => {
  await page.goto("/?scenario=sign-in-unavailable");
  await expect(
    page.getByText("Sign-in is not available in this build (no GitHub client id)."),
  ).toBeVisible();
  await expect(page.getByText("Download claude-sync from its GitHub Releases page.")).toBeVisible();
});

test("install edge: unsigned warning, then Run anyway reaches the tray menu, Don't run quits", async ({
  page,
}) => {
  await page.goto("/?view=unsigned");
  await expect(page.getByRole("heading", { name: "Windows protected your PC" })).toBeVisible();
  await page.getByRole("link", { name: "Run anyway" }).click();
  await expect(page.getByRole("link", { name: "Quit" })).toBeVisible();
  await page.getByRole("link", { name: "Quit" }).click();
  await expect(page.getByText("claude-sync has quit.")).toBeVisible();
  await page.goto("/?view=unsigned");
  await page.getByRole("link", { name: "Don't run" }).click();
  await expect(page.getByText("claude-sync has quit.")).toBeVisible();
});

test("install edge: a v1 file that cannot be read is named and sign-in goes on", async ({
  page,
}) => {
  await page.goto("/?scenario=install-import-failed");
  await expect(
    page.getByText(
      "Could not read 1 of 9 old v1 files (jobs.json). It was skipped and kept in ~/.claude-sync/backup/; nothing was deleted.",
    ),
  ).toBeVisible();
  await page.getByRole("button", { name: "Send feedback with the log" }).click();
  await expect(page.getByText("Sign in first")).toBeVisible();
  await page.getByRole("button", { name: "Sign in with GitHub" }).click();
  await expect(page.getByText("WDJB-MJHT")).toBeVisible();
});

test("install edge: a v1 daemon that did not stop shows how to stop it; Try again goes on", async ({
  page,
}) => {
  await page.goto("/?scenario=install-v1-daemon");
  await expect(page.getByText("process 4182")).toBeVisible();
  await expect(page.getByText("kill 4182")).toBeVisible();
  await expect(page.getByText("taskkill /PID 4182 /F")).toBeVisible();
  await expect(page.getByText("systemctl --user restart claude-sync")).toBeVisible();
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByRole("heading", { name: "Share your Claude Code setup" })).toBeVisible();
});
