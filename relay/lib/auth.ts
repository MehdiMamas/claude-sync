import { createHash } from "node:crypto";
import { env } from "./env.ts";
import { discard, github, repoPath } from "./github.ts";
import { HttpError, logWarn } from "./http.ts";

export const AUTH_CACHE_MS = 10 * 60 * 1000;
const MAX_CACHE_ENTRIES = 1000;
const LOGIN_RE = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$/;

export interface Caller {
  login: string;
}

interface CacheEntry {
  login: string;
  allowed: boolean;
  expires: number;
}

// Keyed by a SHA-256 of the token so the token itself is never kept.
const cache = new Map<string, CacheEntry>();

export function resetAuthCache(): void {
  cache.clear();
}

function tokenKey(token: string): string {
  return createHash("sha256").update(token).digest("hex");
}

function bearer(req: Request): string {
  const header = req.headers.get("authorization") ?? "";
  const match = /^Bearer\s+(\S+)\s*$/i.exec(header);
  if (!match?.[1]) {
    throw new HttpError(401, "missing GitHub token", { "www-authenticate": "Bearer" });
  }
  return match[1];
}

async function loginFor(token: string): Promise<string> {
  const res = await github("/user", { token });
  if (res.status === 401) {
    discard(res);
    throw new HttpError(401, "invalid GitHub token", { "www-authenticate": "Bearer" });
  }
  if (!res.ok) {
    discard(res);
    logWarn(`GET /user returned ${res.status}`);
    throw new HttpError(502, "GitHub is unavailable");
  }
  const user = (await res.json()) as { login?: unknown };
  if (typeof user.login !== "string" || !LOGIN_RE.test(user.login)) {
    throw new HttpError(502, "unexpected GitHub response");
  }
  return user.login;
}

/** True when `login` collaborates on any GROUP_REPOS repo. Throws if GitHub fails and no repo says yes. */
async function isGroupCollaborator(login: string): Promise<boolean> {
  const repos = env.groupRepos();
  if (repos.length === 0) return false;
  const token = env.relayToken();
  const results = await Promise.all(
    repos.map(async (repo) => {
      const res = await github(`${repoPath(repo)}/collaborators/${encodeURIComponent(login)}`, { token });
      discard(res);
      if (res.status === 204) return "yes" as const;
      if (res.status === 404) return "no" as const;
      logWarn(`collaborator check on ${repo} returned ${res.status}`);
      return "error" as const;
    }),
  );
  if (results.includes("yes")) return true;
  if (results.includes("error")) throw new HttpError(502, "GitHub is unavailable");
  return false;
}

function remember(key: string, entry: CacheEntry): void {
  if (cache.size >= MAX_CACHE_ENTRIES) {
    const now = Date.now();
    for (const [k, v] of cache) if (v.expires <= now) cache.delete(k);
    if (cache.size >= MAX_CACHE_ENTRIES) cache.clear();
  }
  cache.set(key, entry);
}

/**
 * Resolves the caller from `Authorization: Bearer <GitHub token>`.
 * 401 when the token is missing or GitHub rejects it, 403 when the login is not allowed.
 */
export async function requireCaller(req: Request): Promise<Caller> {
  const token = bearer(req);
  const key = tokenKey(token);
  const now = Date.now();
  let entry = cache.get(key);
  if (!entry || entry.expires <= now) {
    const login = await loginFor(token);
    const allowed = env.allowedUsers().has(login.toLowerCase()) || (await isGroupCollaborator(login));
    entry = { login, allowed, expires: now + AUTH_CACHE_MS };
    remember(key, entry);
  }
  if (!entry.allowed) throw new HttpError(403, `${entry.login} is not allowed to use this relay`);
  return { login: entry.login };
}
