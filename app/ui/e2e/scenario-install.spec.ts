import { expect, test } from "@playwright/test";

// v2-ScenarioInstallHappy and v2-ScenarioInstallEdge: the steps that have a screen in this part
// are the sign-in window, the tray menu and the build without a client id. The rest (import
// banners, the CLI on PATH) have no window frames yet and wait for UI part 8c.
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
