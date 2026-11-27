import { expect, type Page, test } from "@playwright/test";

const at = (scenario: string) => `/?scenario=${scenario}#/feedback`;
const fill = async (p: Page) => {
  await p.getByLabel("Title").fill("Group did not sync after sleep");
  await p.getByLabel("What happened").fill("The group did not sync after I woke the laptop.");
};
const send = (p: Page) => p.getByRole("button", { name: "Send", exact: true }).click();
const sent = (p: Page) =>
  p.evaluate(() => (window as unknown as { __sent: Record<string, unknown>[] }).__sent);

// v2-ScenarioFeedbackHappy: write, attach logs and a screenshot, send, see the issue number.
test("feedback: kind, title, text, logs and a pasted screenshot are sent; the form starts over", async ({
  page,
  context,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto(at("feedback"));
  await page.getByRole("button", { name: "Idea" }).click();
  await fill(page);
  await page.getByRole("button", { name: "Turn off" }).click();
  await page.getByRole("button", { name: "Turn on" }).click();
  await page.evaluate(async () => {
    const c = document.createElement("canvas");
    c.width = c.height = 4;
    const blob = await new Promise<Blob>((r) => c.toBlob((b) => r(b as Blob), "image/png"));
    await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
  });
  await page.getByRole("button", { name: "Paste" }).click();
  await expect(page.getByText(/Screenshot attached/)).toBeVisible();
  await send(page);
  await expect(page.getByText("Sent as issue #41. You will get a reply there.")).toBeVisible();
  const [one] = await sent(page);
  expect(one).toMatchObject({ kind: "idea", title: "Group did not sync after sleep" });
  expect(typeof one?.screenshot).toBe("string");
  expect(one?.logs).toBe("INFO sync pass done: nothing changed");
  await page.getByRole("button", { name: "Send another" }).click();
  await expect(page.getByLabel("Title")).toHaveValue("");
});

test("feedback: a blank title takes the first line of the text; a screenshot can be removed", async ({
  page,
}) => {
  await page.goto(at("feedback"));
  await page.getByLabel("What happened").fill("It crashed\nwhen I clicked Sync now");
  await send(page);
  await expect(page.getByText("Sent as issue #41.")).toBeVisible();
  const [one] = await sent(page);
  expect(one).toMatchObject({ kind: "bug", title: "It crashed", screenshot: null });
});

// v2-ScenarioFeedbackEdge: empty, refused, too large, rate limited, offline.
test("feedback edge: an empty report is not sent", async ({ page }) => {
  await page.goto(at("feedback"));
  await send(page);
  await expect(page.getByText("Write a few words first.")).toBeVisible();
  expect(await sent(page)).toBeUndefined();
  await page.getByLabel("What happened").fill("x");
  await expect(page.getByText("Write a few words first.")).toHaveCount(0);
});

test("feedback edge: refused keeps the text, Try again sends it again", async ({ page }) => {
  await page.goto(at("feedback-refused"));
  await fill(page);
  await send(page);
  await expect(
    page.getByText(
      "Feedback not sent: GitHub refused the issue (403: your account cannot open issues on this repo). Your text is kept.",
    ),
  ).toBeVisible();
  await page.getByRole("button", { name: "Copy text" }).click();
  await expect(page.getByText("Text copied")).toBeVisible();
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByText("Feedback not sent:")).toBeVisible();
  expect(await sent(page)).toHaveLength(2);
});

test("feedback edge: too large says what to remove; Edit report keeps the text", async ({
  page,
}) => {
  await page.goto(at("feedback-large"));
  await fill(page);
  await send(page);
  await expect(
    page.getByText(
      "The report is too large (413). Remove the screenshot or the logs and send again.",
    ),
  ).toBeVisible();
  await page.getByRole("button", { name: "Edit report" }).click();
  await expect(page.getByLabel("Title")).toHaveValue("Group did not sync after sleep");
});

test("feedback edge: rate limited and offline reports are saved and say so", async ({ page }) => {
  await page.goto(at("feedback-limited"));
  await fill(page);
  await send(page);
  await expect(page.getByText("Your report is saved and sent later.")).toBeVisible();
  await page.getByRole("button", { name: "Write another" }).click();
  await expect(page.getByLabel("Title")).toHaveValue("");
  await page.goto(at("feedback-queued"));
  await fill(page);
  await send(page);
  await expect(
    page.getByText(
      "You are offline. Your report is saved and is sent when the connection is back.",
    ),
  ).toBeVisible();
});

test("feedback: Capture takes the screen through the shell and sends it", async ({ page }) => {
  await page.goto(at("feedback"));
  await fill(page);
  await page.getByRole("button", { name: "Capture", exact: true }).click();
  await expect(page.getByText(/Screenshot attached/)).toBeVisible();
  await send(page);
  await expect(page.getByText("Sent as issue #41.")).toBeVisible();
  const [one] = await sent(page);
  expect(String(one?.screenshot).startsWith("iVBORw0KGgo")).toBe(true);
});
