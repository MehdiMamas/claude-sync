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
  it("redirects to the Releases page without calling GitHub", async () => {
    const res = await page(get("/install"));
    assert.equal(res.status, 302);
    assert.equal(res.headers.get("location"), "https://github.com/MehdiMamas/claude-sync/releases");
    assert.equal(fm.calls.length, 0);
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
