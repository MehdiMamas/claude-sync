import { handler } from "../../lib/http.ts";
import { SESSION_COOKIE, setCookie } from "../../lib/session.ts";

/** GET /api/install/logout: clears the install session. */
export const GET = handler("install-logout", async () => {
  const headers = new Headers({ location: "/install", "cache-control": "no-store" });
  headers.append("set-cookie", setCookie(SESSION_COOKIE, "", 0));
  return new Response(null, { status: 302, headers });
});
