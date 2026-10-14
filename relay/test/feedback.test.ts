import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, it, mock } from "node:test";
import { POST as feedback } from "../api/feedback.ts";
import { resetAuthCache } from "../lib/auth.ts";
import { fenceFor, MAX_ISSUE_BODY } from "../lib/feedback.ts";
import { FEEDBACK_LIMIT, FEEDBACK_WINDOW_MS, resetRateLimits } from "../lib/ratelimit.ts";
import { type Call, type FetchMock, mockFetch, mockUser, RELAY_TOKEN, req, setEnv, USER_TOKEN } from "./helpers.ts";

const REPO = "/repos/MehdiMamas/claude-sync";
const MILESTONES = `${REPO}/milestones?state=open&per_page=100`;
const PNG = Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), Buffer.from("fake png body")]);
const JPEG = Buffer.concat([Buffer.from([0xff, 0xd8, 0xff, 0xe0]), Buffer.from("fake jpeg body")]);
const NOW = Date.parse("2026-10-13T15:00:00Z");
const ISSUE_URL = "https://github.com/MehdiMamas/claude-sync/issues/42";

interface Issue {
  title: string;
  body: string;
  labels: string[];
  milestone?: number;
}

let fm: FetchMock;
let logs: string[];

function post(body: unknown, token?: string | null): Request {
  return req("/api/feedback", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: typeof body === "string" ? body : JSON.stringify(body),
    token,
  });
}

function valid(extra: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    title: "Sync stops after sleep",
    description: "After the laptop wakes up, nothing syncs until I restart.",
    kind: "bug",
    appVersion: "0.3.0",
    os: "Windows 11 Pro 10.0.26200",
    logs: "2026-10-13T10:00:00Z INFO started\n2026-10-13T10:00:01Z WARN poll failed\n",
    ...extra,
  };
}

function issueCalls(): Array<Call & { body: Issue }> {
  return fm.calls.filter((c) => c.method === "POST" && c.url.endsWith(`${REPO}/issues`)) as Array<Call & { body: Issue }>;
}

function issueCall(): Call & { body: Issue } {
  const call = issueCalls()[0];
  assert.ok(call, "issue created");
  return call;
}

function patchCalls(): Array<Call & { body: { labels: string[]; milestone?: number } }> {
  return fm.calls.filter((c) => c.method === "PATCH") as Array<Call & { body: { labels: string[]; milestone?: number } }>;
}

/** The user's create answers `status`; the relay token's create succeeds. */
function refuseUser(status: number): void {
  fm.on("POST", `${REPO}/issues`, (call) =>
    call.headers.authorization === `Bearer ${USER_TOKEN}`
      ? Response.json({ message: "refused" }, { status })
      : Response.json({ number: 43, html_url: ISSUE_URL.replace("42", "43") }, { status: 201 }),
  );
}

function putCalls(): Call[] {
  return fm.calls.filter((c) => c.method === "PUT");
}

beforeEach(() => {
  setEnv();
  resetAuthCache();
  resetRateLimits();
  mock.timers.enable({ apis: ["Date"], now: NOW });
  fm = mockFetch();
  mockUser(fm, "MehdiMamas");
  fm.on("GET", `${REPO}/git/ref/heads/feedback-assets`, { ref: "refs/heads/feedback-assets", object: { sha: "a1" } });
  fm.on("PUT", new RegExp(`${REPO}/contents/feedback/`), (call) =>
    Response.json({ content: { path: call.url.split("/contents/")[1] } }, { status: 201 }),
  );
  fm.on("GET", MILESTONES, [
    { number: 1, title: "Sprint 1", state: "open", due_on: "2026-10-06T07:00:00Z" },
    { number: 2, title: "Sprint 2", state: "open", due_on: "2026-10-20T07:00:00Z" },
    { number: 3, title: "Sprint 3", state: "open", due_on: "2026-10-27T07:00:00Z" },
  ]);
  // As for a user without push access: GitHub drops the labels and milestone.
  fm.on("POST", `${REPO}/issues`, () =>
    Response.json({ number: 42, html_url: ISSUE_URL, labels: [], milestone: null }, { status: 201 }),
  );
  fm.on("PATCH", `${REPO}/issues/42`, (call) => Response.json({ number: 42, ...(call.body as object) }));
  logs = [];
  for (const level of ["log", "warn", "error", "info", "debug"] as const) {
    mock.method(console, level, (...args: unknown[]) => logs.push(args.map(String).join(" ")));
  }
});

afterEach(() => {
  fm.restore();
  mock.restoreAll();
  mock.timers.reset();
  for (const line of logs) {
    assert.ok(!line.includes(USER_TOKEN) && !line.includes(RELAY_TOKEN), `token leaked in log: ${line}`);
  }
});

describe("POST /api/feedback", () => {
  it("opens the issue as the caller, then sets labels and the nearest sprint with the relay token", async () => {
    const res = await feedback(post(valid()));
    assert.equal(res.status, 201);
    assert.deepEqual(await res.json(), { number: 42, url: ISSUE_URL, asUser: true });

    assert.equal(issueCalls().length, 1);
    const call = issueCall();
    assert.equal(call.headers.authorization, `Bearer ${USER_TOKEN}`);
    assert.equal(call.body.title, "Sync stops after sleep");
    assert.deepEqual(call.body.labels, ["feedback", "type:bug"]);
    assert.equal(call.body.milestone, 2);

    const patches = patchCalls();
    assert.equal(patches.length, 1);
    assert.equal(patches[0]?.headers.authorization, `Bearer ${RELAY_TOKEN}`);
    assert.deepEqual(patches[0]?.body, { labels: ["feedback", "type:bug"], milestone: 2 });
    // The labels are set after the issue exists.
    assert.ok(fm.calls.indexOf(call) < fm.calls.indexOf(patches[0] as Call));

    const body = call.body.body;
    assert.match(body, /^After the laptop wakes up/);
    // GitHub shows the author, so there is no Reporter row.
    assert.ok(!body.includes("Reporter"));
    assert.match(body, /\| Kind \| bug \|/);
    assert.match(body, /\| App version \| `0\.3\.0` \|/);
    assert.match(body, /\| OS \| `Windows 11 Pro 10\.0\.26200` \|/);
    assert.match(body, /<details>\n<summary>Logs \(redacted\)<\/summary>\n\n```text\n2026-10-13T10:00:00Z INFO started\n/);
    assert.match(body, /```\n\n<\/details>$/);
    // No screenshot: nothing is committed.
    assert.equal(putCalls().length, 0);
    assert.equal(fm.calls.filter((c) => c.url.includes("/git/")).length, 0);
  });

  it("labels ideas type:idea", async () => {
    assert.equal((await feedback(post(valid({ kind: "idea" })))).status, 201);
    assert.deepEqual(issueCall().body.labels, ["feedback", "type:idea"]);
    assert.deepEqual(patchCalls()[0]?.body.labels, ["feedback", "type:idea"]);
  });

  it("skips the label call when GitHub kept the labels and milestone (a collaborator)", async () => {
    fm.on("POST", `${REPO}/issues`, () =>
      Response.json(
        { number: 42, html_url: ISSUE_URL, labels: [{ name: "feedback" }, { name: "type:bug" }], milestone: { number: 2 } },
        { status: 201 },
      ),
    );
    assert.equal((await feedback(post(valid()))).status, 201);
    assert.equal(patchCalls().length, 0);
  });

  it("still answers 201 when setting the labels fails, and logs it", async () => {
    fm.on("PATCH", `${REPO}/issues/42`, 403);
    const res = await feedback(post(valid()));
    assert.equal(res.status, 201);
    assert.deepEqual(await res.json(), { number: 42, url: ISSUE_URL, asUser: true });
    assert.ok(logs.some((l) => l.includes("feedback labels") && l.includes("403")), logs.join("\n"));
  });

  it("files the issue with the relay token, with a Reporter row, when GitHub refuses the caller with 403", async () => {
    refuseUser(403);
    const res = await feedback(post(valid({ screenshot: PNG.toString("base64") })));
    assert.equal(res.status, 201);
    assert.deepEqual(await res.json(), { number: 43, url: ISSUE_URL.replace("42", "43"), asUser: false });

    const [asUser, asRelay] = issueCalls();
    assert.equal(asUser?.headers.authorization, `Bearer ${USER_TOKEN}`);
    assert.equal(asRelay?.headers.authorization, `Bearer ${RELAY_TOKEN}`);
    assert.deepEqual(asRelay?.body.labels, ["feedback", "type:bug"]);
    assert.equal(asRelay?.body.milestone, 2);
    assert.match(asRelay?.body.body ?? "", /\| Reporter \| \[MehdiMamas\]\(https:\/\/github\.com\/MehdiMamas\) \|/);
    // The screenshot is committed once, and both bodies link it.
    assert.equal(putCalls().length, 1);
    assert.match(asRelay?.body.body ?? "", /!\[screenshot\]\(https:\/\/github\.com\/MehdiMamas\/claude-sync\/blob\/feedback-assets\//);
    // The relay token sets labels at creation; no follow-up call.
    assert.equal(patchCalls().length, 0);
    assert.ok(logs.some((l) => l.includes("as MehdiMamas returned 403")), logs.join("\n"));
  });

  it("falls back to the relay token for 401, 404, 410, 422 and 429 from the caller's create", async () => {
    for (const status of [401, 404, 410, 422, 429]) {
      resetRateLimits();
      fm.calls.length = 0;
      refuseUser(status);
      const res = await feedback(post(valid()));
      assert.equal(res.status, 201, `status ${status}`);
      assert.equal(((await res.json()) as { asUser: boolean }).asUser, false);
      assert.equal(issueCalls()[1]?.headers.authorization, `Bearer ${RELAY_TOKEN}`, `status ${status}`);
    }
  });

  it("does not fall back on a GitHub 5xx: answers 502 so the app retries later", async () => {
    refuseUser(503);
    const res = await feedback(post(valid()));
    assert.equal(res.status, 502);
    assert.equal(issueCalls().length, 1);
    assert.equal(issueCall().headers.authorization, `Bearer ${USER_TOKEN}`);
  });

  it("commits a PNG screenshot to feedback-assets and links it in the issue", async () => {
    const res = await feedback(post(valid({ screenshot: PNG.toString("base64") })));
    assert.equal(res.status, 201);
    const puts = putCalls();
    assert.equal(puts.length, 1);
    const put = puts[0] as Call & { body: { content: string; branch: string; message: string } };
    const path = put.url.split("/contents/")[1] ?? "";
    assert.match(path, /^feedback\/2026-10\/[0-9a-f-]{36}\.png$/);
    assert.equal(put.body.branch, "feedback-assets");
    assert.deepEqual(Buffer.from(put.body.content, "base64"), PNG);
    assert.equal(put.headers.authorization, `Bearer ${RELAY_TOKEN}`);
    assert.ok(
      issueCall().body.body.includes(
        `![screenshot](https://github.com/MehdiMamas/claude-sync/blob/feedback-assets/${path}?raw=true)`,
      ),
    );
  });

  it("accepts a JPEG given as a data URL and stores it as .jpg", async () => {
    const res = await feedback(post(valid({ screenshot: `data:image/jpeg;base64,${JPEG.toString("base64")}` })));
    assert.equal(res.status, 201);
    assert.match(putCalls()[0]?.url ?? "", /\/feedback\/2026-10\/[0-9a-f-]{36}\.jpg$/);
  });

  it("creates feedback-assets from the default branch head when it is missing", async () => {
    fm.on("GET", `${REPO}/git/ref/heads/feedback-assets`, 404);
    fm.on("GET", REPO, { default_branch: "main" });
    fm.on("GET", `${REPO}/git/ref/heads/main`, { ref: "refs/heads/main", object: { sha: "c0ffee" } });
    fm.on("POST", `${REPO}/git/refs`, (call) => Response.json({ ref: (call.body as { ref: string }).ref }, { status: 201 }));

    const res = await feedback(post(valid({ screenshot: PNG.toString("base64") })));
    assert.equal(res.status, 201);
    const create = fm.calls.find((c) => c.method === "POST" && c.url.endsWith("/git/refs"));
    assert.deepEqual(create?.body, { ref: "refs/heads/feedback-assets", sha: "c0ffee" });
    // The branch exists before the screenshot is committed to it.
    assert.ok(fm.calls.indexOf(create as Call) < fm.calls.indexOf(putCalls()[0] as Call));
  });

  it("still files the issue when the screenshot cannot be stored", async () => {
    fm.on("PUT", new RegExp(`${REPO}/contents/feedback/`), 409);
    const res = await feedback(post(valid({ screenshot: PNG.toString("base64") })));
    assert.equal(res.status, 201);
    assert.match(issueCall().body.body, /_The screenshot could not be stored\._/);
  });

  it("puts the issue in the latest sprint when every sprint is past", async () => {
    fm.on("GET", MILESTONES, [
      { number: 1, title: "Sprint 1", state: "open", due_on: "2026-09-01T07:00:00Z" },
      { number: 2, title: "Sprint 2", state: "open", due_on: "2026-09-15T07:00:00Z" },
      { number: 7, title: "Someday", state: "open", due_on: "2027-01-01T07:00:00Z" },
    ]);
    assert.equal((await feedback(post(valid()))).status, 201);
    assert.equal(issueCall().body.milestone, 2);
  });

  it("sets no milestone when there is no sprint", async () => {
    fm.on("GET", MILESTONES, []);
    assert.equal((await feedback(post(valid()))).status, 201);
    assert.equal("milestone" in issueCall().body, false);
    assert.deepEqual(patchCalls()[0]?.body, { labels: ["feedback", "type:bug"] });
  });

  it("sets no milestone when listing milestones fails", async () => {
    fm.on("GET", MILESTONES, 500);
    assert.equal((await feedback(post(valid()))).status, 201);
    assert.equal("milestone" in issueCall().body, false);
  });

  it("redacts secrets in the logs, title and description again", async () => {
    const pat = "github_pat_" + "Z".repeat(60);
    const res = await feedback(
      post(
        valid({
          title: `token ${pat}`,
          description: `my key is sk-ant-api03-${"k".repeat(30)}`,
          logs: `GET /user Authorization: Bearer ${"t".repeat(40)}\n{"access_token":"secret-value"}\nghp_${"a".repeat(36)}\n`,
        }),
      ),
    );
    assert.equal(res.status, 201);
    const { title, body } = issueCall().body;
    assert.equal(title, "token [REDACTED:github_pat]");
    assert.ok(body.includes("my key is [REDACTED:sk-ant]"));
    assert.ok(body.includes("Authorization: Bearer [REDACTED:bearer]"));
    assert.ok(body.includes('{"access_token":"[REDACTED:token-field]"}'));
    assert.ok(body.includes("[REDACTED:ghp]"));
    for (const secret of [pat, "t".repeat(40), "secret-value", "a".repeat(36)]) {
      assert.ok(!body.includes(secret), "secret removed");
    }
  });

  it("keeps the last 200 KB of the logs, inline when they fit, else in a committed .log file", async () => {
    const line = "2026-10-13T10:00:00Z INFO poll ok ".padEnd(99, ".") + "\n"; // 100 bytes
    const big = "FIRST-LINE\n" + line.repeat(3000) + "LAST-LINE\n"; // ~300 KB
    const res = await feedback(post(valid({ logs: big })));
    assert.equal(res.status, 201);

    // The full log file holds the last 200 KB.
    const put = putCalls()[0] as Call & { body: { content: string } };
    assert.match(put.url, /\/feedback\/2026-10\/[0-9a-f-]{36}\.log$/);
    const stored = Buffer.from(put.body.content, "base64").toString("utf8");
    assert.ok(Buffer.byteLength(stored) <= 200 * 1024 + 64);
    assert.ok(stored.endsWith("LAST-LINE\n"));
    assert.ok(!stored.includes("FIRST-LINE"));
    assert.match(stored, /^\[… \d+ earlier bytes cut\]\n/);

    // The issue body shows the end and links the file, under GitHub's body limit.
    const body = issueCall().body.body;
    assert.ok(body.length <= MAX_ISSUE_BODY, `body is ${body.length} chars`);
    assert.ok(body.includes("LAST-LINE"));
    assert.match(body, /\[Full logs \(last 200 KB\)\]\(https:\/\/github\.com\/MehdiMamas\/claude-sync\/blob\/feedback-assets\/feedback\/2026-10\/[0-9a-f-]{36}\.log\?raw=true\)/);
  });

  it("fences logs so backticks in them cannot close the block", async () => {
    const res = await feedback(post(valid({ logs: "a ``` b\nc ```` d\n</details>\n" })));
    assert.equal(res.status, 201);
    const body = issueCall().body.body;
    assert.ok(body.includes("`````text\na ``` b\nc ```` d\n</details>\n\n`````\n\n</details>"));
    assert.equal(fenceFor("no ticks"), "```");
    assert.equal(fenceFor("x `y` ``z``"), "```");
    assert.equal(fenceFor("````"), "`````");
  });

  it("keeps table cells safe from markdown in appVersion and os", async () => {
    assert.equal((await feedback(post(valid({ appVersion: "1.0`|<b>", os: "mac\nOS | x" })))).status, 201);
    const body = issueCall().body.body;
    assert.ok(body.includes("| App version | `1.0b` |"));
    assert.ok(body.includes("| OS | `mac OS  x` |"));
  });

  it("rejects invalid bodies with 400", async () => {
    const cases: Array<[unknown, string]> = [
      ["not json", "body must be JSON"],
      [[1, 2], "body must be a JSON object"],
      [valid({ title: "x".repeat(201) }), "title is longer than 200 characters"],
      [valid({ title: "   " }), "title is required"],
      [valid({ description: "d".repeat(20_001) }), "description is longer than 20000 characters"],
      [valid({ kind: "question" }), 'kind must be "bug" or "idea"'],
      [valid({ appVersion: undefined }), "appVersion is required"],
      [valid({ os: 11 }), "os must be a string"],
      [valid({ logs: 5 }), "logs must be a string"],
      [valid({ screenshot: "!!!not base64!!!" }), "screenshot is not valid base64"],
      [valid({ screenshot: Buffer.from("GIF89a....").toString("base64") }), "screenshot must be a PNG or JPEG image"],
    ];
    for (const [body, error] of cases) {
      const res = await feedback(post(body));
      assert.equal(res.status, 400, error);
      assert.deepEqual(await res.json(), { error });
    }
    assert.equal(fm.calls.filter((c) => c.method !== "GET" || !c.url.endsWith("/user")).length, 0);
  });

  it("accepts a title of exactly 200 characters and a description of 20000", async () => {
    const res = await feedback(post(valid({ title: "t".repeat(200), description: "d".repeat(20_000) })));
    assert.equal(res.status, 201);
  });

  it("413 for a screenshot over 3 MB", async () => {
    const big = Buffer.concat([PNG, Buffer.alloc(3 * 1024 * 1024)]);
    const res = await feedback(post(valid({ screenshot: big.toString("base64") })));
    assert.equal(res.status, 413);
    assert.equal(putCalls().length, 0);
  });

  it(`allows ${FEEDBACK_LIMIT} posts per login per hour, then 429 with Retry-After`, async () => {
    for (let i = 0; i < FEEDBACK_LIMIT; i++) assert.equal((await feedback(post(valid()))).status, 201, `post ${i + 1}`);
    const res = await feedback(post(valid()));
    assert.equal(res.status, 429);
    assert.equal(res.headers.get("retry-after"), String(FEEDBACK_WINDOW_MS / 1000));
    assert.equal(fm.calls.filter((c) => c.url.endsWith("/issues")).length, FEEDBACK_LIMIT);

    // Another login has its own budget.
    resetAuthCache();
    mockUser(fm, "friend-one");
    assert.equal((await feedback(post(valid()))).status, 201);

    // The window slides.
    resetAuthCache();
    mockUser(fm, "MehdiMamas");
    mock.timers.tick(FEEDBACK_WINDOW_MS + 1);
    assert.equal((await feedback(post(valid()))).status, 201);
  });

  it("does not count invalid posts against the limit", async () => {
    for (let i = 0; i < 12; i++) await feedback(post(valid({ kind: "nope" })));
    assert.equal((await feedback(post(valid()))).status, 201);
  });

  it("401 for an unauthorised caller, before anything is written", async () => {
    assert.equal((await feedback(post(valid(), null))).status, 401);
    assert.equal((await feedback(post(valid(), "gho_wrong"))).status, 401);
    assert.equal(fm.calls.filter((c) => c.method !== "GET").length, 0);
  });

  it("files feedback from any GitHub user, as that user", async () => {
    mockUser(fm, "stranger");
    assert.equal((await feedback(post(valid()))).status, 201);
    assert.equal(issueCall().headers.authorization, `Bearer ${USER_TOKEN}`);
    assert.ok(!issueCall().body.body.includes("stranger"));
  });

  it("403 for a blocked login, before anything is written", async () => {
    setEnv({ BLOCKED_GITHUB_USERS: "stranger" });
    mockUser(fm, "stranger");
    assert.equal((await feedback(post(valid()))).status, 403);
    assert.equal(fm.calls.filter((c) => c.method !== "GET").length, 0);
  });

  it("files into FEEDBACK_REPO when set", async () => {
    setEnv({ FEEDBACK_REPO: "MehdiMamas/claude-sync-feedback" });
    fm.on("GET", "/repos/MehdiMamas/claude-sync-feedback/milestones?state=open&per_page=100", []);
    fm.on("POST", "/repos/MehdiMamas/claude-sync-feedback/issues", () =>
      Response.json({ number: 1, html_url: "https://github.com/MehdiMamas/claude-sync-feedback/issues/1" }, { status: 201 }),
    );
    fm.on("PATCH", "/repos/MehdiMamas/claude-sync-feedback/issues/1", {});
    const res = await feedback(post(valid()));
    assert.deepEqual(await res.json(), {
      number: 1,
      url: "https://github.com/MehdiMamas/claude-sync-feedback/issues/1",
      asUser: true,
    });
  });

  it("502 when GitHub refuses the issue from both the caller and the relay", async () => {
    fm.on("POST", `${REPO}/issues`, 422);
    assert.equal((await feedback(post(valid()))).status, 502);
    assert.equal(issueCalls().length, 2);
  });
});
