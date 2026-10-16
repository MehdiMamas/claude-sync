import { env } from "../../lib/env.ts";
import { handler } from "../../lib/http.ts";
import { newState, setCookie, STATE_COOKIE, STATE_MAX_AGE_S } from "../../lib/session.ts";

/** GET /api/install/login: sends the browser to GitHub's authorize page with a random state. */
export const GET = handler("install-login", async () => {
  const clientId = env.oauthClientId();
  if (!env.installConfigured() || !clientId) {
    return new Response(null, { status: 302, headers: { location: "/install", "cache-control": "no-store" } });
  }
  const state = newState();
  const url = new URL("https://github.com/login/oauth/authorize");
  url.searchParams.set("client_id", clientId);
  url.searchParams.set("scope", "read:user");
  url.searchParams.set("state", state);
  const headers = new Headers({ location: url.href, "cache-control": "no-store" });
  headers.append("set-cookie", setCookie(STATE_COOKIE, state, STATE_MAX_AGE_S, "/api/install"));
  return new Response(null, { status: 302, headers });
});
