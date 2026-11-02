import { ARCHES, type Arch, findBundle, type Target, TARGETS } from "../lib/assets.ts";
import { requireCaller } from "../lib/auth.ts";
import { env } from "../lib/env.ts";
import { discard, github, GitHubError, type Release, type ReleaseAsset, repoPath } from "../lib/github.ts";
import { handler, HttpError, json, logWarn, noContent } from "../lib/http.ts";
import { compareSemver, formatSemver, parseSemver } from "../lib/semver.ts";

const MAX_SIG_BYTES = 16 * 1024;

function oneOf<T extends string>(value: string | null, allowed: readonly T[], name: string): T {
  if (value !== null && (allowed as readonly string[]).includes(value)) return value as T;
  throw new HttpError(400, `${name} must be one of: ${allowed.join(", ")}`);
}

/** How long an answer from GitHub is reused without asking again. */
const CACHE_MS = 60_000;
/** Lets Vercel's CDN reuse an answer; Vary keeps one entry per caller token so requireCaller still decides first. */
const CDN_CACHE = { "cache-control": "public, s-maxage=60, stale-while-revalidate=300", vary: "authorization" };

interface CachedRelease {
  etag: string | null;
  release: Release;
  at: number;
}
let releaseCache: CachedRelease | null = null;
const sigCache = new Map<number, { text: string; at: number }>();

export function resetUpdateCache(): void {
  releaseCache = null;
  sigCache.clear();
}

/** GitHub refused the relay token for now (rate limit): a stale answer beats failing. */
function limited(status: number): boolean {
  return status === 403 || status === 429;
}

async function latestRelease(repo: string, token: string): Promise<Release | null> {
  const cached = releaseCache;
  if (cached && Date.now() - cached.at < CACHE_MS) return cached.release;
  const res = await github(`${repoPath(repo)}/releases/latest`, { token, etag: cached?.etag ?? undefined });
  if (res.status === 304 && cached) {
    discard(res);
    cached.at = Date.now();
    return cached.release;
  }
  if (res.status === 404) {
    discard(res);
    releaseCache = null;
    return null;
  }
  if (!res.ok) {
    discard(res);
    if (cached && limited(res.status)) {
      logWarn(`GitHub answered ${res.status} for the latest release; serving the cached one`);
      return cached.release;
    }
    throw new GitHubError("latest release", res.status);
  }
  const release = (await res.json()) as Release;
  releaseCache = { etag: res.headers.get("etag"), release, at: Date.now() };
  return release;
}

/** The .sig asset's text. fetch follows GitHub's redirect and drops the auth header on the cross-origin hop. */
async function signature(repo: string, token: string, asset: ReleaseAsset): Promise<string> {
  const cached = sigCache.get(asset.id);
  if (cached && Date.now() - cached.at < CACHE_MS) return cached.text;
  if (asset.size > MAX_SIG_BYTES) throw new Error(`signature asset ${asset.name} is too large`);
  const res = await github(`${repoPath(repo)}/releases/assets/${asset.id}`, {
    token,
    accept: "application/octet-stream",
  });
  if (!res.ok) {
    discard(res);
    if (cached && limited(res.status)) {
      logWarn(`GitHub answered ${res.status} for signature asset ${asset.id}; serving the cached one`);
      return cached.text;
    }
    throw new GitHubError("signature asset", res.status);
  }
  const text = (await res.text()).trim();
  sigCache.set(asset.id, { text, at: Date.now() });
  return text;
}

/**
 * GET /api/update?target=<windows|darwin|linux>&arch=<x86_64|aarch64>&current_version=<semver>
 * 200 with the Tauri updater JSON when the latest release is newer, else 204.
 */
export const GET = handler("update", async (req) => {
  await requireCaller(req);
  const params = new URL(req.url).searchParams;
  const target = oneOf<Target>(params.get("target"), TARGETS, "target");
  const arch = oneOf<Arch>(params.get("arch"), ARCHES, "arch");
  const current = parseSemver(params.get("current_version") ?? "");
  if (!current) throw new HttpError(400, "current_version must be a semver version");

  const repo = env.releasesRepo();
  const token = env.relayToken();
  const release = await latestRelease(repo, token);
  if (!release || release.draft) return noContent(CDN_CACHE);

  const latest = parseSemver(release.tag_name);
  if (!latest) {
    logWarn(`latest release tag ${release.tag_name} is not semver`);
    return noContent(CDN_CACHE);
  }
  if (compareSemver(latest, current) <= 0) return noContent(CDN_CACHE);

  const version = formatSemver(latest);
  const found = findBundle(release.assets, version, target, arch);
  if (!found) {
    // Normal while a release is being assembled (the macOS build uploads later).
    logWarn(`release ${release.tag_name} has no ${target}/${arch} update bundle with a .sig`);
    return noContent(CDN_CACHE);
  }

  const origin = env.publicUrl() ?? new URL(req.url).origin;
  return json(
    200,
    {
      version,
      notes: release.body ?? "",
      pub_date: release.published_at ?? new Date().toISOString(),
      url: `${origin}/api/download/${found.bundle.id}`,
      signature: await signature(repo, token, found.sig),
    },
    CDN_CACHE,
  );
});
