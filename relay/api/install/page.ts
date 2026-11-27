import { env } from "../../lib/env.ts";
import { handler } from "../../lib/http.ts";

/** GET /install: the Releases page. Group READMEs already in the field link here (v2-Cutover). */
export const GET = handler("install", async () => {
  return new Response(null, {
    status: 302,
    headers: { location: `https://github.com/${env.releasesRepo()}/releases`, "cache-control": "no-store" },
  });
});
