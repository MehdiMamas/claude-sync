import { expect, test } from "@playwright/test";
import { fake } from "./fake";

// v2-ScenarioSignInHappy
test("sign in with GitHub: happy path", async ({ page }) => {
  await page.goto("/?scenario=signed-out");
  await expect(page.getByRole("heading", { name: "Share your Claude Code setup" })).toBeVisible();
  await expect(page.getByText("Needs a free GitHub account.")).toBeVisible();

  await page.getByRole("button", { name: "Sign in with GitHub" }).click();
  await expect(page.getByRole("heading", { name: "Enter this code on GitHub" })).toBeVisible({
    timeout: 2000,
  });
  await expect(page.getByText("WDJB-MJHT")).toBeVisible();
  await expect(page.getByText("The code works for 15 minutes.")).toBeVisible();

  await page.getByRole("button", { name: "Copy code" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Code copied" })).toBeVisible();
  await page.getByRole("button", { name: "Open GitHub again" }).click();
  expect(await fake(page).opened()).toEqual(["https://github.com/login/device"]);

  await fake(page).approve();
  await expect(page.getByRole("heading", { name: "Signed in as alex-host" })).toBeVisible();
  await expect(page.getByText("Loading your groups…")).toBeVisible();
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page.getByRole("heading", { name: "Groups" })).toBeVisible();
  await expect(page.getByText("alex-host/claude-group")).toBeVisible();
});

test("sign in: cancel goes back to the first screen", async ({ page }) => {
  await page.goto("/?scenario=sign-in-code");
  await page.getByRole("button", { name: "Cancel" }).click();
  await expect(page.getByRole("button", { name: "Sign in with GitHub" })).toBeVisible();
});

// v2-ScenarioSignInEdge
test("sign in with GitHub: edge cases", async ({ page }) => {
  await page.goto("/?scenario=signed-out");
  await page.getByRole("button", { name: "Sign in with GitHub" }).click();
  await page.getByText("WDJB-MJHT").waitFor();

  await fake(page).end("expired");
  await expect(page.getByText("The code expired before it was entered.")).toBeVisible();
  await expect(page.getByText("A new code takes a second.")).toBeVisible();
  await page.getByRole("button", { name: "Get a new code" }).click();
  await expect(page.getByText("WDJB-MJHT")).toBeVisible();

  await fake(page).end("denied");
  await expect(page.getByText("You chose Cancel on GitHub. Nothing was stored.")).toBeVisible();
  await expect(
    page.getByText("claude-sync needs the repo scope to read your private group repos."),
  ).toBeVisible();
  await page.getByRole("button", { name: "Start again" }).click();
  await expect(page.getByText("WDJB-MJHT")).toBeVisible();

  await fake(page).end("offline");
  await expect(page.getByText("Cannot reach github.com. Check the connection.")).toBeVisible();
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByText("WDJB-MJHT")).toBeVisible();
});

test("sign in edge: 401 later says sign-in expired, groups stay", async ({ page }) => {
  await page.goto("/?scenario=sign-in-expired");
  await expect(
    page.getByText(
      "GitHub no longer accepts your sign-in. Syncing is stopped until you sign in again.",
    ),
  ).toBeVisible();
  await expect(page.getByText("Your groups and files stay as they are.")).toBeVisible();
  await page.getByRole("button", { name: "Sign in with GitHub" }).click();
  await expect(page.getByText("WDJB-MJHT")).toBeVisible();
});

test("sign in edge: a build without a client id says so and stops", async ({ page }) => {
  await page.goto("/?scenario=sign-in-unavailable");
  await expect(
    page.getByText("Sign-in is not available in this build (no GitHub client id)."),
  ).toBeVisible();
  await expect(page.getByRole("heading", { name: "Use a release build" })).toBeVisible();
  await expect(page.getByRole("button")).toHaveCount(0);
});

test("query switches keep working: ?signedOut", async ({ page }) => {
  await page.goto("/?signedOut");
  await expect(page.getByRole("button", { name: "Sign in with GitHub" })).toBeVisible();
});
