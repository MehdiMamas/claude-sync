import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, it, mock } from "node:test";
import { GET as health } from "../api/health.ts";
import { GET as update } from "../api/update.ts";
import { AUTH_CACHE_MS, requireCaller, resetAuthCache } from "../lib/auth.ts";
import { handler, json } from "../lib/http.ts";
import { API_LIMIT, API_WINDOW_MS, resetRateLimits } from "../lib/ratelimit.ts";
import { type FetchMock, mockFetch, mockUser, RELAY_TOKEN, req, setEnv, USER_TOKEN } from "./helpers.ts";

const whoami = handler("test", async (r) => json(200, await requireCaller(r)));

let fm: FetchMock;
let logs: string[];

beforeEach(() => {
  setEnv();
  resetAuthCache();
  resetRateLimits();
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

  it("allows any GitHub user, asking GitHub with the caller's own token", async () => {
    mockUser(fm, "stranger");
    const res = await whoami(req("/x"));
    assert.equal(res.status, 200);
    assert.deepEqual(await res.json(), { login: "stranger" });
    assert.deepEqual(
      fm.calls.map((c) => c.url),
      ["https://api.github.com/user"],
    );
    assert.equal(fm.calls[0]?.headers.authorization, `Bearer ${USER_TOKEN}`);
  });

  it("403 for a login in BLOCKED_GITHUB_USERS, case-insensitively", async () => {
    setEnv({ BLOCKED_GITHUB_USERS: "spammer, Stranger" });
    mockUser(fm, "stranger");
    const res = await whoami(req("/x"));
    assert.equal(res.status, 403);
    assert.deepEqual(await res.json(), { error: "stranger is not allowed to use this relay" });
  });

  it("502 when GitHub fails, and does not cache that", async () => {
    fm.on("GET", "/user", 500);
    assert.equal((await whoami(req("/x"))).status, 502);
    mockUser(fm, "pal");
    assert.equal((await whoami(req("/x"))).status, 200);
  });

  it("caches the login for 10 minutes", async () => {
    mock.timers.enable({ apis: ["Date"], now: Date.parse("2026-10-13T12:00:00Z") });
    mockUser(fm, "pal");
    assert.equal((await whoami(req("/x"))).status, 200);
    assert.equal(fm.calls.length, 1);

    mock.timers.tick(AUTH_CACHE_MS - 1000);
    assert.equal((await whoami(req("/x"))).status, 200);
    assert.equal(fm.calls.length, 1, "served from cache");

    mock.timers.tick(2000);
    assert.equal((await whoami(req("/x"))).status, 200);
    assert.equal(fm.calls.length, 2);
  });

  it("a block applies at once, even to a cached login", async () => {
    mockUser(fm, "pal");
    assert.equal((await whoami(req("/x"))).status, 200);
    setEnv({ BLOCKED_GITHUB_USERS: "pal" });
    assert.equal((await whoami(req("/x"))).status, 403);
    assert.equal(fm.calls.length, 1);
  });

  it(`allows ${API_LIMIT} calls per login per hour, then 429 with Retry-After`, async () => {
    mock.timers.enable({ apis: ["Date"], now: Date.parse("2026-10-13T12:00:00Z") });
    mockUser(fm, "pal");
    for (let i = 0; i < API_LIMIT; i++) assert.equal((await whoami(req("/x"))).status, 200, `call ${i + 1}`);
    const res = await whoami(req("/x"));
    assert.equal(res.status, 429);
    assert.equal(res.headers.get("retry-after"), String(API_WINDOW_MS / 1000));

    // Another login has its own budget.
    resetAuthCache();
    mockUser(fm, "friend-one");
    assert.equal((await whoami(req("/x"))).status, 200);

    // The window slides.
    resetAuthCache();
    mockUser(fm, "pal");
    mock.timers.tick(API_WINDOW_MS + 1);
    assert.equal((await whoami(req("/x"))).status, 200);
  });

  it("a rejected token does not count against anyone's budget", async () => {
    mockUser(fm, "pal");
    for (let i = 0; i < API_LIMIT + 5; i++) await whoami(req("/x", { token: "gho_bad" }));
    assert.equal((await whoami(req("/x"))).status, 200);
  });

  it("500 when the relay is not configured, without saying more", async () => {
    setEnv({ RELAY_GITHUB_TOKEN: undefined });
    mockUser(fm, "pal");
    const res = await update(req("/api/update?target=windows&arch=x86_64&current_version=0.1.0"));
    assert.equal(res.status, 500);
    assert.deepEqual(await res.json(), { error: "relay is not configured" });
  });
});
