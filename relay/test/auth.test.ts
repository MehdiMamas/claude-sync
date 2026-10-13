import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, it, mock } from "node:test";
import { GET as health } from "../api/health.ts";
import { AUTH_CACHE_MS, requireCaller, resetAuthCache } from "../lib/auth.ts";
import { handler, json } from "../lib/http.ts";
import { type FetchMock, mockFetch, mockUser, RELAY_TOKEN, req, setEnv, USER_TOKEN } from "./helpers.ts";

const whoami = handler("test", async (r) => json(200, await requireCaller(r)));
const COLLAB = "/repos/MehdiMamas/claude-sync-data/collaborators/";

let fm: FetchMock;
let logs: string[];

beforeEach(() => {
  setEnv();
  resetAuthCache();
  fm = mockFetch();
  logs = [];
  for (const level of ["log", "warn", "error", "info", "debug"] as const) {
    mock.method(console, level, (...args: unknown[]) => logs.push(args.map(String).join(" ")));
  }
});

afterEach(() => {
  fm.restore();
  mock.restoreAll();
  mock.timers.reset();
  // No user or relay token ever reaches a log line.
  for (const line of logs) {
    assert.ok(!line.includes(USER_TOKEN) && !line.includes(RELAY_TOKEN), `token leaked in log: ${line}`);
  }
});

describe("health", () => {
  it("answers 200 without auth or GitHub", async () => {
    const res = await health();
    assert.equal(res.status, 200);
    assert.deepEqual(await res.json(), { ok: true });
    assert.equal(fm.calls.length, 0);
  });
});

describe("auth", () => {
  it("401 without a token", async () => {
    const res = await whoami(req("/x", { token: null }));
    assert.equal(res.status, 401);
    assert.equal(res.headers.get("www-authenticate"), "Bearer");
    assert.equal(fm.calls.length, 0);
  });

  it("401 for a non-Bearer header", async () => {
    const res = await whoami(req("/x", { token: null, headers: { authorization: `token ${USER_TOKEN}` } }));
    assert.equal(res.status, 401);
  });

  it("401 when GitHub rejects the token", async () => {
    mockUser(fm, "MehdiMamas");
    const res = await whoami(req("/x", { token: "gho_bad" }));
    assert.equal(res.status, 401);
    assert.deepEqual(await res.json(), { error: "invalid GitHub token" });
  });

  it("allows a login from ALLOWED_GITHUB_USERS, case-insensitively, without a collaborator call", async () => {
    mockUser(fm, "friend-one");
    const res = await whoami(req("/x"));
    assert.equal(res.status, 200);
    assert.deepEqual(await res.json(), { login: "friend-one" });
    assert.deepEqual(
      fm.calls.map((c) => c.url),
      ["https://api.github.com/user"],
    );
    assert.equal(fm.calls[0]?.headers.authorization, `Bearer ${USER_TOKEN}`);
  });

  it("allows a collaborator of a GROUP_REPOS repo, asking GitHub with the relay token", async () => {
    mockUser(fm, "pal");
    fm.on("GET", COLLAB + "pal", 204);
    const res = await whoami(req("/x"));
    assert.equal(res.status, 200);
    const collab = fm.calls.find((c) => c.url.includes("/collaborators/"));
    assert.equal(collab?.headers.authorization, `Bearer ${RELAY_TOKEN}`);
  });

  it("checks every group repo", async () => {
    setEnv({ GROUP_REPOS: "MehdiMamas/claude-sync-data, other/group" });
    mockUser(fm, "pal");
    fm.on("GET", COLLAB + "pal", 404);
    fm.on("GET", "/repos/other/group/collaborators/pal", 204);
    assert.equal((await whoami(req("/x"))).status, 200);
  });

  it("403 for a login that is neither listed nor a collaborator", async () => {
    mockUser(fm, "stranger");
    fm.on("GET", COLLAB + "stranger", 404);
    const res = await whoami(req("/x"));
    assert.equal(res.status, 403);
    assert.deepEqual(await res.json(), { error: "stranger is not allowed to use this relay" });
  });

  it("502 when the collaborator check fails, and does not cache that", async () => {
    mockUser(fm, "pal");
    fm.on("GET", COLLAB + "pal", 500);
    assert.equal((await whoami(req("/x"))).status, 502);
    fm.on("GET", COLLAB + "pal", 204);
    assert.equal((await whoami(req("/x"))).status, 200);
  });

  it("caches the decision for 10 minutes", async () => {
    mock.timers.enable({ apis: ["Date"], now: Date.parse("2026-10-13T12:00:00Z") });
    mockUser(fm, "stranger");
    fm.on("GET", COLLAB + "stranger", 404);
    assert.equal((await whoami(req("/x"))).status, 403);
    assert.equal(fm.calls.length, 2);

    mock.timers.tick(AUTH_CACHE_MS - 1000);
    assert.equal((await whoami(req("/x"))).status, 403);
    assert.equal(fm.calls.length, 2, "served from cache");

    // Became a collaborator; seen once the entry expires.
    fm.on("GET", COLLAB + "stranger", 204);
    mock.timers.tick(2000);
    assert.equal((await whoami(req("/x"))).status, 200);
    assert.equal(fm.calls.length, 4);
  });

  it("500 when the relay is not configured", async () => {
    setEnv({ ALLOWED_GITHUB_USERS: "", RELAY_GITHUB_TOKEN: undefined });
    mockUser(fm, "pal");
    const res = await whoami(req("/x"));
    assert.equal(res.status, 500);
    assert.deepEqual(await res.json(), { error: "relay is not configured" });
  });
});
