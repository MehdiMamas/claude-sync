import { expect, test } from "@playwright/test";
import { fake } from "./fake";

const at = (scenario: string, tab = "") => `/?scenario=${scenario}#/settings${tab}`;

// v2-ScenarioPersonalHappy: offer, use it, what syncs, machines
test("personal memory: use the offered repo, switch a kind, rename and remove a machine", async ({
  page,
}) => {
  await page.goto(at("settings-offer", "/personal"));
  await expect(
    page.getByText("Found your private repo alex-host/claude-sync-personal"),
  ).toBeVisible();
  await page.getByRole("button", { name: "Use it" }).click();
  await expect(page.getByRole("heading", { name: "What syncs" })).toBeVisible();
  await expect(page.getByText("Synced", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Memory: turn off" }).click();
  await expect(page.getByRole("button", { name: "Memory: turn on" })).toBeVisible();
  await page.getByRole("button", { name: "Rename" }).click();
  await page.getByLabel("New name").fill("Work laptop");
  await page.getByLabel("New name").press("Enter");
  await expect(page.getByText("Work laptop")).toBeVisible();
  await page.getByRole("link", { name: "Remove" }).click();
  await expect(page.getByText("Desktop stops getting your personal memory")).toBeVisible();
  await page.getByRole("button", { name: "Remove", exact: true }).click();
  await expect(page.getByText("Desktop", { exact: true })).toHaveCount(0);
  // settings tabs of the scenario
  await page.getByRole("link", { name: "Advanced" }).click();
  await page.getByRole("link", { name: "Open", exact: true }).first().click();
  await expect(page.getByText("browser-tools")).toBeVisible();
  await page.getByRole("link", { name: "Back", exact: true }).click();
  await page.getByRole("link", { name: "Open", exact: true }).nth(1).click();
  await expect(page.getByRole("heading", { name: "SSH between your machines" })).toBeVisible();
});

test("personal memory: not now turns the offer off, the switch turns it on and off", async ({
  page,
}) => {
  await page.goto(at("settings-offer", "/personal"));
  await page.getByRole("button", { name: "Not now" }).click();
  await expect(page.getByText("Makes a private repo the first time.")).toBeVisible();
  await page.getByRole("button", { name: "Turn on" }).click();
  await expect(page.getByRole("heading", { name: "Your machines" })).toBeVisible();
  await page.getByRole("button", { name: "Turn off", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Your machines" })).toHaveCount(0);
});

// v2-ScenarioPersonalEdge: files that stay here, a repo that went public
test("personal memory: files that stay and a public repo stop with words", async ({ page }) => {
  await page.goto(at("settings-personal", "/personal"));
  await expect(page.getByText("memory/big-notes.md")).toBeVisible();
  await expect(page.getByText("over the 256 KiB limit per file")).toBeVisible();
  await expect(page.getByText("Conflicts: 1 file changed on two devices")).toBeVisible();

  await page.goto(at("settings-public", "/personal"));
  await expect(page.getByRole("alert")).toContainText("is public now. Nothing more is pushed");
  await expect(page.getByRole("button", { name: "Sync now" })).toHaveCount(0);
  await fake(page).fixed();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Sync now" })).toBeVisible();
});

// v2-ScenarioUpdateHappy: up to date, check, ready, restart
test("update: check now finds a verified download", async ({ page }) => {
  await page.goto(at("groups-invite", "/updates"));
  await expect(page.getByText("Version 2.0.0 · up to date · checked 1 h ago")).toBeVisible();
  await page.getByRole("button", { name: "Check now" }).click();
  await expect(
    page.getByText("Version 2.0.1 is ready. It installs when the app is idle, or now."),
  ).toBeVisible();
  await expect(page.getByText("Version 2.0.0 · 2.0.1 downloaded and verified")).toBeVisible();
  await page.getByRole("button", { name: "Restart to update" }).click();
  await expect(
    page.getByText("Installing from the window comes with the update step."),
  ).toBeVisible();
});

test("update: after the restart the window says what carried over", async ({ page }) => {
  await page.goto(at("update-updated", "/updates"));
  await expect(
    page.getByText("Updated to 2.0.1. Groups, jobs and sign-in carried over."),
  ).toBeVisible();
  await expect(page.getByText("Version 2.0.1 · up to date")).toBeVisible();
  await page.getByRole("button", { name: "What changed" }).click();
  expect(await fake(page).opened()).toEqual([
    "https://github.com/MehdiMamas/claude-sync/releases/latest",
  ]);
});

// v2-ScenarioUpdateEdge: signature fails
test("update: a failed signature is thrown away and Check again tries once more", async ({
  page,
}) => {
  await page.goto(at("update-error", "/updates"));
  await expect(page.getByRole("alert")).toContainText(
    "failed its signature check and was thrown away",
  );
  await expect(page.getByText("Version 2.0.0", { exact: true })).toBeVisible();
  await expect(page.getByRole("link", { name: "Report" })).toBeVisible();
  await page.getByRole("button", { name: "Check again" }).click();
  await expect(page.getByText("Version 2.0.1 is ready.")).toBeVisible();
});

// SignIn and SyncTick, the settings parts: interval, pause, resume, sign out
test("general: interval, pause and resume, start at login", async ({ page }) => {
  await page.goto(at("groups-invite"));
  const interval = page.getByLabel("Check for changes");
  await expect(interval).toHaveValue("60");
  await expect(interval.locator("option")).toHaveText([
    "Every 30 seconds",
    "Every minute",
    "Every 2 minutes",
    "Every 5 minutes",
    "Every 15 minutes",
  ]);
  await interval.selectOption("900");
  await expect(interval).toHaveValue("900");
  await page.getByRole("button", { name: "Turn off" }).click();
  await expect(page.getByRole("button", { name: "Turn on" }).first()).toBeVisible();
  await page.getByRole("button", { name: "Turn on" }).last().click();
  await expect(page.getByText("Syncing is paused on all your machines.")).toBeVisible();
  await page.getByRole("button", { name: "Resume" }).click();
  await expect(page.getByText("Syncing is paused")).toHaveCount(0);
});

test("account: sign out asks first, then goes to the sign-in screen", async ({ page }) => {
  await page.goto(at("groups-invite", "/account"));
  await expect(page.getByText("Signed in as alex-host")).toBeVisible();
  await page.getByRole("link", { name: "Sign out" }).click();
  await expect(page.getByText("Syncing stops on this machine. Your files stay.")).toBeVisible();
  await page.getByRole("link", { name: "Cancel" }).click();
  await expect(page.getByText("Signed in as alex-host")).toBeVisible();
  await page.getByRole("link", { name: "Sign out" }).click();
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(page.getByRole("button", { name: "Sign in with GitHub" })).toBeVisible();
});

test("account: App coverage and the links to GitHub", async ({ page }) => {
  await page.goto(at("settings-app", "/account"));
  await expect(page.getByText("Signed in through the app as alex-host")).toBeVisible();
  await expect(
    page
      .locator(".row", { hasText: "alex-host/claude-group" })
      .getByText("Covered", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("This group does not sync until the app is installed on it."),
  ).toBeVisible();
  await page.getByRole("button", { name: "Install the App" }).click();
  expect((await fake(page).opened()).length).toBe(1);

  await page.goto(at("groups-invite", "/account"));
  await page.getByRole("button", { name: "Open" }).click();
  expect(await fake(page).opened()).toEqual([
    "https://github.com/settings/connections/applications",
  ]);
});

test("notifications: switch, and Test says when the system blocks them", async ({ page }) => {
  await page.goto(at("groups-invite", "/notifications"));
  await page.getByRole("button", { name: "Turn off" }).click();
  await expect(page.getByRole("button", { name: "Turn on" })).toBeVisible();
  await page.getByRole("button", { name: "Test" }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "blocks notifications from claude-sync" }),
  ).toBeVisible();
});

test("advanced: packs, ssh and conflicts", async ({ page }) => {
  await page.goto(at("groups-invite", "/advanced/packs"));
  const gpu = page.locator(".row", { hasText: "gpu-tools" });
  await expect(gpu.getByText("step 2 of 3: download timed out")).toBeVisible();
  await gpu.getByRole("button", { name: "Try again" }).click();
  await expect(gpu.getByText("On", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "browser-tools: turn off" }).click();
  await expect(page.getByRole("button", { name: "browser-tools: turn on" })).toBeVisible();
  await expect(page.getByText("not for this machine")).toBeVisible();

  await page.goto(at("groups-invite", "/advanced/ssh"));
  await expect(page.getByLabel("This machine's alias")).toHaveCount(0);
  await page.getByRole("button", { name: "Turn on" }).click();
  await expect(page.getByLabel("This machine's alias")).toHaveValue("laptop");
  await expect(page.getByText("desktop · Tailscale, LAN fallback")).toBeVisible();

  await page.goto(at("groups-invite", "/advanced"));
  await expect(page.getByRole("link", { name: "Open (1)" })).toBeVisible();
  await page.getByRole("link", { name: "Open (1)" }).click();
  await expect(page.getByText("skills/notes/SKILL.md")).toBeVisible();
  await expect(page.getByRole("link", { name: "Share mine" })).toHaveAttribute(
    "href",
    "#/group/alex-host/claude-group/share/skills/notes/SKILL.md",
  );
  await page.getByRole("button", { name: "Open copy" }).click();
  await expect
    .poll(() => fake(page).opened())
    .toEqual(["/home/me/.claude-sync/state/conflicts/skills/notes/SKILL.md"]);
  await page.getByRole("button", { name: "Delete copy" }).click();
  await expect(page.getByText("No conflicts saved.")).toBeVisible();
});
