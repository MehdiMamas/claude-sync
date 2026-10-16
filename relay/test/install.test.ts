import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, it } from "node:test";
import { GET as download } from "../api/download/[id].ts";
import { GET as callback } from "../api/install/callback.ts";
import { GET as login } from "../api/install/login.ts";
import { GET as logout } from "../api/install/logout.ts";
import { GET as page } from "../api/install/page.ts";
import { resetAuthCache } from "../lib/auth.ts";
import { SESSION_COOKIE, signSession, STATE_COOKIE } from "../lib/session.ts";
import { type FetchMock, mockFetch, setEnv } from "./helpers.ts";

const SECRET = "s".repeat(40);
const OAUTH = {
  GITHUB_OAUTH_CLIENT_ID: "Iv1.client",
  GITHUB_OAUTH_CLIENT_SECRET: "oauth-secret",
  INSTALL_SESSION_SECRET: SECRET,
};
const TOKEN_URL = "https://github.com/login/oauth/access_token";
const REPO = "/repos/MehdiMamas/claude-sync";

function get(path: string, cookie?: string): Request {
  return new Request(`https://relay.example.test${path}`, { headers: cookie ? { cookie } : {} });
}

function cookies(res: Response): string[] {
  return res.headers.getSetCookie();
}

function session(loginName: string, secret = SECRET): string {
  return `${SESSION_COOKIE}=${signSession(loginName, secret)}`;
}

const RELEASE = {
  tag_name: "v0.3.0",
  name: "v0.3.0",
  body: "",
  draft: false,
  prerelease: false,
  published_at: "2026-10-12T10:00:00Z",
  assets: [
    { id: 101, name: "claude-sync_0.3.0_windows_x86_64-setup.exe", size: 1 },
    { id: 202, name: "claude-sync_0.3.0_darwin_universal.dmg", size: 1 },
  ],
};

let fm: FetchMock;

beforeEach(() => {
  resetAuthCache();
  fm = mockFetch();
  setEnv(OAUTH);
});
afterEach(() => {
  fm.restore();
  setEnv({ GITHUB_OAUTH_CLIENT_ID: undefined, GITHUB_OAUTH_CLIENT_SECRET: undefined, INSTALL_SESSION_SECRET: undefined });
});

describe("/api/install/login", () => {
  it("redirects to GitHub with read:user and a state that matches the cookie", async () => {
    const res = await login(get("/api/install/login"));
    assert.equal(res.status, 302);
    const url = new URL(res.headers.get("location") ?? "");
    assert.equal(url.origin + url.pathname, "https://github.com/login/oauth/authorize");
    assert.equal(url.searchParams.get("client_id"), "Iv1.client");
    assert.equal(url.searchParams.get("scope"), "read:user");
    const state = url.searchParams.get("state") ?? "";
    assert.ok(state.length >= 20);
    const [cookie] = cookies(res);
    assert.ok(cookie?.startsWith(`${STATE_COOKIE}=${state};`));
    for (const flag of ["HttpOnly", "Secure", "SameSite=Lax", "Max-Age=600"]) assert.ok(cookie?.includes(flag), flag);
  });
});

describe("/api/install/callback", () => {
  it("rejects a state that does not match the cookie, without calling GitHub", async () => {
    const res = await callback(get("/api/install/callback?code=c&state=evil", `${STATE_COOKIE}=good`));
    assert.equal(res.status, 302);
    assert.equal(res.headers.get("location"), "/install?error=state");
    assert.ok(!cookies(res).some((c) => c.startsWith(`${SESSION_COOKIE}=`)));
    assert.equal(fm.calls.length, 0);
  });

  it("rejects a missing state cookie", async () => {
    const res = await callback(get("/api/install/callback?code=c&state=x"));
    assert.equal(res.headers.get("location"), "/install?error=state");
  });

  it("exchanges the code, signs a session for the login and keeps the token out of it", async () => {
    fm.on("POST", TOKEN_URL, () => Response.json({ access_token: "gho_secretoken" }));
    fm.on("GET", "/user", (call) =>
      call.headers.authorization === "Bearer gho_secretoken"
        ? Response.json({ login: "friend" })
        : Response.json({}, { status: 401 }),
    );
    const res = await callback(get("/api/install/callback?code=abc&state=st", `${STATE_COOKIE}=st`));
    assert.equal(res.status, 302);
    assert.equal(res.headers.get("location"), "/install");
    const exchange = fm.calls.find((c) => c.url === TOKEN_URL);
    assert.deepEqual(exchange?.body, { client_id: "Iv1.client", client_secret: "oauth-secret", code: "abc" });
    const sess = cookies(res).find((c) => c.startsWith(`${SESSION_COOKIE}=`)) ?? "";
    assert.ok(sess.includes("HttpOnly") && sess.includes("Secure") && sess.includes("Max-Age=86400"));
    assert.ok(!sess.includes("gho_secretoken"));
    assert.ok(cookies(res).some((c) => c.startsWith(`${STATE_COOKIE}=;`)), "state cookie cleared");
    // The cookie it set works on the page.
    fm.on("GET", `${REPO}/releases/latest`, RELEASE);
    const html = await (await page(get("/install", sess.split(";")[0]))).text();
    assert.ok(html.includes("friend"));
  });

  it("answers an error when GitHub refuses the code", async () => {
    fm.on("POST", TOKEN_URL, () => Response.json({ error: "bad_verification_code" }));
    const res = await callback(get("/api/install/callback?code=abc&state=st", `${STATE_COOKIE}=st`));
    assert.equal(res.headers.get("location"), "/install?error=github");
    assert.ok(!cookies(res).some((c) => c.startsWith(`${SESSION_COOKIE}=`)));
  });

  it("does nothing when sign-in is not configured", async () => {
    setEnv({ GITHUB_OAUTH_CLIENT_SECRET: undefined });
    const res = await callback(get("/api/install/callback?code=abc&state=st", `${STATE_COOKIE}=st`));
    assert.equal(res.headers.get("location"), "/install");
    assert.equal(fm.calls.length, 0);
  });
});

describe("/install", () => {
  it("says sign-in is not configured when an env var is missing", async () => {
    setEnv({ INSTALL_SESSION_SECRET: undefined });
    const res = await page(get("/install"));
    assert.equal(res.status, 503);
    assert.match(await res.text(), /not configured/);
  });

  it("shows the sign-in button when signed out", async () => {
    const html = await (await page(get("/install"))).text();
    assert.match(html, /Sign in with GitHub/);
    assert.match(html, /href="\/api\/install\/login"/);
    assert.ok(!html.includes("/api/download/"));
  });

  it("shows the downloads, version and macOS steps to an allowed login", async () => {
    fm.on("GET", `${REPO}/releases/latest`, RELEASE);
    const res = await page(get("/install", session("friend")));
    assert.equal(res.status, 200);
    const html = await res.text();
    assert.match(html, /href="\/api\/download\/101"/);
    assert.match(html, /href="\/api\/download\/202"/);
    assert.match(html, /0\.3\.0/);
    assert.match(html, /Open Anyway/);
    assert.match(html, /xattr -dr com\.apple\.quarantine \/Applications\/claude-sync\.app/);
    assert.match(html, /Sign out/);
  });

  it("tells a blocked login it is not on the list and shows no downloads", async () => {
    setEnv({ ...OAUTH, BLOCKED_GITHUB_USERS: "Friend" });
    const res = await page(get("/install", session("friend")));
    assert.equal(res.status, 403);
    const html = await res.text();
    assert.match(html, /Your GitHub account \(friend\) isn't on the list yet\. Ask the person who invited you\./);
    assert.ok(!html.includes("/api/download/"));
    assert.equal(fm.calls.length, 0);
  });

  it("escapes the login", async () => {
    const html = await (await page(get("/install", session("<b>x</b>")))).text();
    assert.ok(!html.includes("<b>x</b>"));
  });

  it("treats a tampered cookie as signed out", async () => {
    const good = signSession("friend", SECRET);
    const [payload, mac] = good.split(".");
    const forged = Buffer.from(JSON.stringify({ l: "MehdiMamas", e: Date.now() + 1e9 })).toString("base64url");
    for (const value of [`${forged}.${mac}`, `${payload}.${mac?.slice(0, -2)}AA`, signSession("friend", "other-secret")]) {
      const html = await (await page(get("/install", `${SESSION_COOKIE}=${value}`))).text();
      assert.match(html, /Sign in with GitHub/, value);
    }
  });

  it("treats an expired cookie as signed out", async () => {
    const old = signSession("friend", SECRET, Date.now() - 2 * 86_400_000);
    const html = await (await page(get("/install", `${SESSION_COOKIE}=${old}`))).text();
    assert.match(html, /Sign in with GitHub/);
  });
});

describe("/api/download with the install session", () => {
  const redirect = () =>
    fm.on("GET", `${REPO}/releases/assets/101`, new Response(null, { status: 302, headers: { location: "https://objects.example/x" } }));

  it("redirects for a signed-in, allowed login", async () => {
    redirect();
    const res = await download(get("/api/download/101", session("friend")));
    assert.equal(res.status, 302);
  });

  it("401 without a bearer token or session, 403 for a blocked login", async () => {
    assert.equal((await download(get("/api/download/101"))).status, 401);
    assert.equal((await download(get("/api/download/101", `${SESSION_COOKIE}=forged.sig`))).status, 401);
    setEnv({ ...OAUTH, BLOCKED_GITHUB_USERS: "friend" });
    assert.equal((await download(get("/api/download/101", session("friend")))).status, 403);
  });
});

describe("/api/install/logout", () => {
  it("clears the session cookie", async () => {
    const res = await logout(get("/api/install/logout"));
    assert.equal(res.status, 302);
    assert.ok(cookies(res)[0]?.startsWith(`${SESSION_COOKIE}=;`));
  });
});
