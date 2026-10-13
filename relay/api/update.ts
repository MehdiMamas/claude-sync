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

async function latestRelease(repo: string, token: string): Promise<Release | null> {
  const res = await github(`${repoPath(repo)}/releases/latest`, { token });
  if (res.status === 404) {
    discard(res);
    return null;
  }
  if (!res.ok) {
    discard(res);
    throw new GitHubError("latest release", res.status);
  }
  return (await res.json()) as Release;
}

/** The .sig asset's text. fetch follows GitHub's redirect and drops the auth header on the cross-origin hop. */
async function signature(repo: string, token: string, asset: ReleaseAsset): Promise<string> {
  if (asset.size > MAX_SIG_BYTES) throw new Error(`signature asset ${asset.name} is too large`);
  const res = await github(`${repoPath(repo)}/releases/assets/${asset.id}`, {
    token,
    accept: "application/octet-stream",
  });
  if (!res.ok) {
    discard(res);
    throw new GitHubError("signature asset", res.status);
  }
  return (await res.text()).trim();
}

/**
 * GET /api/update?target=<windows|darwin>&arch=<x86_64|aarch64>&current_version=<semver>
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
  if (!release || release.draft) return noContent();

  const latest = parseSemver(release.tag_name);
  if (!latest) {
    logWarn(`latest release tag ${release.tag_name} is not semver`);
    return noContent();
  }
  if (compareSemver(latest, current) <= 0) return noContent();

  const version = formatSemver(latest);
  const found = findBundle(release.assets, version, target, arch);
  if (!found) {
    // Normal while a release is being assembled (the macOS build uploads later).
    logWarn(`release ${release.tag_name} has no ${target}/${arch} update bundle with a .sig`);
    return noContent();
  }

  const origin = env.publicUrl() ?? new URL(req.url).origin;
  return json(200, {
    version,
    notes: release.body ?? "",
    pub_date: release.published_at ?? new Date().toISOString(),
    url: `${origin}/api/download/${found.bundle.id}`,
    signature: await signature(repo, token, found.sig),
  });
});
