import { env } from "../../lib/env.ts";
import { discard, github } from "../../lib/github.ts";
import { handler, logWarn } from "../../lib/http.ts";
import { readCookie, SESSION_COOKIE, SESSION_MS, setCookie, signSession, STATE_COOKIE } from "../../lib/session.ts";

const LOGIN_RE = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$/;

function back(error?: string, cookies: string[] = []): Response {
  const headers = new Headers({ location: error ? `/install?error=${error}` : "/install", "cache-control": "no-store" });
  headers.append("set-cookie", setCookie(STATE_COOKIE, "", 0, "/api/install"));
  for (const c of cookies) headers.append("set-cookie", c);
  return new Response(null, { status: 302, headers });
}

/** Trades the code for a token, reads the login, and drops the token. Null when GitHub refuses. */
async function loginFor(code: string, clientId: string, clientSecret: string): Promise<string | null> {
  const res = await fetch("https://github.com/login/oauth/access_token", {
    method: "POST",
    headers: { accept: "application/json", "content-type": "application/json", "user-agent": "claude-sync-relay" },
    body: JSON.stringify({ client_id: clientId, client_secret: clientSecret, code }),
  });
  if (!res.ok) {
    discard(res);
    logWarn(`install: token exchange returned ${res.status}`);
    return null;
  }
  const token = ((await res.json()) as { access_token?: unknown }).access_token;
  if (typeof token !== "string" || !token) return null;
  const user = await github("/user", { token });
  if (!user.ok) {
    discard(user);
    logWarn(`install: GET /user returned ${user.status}`);
    return null;
  }
  const login = ((await user.json()) as { login?: unknown }).login;
  return typeof login === "string" && LOGIN_RE.test(login) ? login : null;
}

/** GET /api/install/callback?code&state: checks the state cookie, signs the user in, redirects to /install. */
export const GET = handler("install-callback", async (req) => {
  const clientId = env.oauthClientId();
  const clientSecret = env.oauthClientSecret();
  const secret = env.installSessionSecret();
  if (!clientId || !clientSecret || !secret) return back();

  const params = new URL(req.url).searchParams;
  const state = params.get("state");
  const expected = readCookie(req, STATE_COOKIE);
  if (!state || !expected || state !== expected) return back("state");
  const code = params.get("code");
  if (!code || params.get("error")) return back("denied");

  const login = await loginFor(code, clientId, clientSecret);
  if (!login) return back("github");
  return back(undefined, [setCookie(SESSION_COOKIE, signSession(login, secret), SESSION_MS / 1000)]);
});
