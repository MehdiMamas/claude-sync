import { requireCaller } from "../../lib/auth.ts";
import { sessionLogin } from "../../lib/session.ts";
import { env } from "../../lib/env.ts";
import { discard, github, GitHubError, repoPath } from "../../lib/github.ts";
import { handler, HttpError } from "../../lib/http.ts";

const REDIRECTS = new Set([301, 302, 303, 307, 308]);

function assetId(req: Request): string {
  const url = new URL(req.url);
  const id = url.pathname.split("/").filter(Boolean).pop() ?? url.searchParams.get("id") ?? "";
  if (!/^\d{1,20}$/.test(id)) throw new HttpError(400, "asset id must be a number");
  return id;
}

/**
 * GET /api/download/<asset-id>
 * 302 to GitHub's short-lived signed URL for an asset of a RELEASES_REPO release.
 * The bytes never pass through the relay.
 */
export const GET = handler("download", async (req) => {
  // The install page sends a signed session cookie instead of a bearer token.
  if (!req.headers.has("authorization")) {
    const login = sessionLogin(req);
    if (!login) throw new HttpError(401, "missing GitHub token", { "www-authenticate": "Bearer" });
    if (env.blockedUsers().has(login.toLowerCase())) throw new HttpError(403, `${login} is not allowed to use this relay`);
  } else {
    await requireCaller(req);
  }
  const id = assetId(req);

  // The asset is looked up under RELEASES_REPO, so ids from other repos 404.
  const res = await github(`${repoPath(env.releasesRepo())}/releases/assets/${id}`, {
    token: env.relayToken(),
    accept: "application/octet-stream",
    redirect: "manual",
  });
  discard(res);
  const location = res.headers.get("location");
  if (REDIRECTS.has(res.status) && location) {
    return new Response(null, { status: 302, headers: { location, "cache-control": "no-store" } });
  }
  if (res.status === 404) throw new HttpError(404, "asset not found");
  throw new GitHubError("asset download", res.status);
});
