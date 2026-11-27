import { expect, test } from "@playwright/test";

const host = "#/group/alex-host/claude-group";

// v2-ScenarioJobsHappy: review a job, accept it, run it
test("jobs: review, accept and schedule, then run now shows the result", async ({ page }) => {
  await page.goto(`/?scenario=jobs${host}/jobs`);
  const row = page.locator(".row", { hasText: "nightly-lint" });
  await expect(row.getByText("Waiting for your yes")).toBeVisible();
  await row.getByRole("link", { name: "Review" }).click();
  await expect(page.getByText("Lint the repo")).toBeVisible();
  await expect(page.getByText("Nothing runs until you accept.")).toBeVisible();
  await page.getByRole("button", { name: "Accept and schedule" }).click();
  await expect(page.getByText("Scheduled: nightly-lint runs every day at 03:00")).toBeVisible();
  await expect(page.locator(".row", { hasText: "nightly-lint" }).getByText("On")).toBeVisible();
  const report = page.locator(".row", { hasText: "weekly-report" });
  await report.getByRole("button", { name: "Run now" }).click();
  await expect(page.getByText("weekly-report started")).toBeVisible();
  await expect(page.getByText("weekly-report finished")).toBeVisible();
});

// v2-ScenarioJobsEdge: failed, skipped, off, bypassPermissions, auto-accept
test("jobs: failed, skipped and off states, the bypass warning and the auto-accept switch", async ({
  page,
}) => {
  await page.goto(`/?scenario=jobs-edge${host}/jobs`);
  await expect(
    page.locator(".row", { hasText: "flaky" }).getByText("Failed", { exact: true }),
  ).toBeVisible();
  await expect(
    page.locator(".row", { hasText: "offline" }).getByText("Skipped", { exact: true }),
  ).toBeVisible();
  const idle = page.locator(".row", { hasText: "idle" });
  await expect(idle.getByText("Off", { exact: true })).toBeVisible();
  await idle.getByRole("button", { name: "Turn on" }).click();
  await expect(idle.getByText("On", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Turn on" }).last().click();
  await expect(page.getByText("run on your devices without your review")).toBeVisible();
  await page
    .locator(".row", { hasText: "nightly-lint" })
    .getByRole("link", { name: "Review" })
    .click();
  await expect(page.getByText("Permission mode bypassPermissions")).toBeVisible();
  await expect(page.getByText("--chrome --model sonnet")).toBeVisible();
  await page.getByRole("link", { name: "Not now" }).click();
  await expect(page.getByRole("heading", { name: "Jobs" })).toBeVisible();
});
