import { expect, test } from "@playwright/test";
import { fake } from "./fake";

// v2-ScenarioSyncTickHappy
test("sync tick: Sync now, then the result", async ({ page }) => {
  await page.goto("/?scenario=groups&hold");
  await page.getByRole("button", { name: "Sync now" }).click();
  await expect(page.getByText("Syncing…")).toBeVisible();
  await expect(page.getByText("Checking GitHub…").first()).toBeVisible();
  await expect(page.getByText("Unchanged groups cost no GitHub requests.")).toBeVisible();
  await fake(page).release();
  await expect(
    page
      .getByRole("status")
      .filter({ hasText: "Synced: 2 files updated in alex-host/claude-group" }),
  ).toBeVisible();
  await expect(page.getByText("Synced just now · 2 files updated")).toBeVisible();
});

test("sync tick: loading shows first, a tick updates the open window", async ({ page }) => {
  await page.goto("/?scenario=groups-loading");
  await expect(page.getByText("Loading your groups…")).toBeVisible();
  await page.goto("/?scenario=groups");
  await expect(page.getByText("GitHub asked us to slow down")).toHaveCount(0);
  await page.evaluate(() =>
    window.__fake.patch({ rateLimitedUntil: Math.floor(Date.now() / 1000) + 600 }),
  );
  await expect(page.getByText("GitHub asked us to slow down.")).toBeVisible();
});

test("tray menu lists the items of Parity row 56, Quit ends", async ({ page }) => {
  await page.goto("/?view=tray");
  const items = page.locator(".tray a");
  await expect(items).toHaveText([
    "Sync now",
    "Open claude-sync",
    "Notifications (2)",
    "Pause syncing",
    "Turn groups off",
    "Send feedback…",
    "Quit",
  ]);
  await expect(page.getByText("Synced 2 min ago")).toBeVisible();
  await page.getByRole("link", { name: "Quit" }).click();
  await expect(
    page.getByText("claude-sync has quit. Nothing syncs until it starts again."),
  ).toBeVisible();
});

// v2-ScenarioSyncTickEdge
test("sync tick edge: rate limited waits until the reset", async ({ page }) => {
  await page.goto("/?scenario=groups-rate-limited");
  await expect(
    page.getByText(
      "GitHub asked us to slow down. Syncing waits until 14:05 and starts again by itself.",
    ),
  ).toBeVisible();
  await expect(page.getByText("Waiting for GitHub until 14:05")).toBeVisible();
  await expect(page.getByRole("link", { name: "Join", exact: true })).toHaveCount(0);
});

test("sync tick edge: offline skips, Try now syncs", async ({ page }) => {
  await page.goto("/?scenario=groups-offline");
  await expect(
    page.getByText(
      "You are offline. Your files keep working; syncing starts again when the connection is back.",
    ),
  ).toBeVisible();
  await expect(page.getByText("Offline · last sync 40 min ago")).toBeVisible();
  await page.getByRole("button", { name: "Try now" }).click();
  await expect(page.getByText("You are offline.")).toHaveCount(0);
});

test("sync tick edge: failed sync says why and offers a report", async ({ page }) => {
  await page.goto("/?scenario=groups-sync-error");
  await expect(
    page.getByText(
      "alex-host/claude-group did not sync: GitHub answered 500. It tries again in 1 minute.",
    ),
  ).toBeVisible();
  await expect(page.getByText("Last sync failed 1 min ago")).toBeVisible();
  await page.getByRole("link", { name: "Report" }).click();
  await expect(page).toHaveURL(/#\/feedback$/);
});

test("sync tick edge: plugin problem warns with Retry", async ({ page }) => {
  await page.goto("/?scenario=groups-plugins");
  await expect(
    page.getByText(
      "2 group plugins are waiting: one could not install (repo-tools), one has a file over the 1 MiB limit (big-pack).",
    ),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Details" })).toBeVisible();
});
