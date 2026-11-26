import { expect, test } from "@playwright/test";

const review = "#/group/alex-host/claude-group/review";

// v2-ScenarioReviewHappy
test("review: accept all writes nothing more to ask", async ({ page }) => {
  await page.goto(`/?scenario=group${review}`);
  await expect(page.getByText("CLAUDE.md from sam-member · 1 h ago")).toBeVisible();
  await expect(page.getByText("+ Prefer small pull requests.")).toBeVisible();
  await expect(page.getByText("- Use tabs.")).toBeVisible();
  await page.getByRole("button", { name: "Accept all" }).click();
  await expect(page.getByText("3 lines accepted")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Nothing to review" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Review" })).not.toContainText("3");
  await page.getByRole("link", { name: "Back to overview" }).click();
  await expect(page.getByRole("heading", { name: "Needs you" })).toBeVisible();
  await expect(page.getByText("CLAUDE.md line")).toHaveCount(0);
});

test("review: decline keeps the lines out", async ({ page }) => {
  await page.goto(`/?scenario=group${review}`);
  await page.getByRole("button", { name: "Decline" }).click();
  await expect(page.getByText("3 lines declined")).toBeVisible();
  await expect(page.getByText("A later change to them asks again.")).toBeVisible();
});

test("review: choose line by line", async ({ page }) => {
  await page.goto(`/?scenario=group${review}`);
  await page.getByRole("button", { name: "Choose lines" }).click();
  await page.getByRole("button", { name: "Accept" }).nth(1).click(); // now declined
  await page.getByRole("button", { name: "Apply my choices" }).click();
  await expect(page.getByText("1 line accepted, 2 lines declined")).toBeVisible();
});

test("review: the choice for this group is kept, explain answers", async ({ page }) => {
  await page.goto(`/?scenario=group${review}`);
  await page.getByRole("button", { name: "Auto-accept" }).click();
  await expect(page.getByRole("button", { name: "Auto-accept" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await page.getByRole("button", { name: "Explain" }).click();
  await expect(page.getByText("Adds two working rules")).toBeVisible();
});

test("review: a member reviews the same way", async ({ page }) => {
  await page.goto("/?scenario=review-guest#/group/sample-org/dev-rules/review");
  await expect(page.getByText("CLAUDE.md from alex-host · 3 h ago")).toBeVisible();
  await expect(page.getByRole("button", { name: "Choose lines" })).toHaveCount(0);
  await page.getByRole("button", { name: "Accept", exact: true }).click();
  await expect(page.getByText("1 line accepted")).toBeVisible();
});

// v2-ScenarioReviewEdge
test("review: a clash with my own line is resolved before the lines", async ({ page }) => {
  await page.goto(`/?scenario=group&clash${review}`);
  await expect(page.getByText("1 line clashes with your own CLAUDE.md.")).toBeVisible();
  await expect(page.getByText("Use two spaces.")).toBeVisible();
  await page.getByRole("button", { name: "Keep mine" }).click();
  await expect(page.getByText("clashes with your own CLAUDE.md")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Accept all" })).toBeVisible();
});

test("review: explain without Claude signed in says so and review still works", async ({
  page,
}) => {
  await page.goto(`/?scenario=group&noClaude${review}`);
  await page.getByRole("button", { name: "Explain" }).click();
  await expect(page.getByText("Claude is not signed in on this machine.")).toBeVisible();
  await page.getByRole("button", { name: "Accept all" }).click();
  await expect(page.getByText("3 lines accepted")).toBeVisible();
});
