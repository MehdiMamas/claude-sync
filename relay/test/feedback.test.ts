import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, it, mock } from "node:test";
import { POST as feedback } from "../api/feedback.ts";
import { resetAuthCache } from "../lib/auth.ts";
import { FEEDBACK_LIMIT, FEEDBACK_WINDOW_MS, resetRateLimits } from "../lib/ratelimit.ts";
import { type Call, type FetchMock, mockFetch, mockUser, RELAY_TOKEN, req, setEnv, USER_TOKEN } from "./helpers.ts";

const REPO = "/repos/MehdiMamas/claude-sync";
// The private attachments repo: `<FEEDBACK_REPO>-feedback` by default.
const ATT = "/repos/MehdiMamas/claude-sync-feedback";
const MILESTONES = `${REPO}/milestones?state=open&per_page=100`;
const PNG = Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), Buffer.from("fake png body")]);
const JPEG = Buffer.concat([Buffer.from([0xff, 0xd8, 0xff, 0xe0]), Buffer.from("fake jpeg body")]);
const NOW = Date.parse("2026-10-13T15:00:00Z");
const ISSUE_URL = "https://github.com/MehdiMamas/claude-sync/issues/42";

interface Issue {
  title: string;
  body: string;
  labels?: string[];
  milestone?: number;
}

interface Patch {
  labels: string[];
  milestone?: number;
  body?: string;
}

interface TreeEntry {
  path: string;
  mode: string;
  type: string;
  sha: string;
}

let fm: FetchMock;
let logs: string[];
let blobs: Map<string, Buffer>;

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

function patchCalls(): Array<Call & { body: Patch }> {
  return fm.calls.filter((c) => c.method === "PATCH" && /\/repos\/MehdiMamas\/claude-sync\/issues\/\d+$/.test(c.url)) as Array<
    Call & { body: Patch }
  >;
}

function attachmentCalls(): Call[] {
  return fm.calls.filter((c) => c.url.includes(`${ATT}`));
}

/** The files of the last attachments commit, by path. */
function committed(): Map<string, Buffer> {
  const trees = fm.calls.filter((c) => c.method === "POST" && c.url.endsWith(`${ATT}/git/trees`));
  const tree = trees.at(-1) as (Call & { body: { base_tree: string; tree: TreeEntry[] } }) | undefined;
  assert.ok(tree, "a tree was written");
  return new Map(tree.body.tree.map((e) => [e.path, blobs.get(e.sha) ?? Buffer.alloc(0)]));
}

/** The caller's create answers `status` with GitHub's `message`; any other token would succeed. */
function refuseUser(status: number, message = "Resource not accessible", headers: Record<string, string> = {}): void {
  fm.on("POST", `${REPO}/issues`, (call) =>
    call.headers.authorization === `Bearer ${USER_TOKEN}`
      ? Response.json({ message }, { status, headers })
      : Response.json({ number: 43, html_url: ISSUE_URL.replace("42", "43") }, { status: 201 }),
  );
}

/** No issue was filed with the relay token, nothing was labelled and nothing was stored. */
function assertOnlyTheCallerTried(): void {
  assert.equal(issueCalls().length, 1);
  assert.equal(issueCall().headers.authorization, `Bearer ${USER_TOKEN}`);
  assert.equal(patchCalls().length, 0);
  assert.equal(attachmentCalls().length, 0, "no attachment is left behind for a refused report");
}

beforeEach(() => {
  setEnv();
  resetAuthCache();
  resetRateLimits();
  mock.timers.enable({ apis: ["Date"], now: NOW });
  fm = mockFetch();
  mockUser(fm, "MehdiMamas");
  fm.on("GET", MILESTONES, [
    { number: 1, title: "Sprint 1", state: "open", due_on: "2026-10-06T07:00:00Z" },
    { number: 2, title: "Sprint 2", state: "open", due_on: "2026-10-20T07:00:00Z" },
    { number: 3, title: "Sprint 3", state: "open", due_on: "2026-10-27T07:00:00Z" },
  ]);
  fm.on("POST", `${REPO}/issues`, () =>
    Response.json({ number: 42, html_url: ISSUE_URL, created_at: "2026-10-13T15:00:01Z" }, { status: 201 }),
  );
  fm.on("PATCH", `${REPO}/issues/42`, (call) => Response.json({ number: 42, ...(call.body as object) }));

  blobs = new Map();
  fm.on("GET", ATT, { default_branch: "main" });
  fm.on("POST", `${ATT}/git/blobs`, (call) => {
    const sha = `blob${blobs.size + 1}`;
    blobs.set(sha, Buffer.from((call.body as { content: string }).content, "base64"));
    return Response.json({ sha }, { status: 201 });
  });
  fm.on("GET", `${ATT}/git/ref/heads/main`, { ref: "refs/heads/main", object: { sha: "head1" } });
  fm.on("GET", `${ATT}/git/commits/head1`, { sha: "head1", tree: { sha: "tree0" } });
  fm.on("POST", `${ATT}/git/trees`, Response.json({ sha: "tree1" }, { status: 201 }));
  fm.on("POST", `${ATT}/git/commits`, Response.json({ sha: "commit1" }, { status: 201 }));
  fm.on("PATCH", `${ATT}/git/refs/heads/main`, { ref: "refs/heads/main", object: { sha: "commit1" } });

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
  it("opens the issue as the caller, stores the logs privately, then labels it with the relay token", async () => {
    const res = await feedback(post(valid()));
    assert.equal(res.status, 201);
    assert.deepEqual(await res.json(), { number: 42, url: ISSUE_URL });

    assert.equal(issueCalls().length, 1);
    const call = issueCall();
    assert.equal(call.headers.authorization, `Bearer ${USER_TOKEN}`);
    assert.equal(call.body.title, "Sync stops after sleep");
    // No labels at creation: the feedback label starts triage, which needs the attachments.
    assert.equal("labels" in call.body, false);
    assert.equal("milestone" in call.body, false);

    const body = call.body.body;
    assert.match(body, /^After the laptop wakes up/);
    // GitHub shows the author, so there is no Reporter row.
    assert.ok(!body.includes("Reporter"));
    assert.match(body, /\| Kind \| bug \|/);
    assert.match(body, /\| App version \| `0\.3\.0` \|/);
    assert.match(body, /\| OS \| `Windows 11 Pro 10\.0\.26200` \|/);
    assert.match(body, /_The logs went privately to the maintainer\._$/);
    // The logs never reach the public issue.
    assert.ok(!body.includes("poll failed") && !body.includes("<details>"));

    const files = committed();
    assert.deepEqual([...files.keys()], ["issues/42/logs.txt", "issues/42/report.json"]);
    assert.equal(files.get("issues/42/logs.txt")?.toString("utf8"), valid().logs);
    for (const c of attachmentCalls()) assert.equal(c.headers.authorization, `Bearer ${RELAY_TOKEN}`);
    const commit = fm.calls.find((c) => c.method === "POST" && c.url.endsWith(`${ATT}/git/commits`));
    assert.deepEqual(commit?.body, { message: "feedback: MehdiMamas/claude-sync#42", tree: "tree1", parents: ["head1"] });
    const tree = fm.calls.find((c) => c.method === "POST" && c.url.endsWith(`${ATT}/git/trees`));
    assert.equal((tree?.body as { base_tree: string }).base_tree, "tree0");
    const refUpdate = fm.calls.find((c) => c.method === "PATCH" && c.url.endsWith(`${ATT}/git/refs/heads/main`));
    assert.deepEqual(refUpdate?.body, { sha: "commit1" });

    const patches = patchCalls();
    assert.equal(patches.length, 1);
    assert.equal(patches[0]?.headers.authorization, `Bearer ${RELAY_TOKEN}`);
    assert.deepEqual(patches[0]?.body, { labels: ["feedback", "type:bug"], milestone: 2 });
    // Created, then stored, then labelled.
    const at = (c: Call | undefined): number => fm.calls.indexOf(c as Call);
    assert.ok(at(call) < at(refUpdate));
    assert.ok(at(refUpdate) < at(patches[0]));
  });

  it("keeps who filed it in a private report.json", async () => {
    mockUser(fm, "friend-one");
    assert.equal((await feedback(post(valid()))).status, 201);
    const report = JSON.parse(committed().get("issues/42/report.json")?.toString("utf8") ?? "{}");
    assert.deepEqual(report, {
      repo: "MehdiMamas/claude-sync",
      number: 42,
      url: ISSUE_URL,
      login: "friend-one",
      kind: "bug",
      appVersion: "0.3.0",
      os: "Windows 11 Pro 10.0.26200",
      createdAt: "2026-10-13T15:00:01Z",
    });
  });

  it("labels ideas type:idea", async () => {
    assert.equal((await feedback(post(valid({ kind: "idea" })))).status, 201);
    assert.deepEqual(patchCalls()[0]?.body.labels, ["feedback", "type:idea"]);
  });

  it("stores nothing and says so when there is no screenshot and no logs", async () => {
    assert.equal((await feedback(post(valid({ logs: "" })))).status, 201);
    assert.equal(attachmentCalls().length, 0);
    assert.match(issueCall().body.body, /_No screenshot or logs attached\._$/);
    assert.deepEqual(patchCalls()[0]?.body, { labels: ["feedback", "type:bug"], milestone: 2 });
  });

  it("still answers 201 when setting the labels fails, and logs it", async () => {
    fm.on("PATCH", `${REPO}/issues/42`, 403);
    const res = await feedback(post(valid()));
    assert.equal(res.status, 201);
    assert.deepEqual(await res.json(), { number: 42, url: ISSUE_URL });
    assert.ok(logs.some((l) => l.includes("feedback labels") && l.includes("403")), logs.join("\n"));
  });

  it("never files the issue with the relay token: a 403 from GitHub answers 403 with the reason", async () => {
    refuseUser(403, "Resource not accessible by integration");
    const res = await feedback(post(valid({ screenshot: PNG.toString("base64") })));
    assert.equal(res.status, 403);
    assert.deepEqual(await res.json(), {
      error:
        "GitHub did not let your account open an issue on MehdiMamas/claude-sync: your account is blocked from the repository, or your sign-in lacks the repo scope (GitHub said: Resource not accessible by integration).",
    });
    assertOnlyTheCallerTried();
    assert.ok(logs.some((l) => l.includes("refused the issue from MehdiMamas with 403")), logs.join("\n"));
  });

  it("answers 403 for 401, 404 and 410, and 422 for 422, each with its own reason", async () => {
    const cases: Array<[number, number, string]> = [
      [401, 403, "GitHub no longer accepts your sign-in; sign out and in again"],
      [404, 403, "the repository is not visible to your account"],
      [410, 403, "issues are turned off on the repository"],
      [422, 422, "GitHub rejected the issue"],
    ];
    for (const [github, status, reason] of cases) {
      resetRateLimits();
      fm.calls.length = 0;
      refuseUser(github, "Nope");
      const res = await feedback(post(valid()));
      assert.equal(res.status, status, `GitHub ${github}`);
      const { error } = (await res.json()) as { error: string };
      assert.equal(error, `GitHub did not let your account open an issue on MehdiMamas/claude-sync: ${reason} (GitHub said: Nope).`);
      assertOnlyTheCallerTried();
    }
  });

  it("leaves out GitHub's message when the error body has none, and redacts and shortens it", async () => {
    fm.on("POST", `${REPO}/issues`, 404);
    let res = await feedback(post(valid()));
    assert.equal(res.status, 403);
    assert.match(((await res.json()) as { error: string }).error, /not visible to your account\.$/);

    resetRateLimits();
    refuseUser(403, `bad token ghp_${"a".repeat(36)}\n${"x".repeat(500)}`);
    res = await feedback(post(valid()));
    const { error } = (await res.json()) as { error: string };
    assert.ok(error.includes("bad token [REDACTED:ghp] xxx"), error);
    assert.ok(!error.includes("a".repeat(36)) && !error.includes("\n"));
    assert.ok(error.length < 450, `${error.length} chars`);
  });

  it("answers 429 with Retry-After when GitHub rate-limits the caller, so the app retries", async () => {
    refuseUser(429, "slow down", { "retry-after": "120" });
    let res = await feedback(post(valid()));
    assert.equal(res.status, 429);
    assert.equal(res.headers.get("retry-after"), "120");
    assertOnlyTheCallerTried();

    // A secondary rate limit comes back as 403 with Retry-After or no requests left.
    for (const headers of [{ "retry-after": "30" }, { "x-ratelimit-remaining": "0" }] as Array<Record<string, string>>) {
      resetRateLimits();
      fm.calls.length = 0;
      refuseUser(403, "You have exceeded a secondary rate limit", headers);
      res = await feedback(post(valid()));
      assert.equal(res.status, 429, JSON.stringify(headers));
      assert.equal(res.headers.get("retry-after"), headers["retry-after"] ?? "60");
      assertOnlyTheCallerTried();
    }
  });

  it("answers 502 on a GitHub 5xx, so the app keeps the report and retries", async () => {
    refuseUser(503);
    const res = await feedback(post(valid()));
    assert.equal(res.status, 502);
    assertOnlyTheCallerTried();
  });

  it("answers 500 when GitHub cannot be reached, so the app retries", async () => {
    fm.on("POST", `${REPO}/issues`, () => {
      throw new TypeError("fetch failed");
    });
    const res = await feedback(post(valid()));
    assert.equal(res.status, 500);
    assertOnlyTheCallerTried();
  });

  it("stores a PNG screenshot privately and keeps it out of the issue", async () => {
    const res = await feedback(post(valid({ screenshot: PNG.toString("base64") })));
    assert.equal(res.status, 201);
    const files = committed();
    assert.deepEqual([...files.keys()], ["issues/42/screenshot.png", "issues/42/logs.txt", "issues/42/report.json"]);
    assert.deepEqual(files.get("issues/42/screenshot.png"), PNG);
    const body = issueCall().body.body;
    assert.ok(!body.includes("![") && !body.includes("http"), body);
    assert.match(body, /_The screenshot and logs went privately to the maintainer\._$/);
  });

  it("accepts a JPEG given as a data URL and stores it as .jpg", async () => {
    const res = await feedback(post(valid({ logs: "", screenshot: `data:image/jpeg;base64,${JPEG.toString("base64")}` })));
    assert.equal(res.status, 201);
    assert.deepEqual([...committed().keys()], ["issues/42/screenshot.jpg", "issues/42/report.json"]);
    assert.match(issueCall().body.body, /_The screenshot went privately to the maintainer\._$/);
  });

  it("commits again on top of the new head when another report moved the branch", async () => {
    let heads = 0;
    fm.on("GET", `${ATT}/git/ref/heads/main`, () => Response.json({ object: { sha: heads++ === 0 ? "head1" : "head2" } }));
    fm.on("GET", `${ATT}/git/commits/head2`, { sha: "head2", tree: { sha: "tree9" } });
    let updates = 0;
    fm.on("PATCH", `${ATT}/git/refs/heads/main`, () =>
      updates++ === 0 ? Response.json({ message: "Update is not a fast forward" }, { status: 422 }) : Response.json({}),
    );
    assert.equal((await feedback(post(valid()))).status, 201);
    assert.equal(updates, 2);
    // Blobs are uploaded once; the tree and commit are rebuilt on the new head.
    assert.equal(fm.calls.filter((c) => c.url.endsWith(`${ATT}/git/blobs`)).length, 2);
    const commits = fm.calls.filter((c) => c.method === "POST" && c.url.endsWith(`${ATT}/git/commits`));
    assert.deepEqual((commits[1]?.body as { parents: string[] }).parents, ["head2"]);
    assert.match(issueCall().body.body, /went privately to the maintainer/);
    assert.equal("body" in (patchCalls()[0]?.body ?? {}), false);
  });

  it("still files and labels the issue when the attachments cannot be stored, and says so", async () => {
    fm.on("PATCH", `${ATT}/git/refs/heads/main`, 422);
    const res = await feedback(post(valid({ screenshot: PNG.toString("base64") })));
    assert.equal(res.status, 201);
    assert.equal(fm.calls.filter((c) => c.method === "PATCH" && c.url.includes(ATT)).length, 3);
    const patch = patchCalls()[0]?.body;
    assert.deepEqual(patch?.labels, ["feedback", "type:bug"]);
    assert.match(patch?.body ?? "", /_The screenshot and logs could not be stored\._$/);
    assert.ok(logs.some((l) => l.includes("feedback attachments") && l.includes("422")), logs.join("\n"));
  });

  it("says the attachments could not be stored when the relay token cannot see the repo", async () => {
    fm.on("GET", ATT, 404);
    assert.equal((await feedback(post(valid()))).status, 201);
    assert.match(patchCalls()[0]?.body.body ?? "", /_The logs could not be stored\._$/);
  });

  it("puts the issue in the latest sprint when every sprint is past", async () => {
    fm.on("GET", MILESTONES, [
      { number: 1, title: "Sprint 1", state: "open", due_on: "2026-09-01T07:00:00Z" },
      { number: 2, title: "Sprint 2", state: "open", due_on: "2026-09-15T07:00:00Z" },
      { number: 7, title: "Someday", state: "open", due_on: "2027-01-01T07:00:00Z" },
    ]);
    assert.equal((await feedback(post(valid()))).status, 201);
    assert.equal(patchCalls()[0]?.body.milestone, 2);
  });

  it("sets no milestone when there is no sprint", async () => {
    fm.on("GET", MILESTONES, []);
    assert.equal((await feedback(post(valid()))).status, 201);
    assert.deepEqual(patchCalls()[0]?.body, { labels: ["feedback", "type:bug"] });
  });

  it("sets no milestone when listing milestones fails", async () => {
    fm.on("GET", MILESTONES, 500);
    assert.equal((await feedback(post(valid()))).status, 201);
    assert.equal("milestone" in (patchCalls()[0]?.body ?? {}), false);
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
    const stored = committed().get("issues/42/logs.txt")?.toString("utf8") ?? "";
    assert.ok(stored.includes("Authorization: Bearer [REDACTED:bearer]"));
    assert.ok(stored.includes('{"access_token":"[REDACTED:token-field]"}'));
    assert.ok(stored.includes("[REDACTED:ghp]"));
    for (const secret of [pat, "t".repeat(40), "secret-value", "a".repeat(36)]) {
      assert.ok(!body.includes(secret) && !stored.includes(secret), "secret removed");
    }
  });

  it("keeps the last 200 KB of the logs", async () => {
    const line = "2026-10-13T10:00:00Z INFO poll ok ".padEnd(99, ".") + "\n"; // 100 bytes
    const big = "FIRST-LINE\n" + line.repeat(3000) + "LAST-LINE\n"; // ~300 KB
    const res = await feedback(post(valid({ logs: big })));
    assert.equal(res.status, 201);

    const stored = committed().get("issues/42/logs.txt")?.toString("utf8") ?? "";
    assert.ok(Buffer.byteLength(stored) <= 200 * 1024 + 64);
    assert.ok(stored.endsWith("LAST-LINE\n"));
    assert.ok(!stored.includes("FIRST-LINE"));
    assert.match(stored, /^\[… \d+ earlier bytes cut\]\n/);
    assert.ok(!issueCall().body.body.includes("LAST-LINE"));
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
    assert.equal(issueCalls().length, 0);
    assert.equal(attachmentCalls().length, 0);
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

  it("files into FEEDBACK_REPO and stores into FEEDBACK_ATTACHMENTS_REPO when set", async () => {
    setEnv({ FEEDBACK_REPO: "MehdiMamas/other", FEEDBACK_ATTACHMENTS_REPO: "MehdiMamas/other-private" });
    const other = "/repos/MehdiMamas/other";
    const priv = "/repos/MehdiMamas/other-private";
    fm.on("GET", `${other}/milestones?state=open&per_page=100`, []);
    fm.on("POST", `${other}/issues`, () =>
      Response.json({ number: 1, html_url: "https://github.com/MehdiMamas/other/issues/1" }, { status: 201 }),
    );
    fm.on("PATCH", `${other}/issues/1`, {});
    fm.on("GET", priv, { default_branch: "trunk" });
    fm.on("POST", `${priv}/git/blobs`, Response.json({ sha: "b" }, { status: 201 }));
    fm.on("GET", `${priv}/git/ref/heads/trunk`, { object: { sha: "h" } });
    fm.on("GET", `${priv}/git/commits/h`, { tree: { sha: "t" } });
    fm.on("POST", `${priv}/git/trees`, Response.json({ sha: "t2" }, { status: 201 }));
    fm.on("POST", `${priv}/git/commits`, Response.json({ sha: "c" }, { status: 201 }));
    fm.on("PATCH", `${priv}/git/refs/heads/trunk`, {});
    const res = await feedback(post(valid()));
    assert.deepEqual(await res.json(), { number: 1, url: "https://github.com/MehdiMamas/other/issues/1" });
    const tree = fm.calls.find((c) => c.url.endsWith(`${priv}/git/trees`));
    assert.deepEqual(
      (tree?.body as { tree: TreeEntry[] }).tree.map((e) => e.path),
      ["issues/1/logs.txt", "issues/1/report.json"],
    );
    const patch = fm.calls.find((c) => c.method === "PATCH" && c.url.endsWith(`${other}/issues/1`));
    assert.equal("body" in (patch?.body as object), false);
  });
});
