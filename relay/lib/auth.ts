import { createHash } from "node:crypto";
import { env } from "./env.ts";
import { discard, github } from "./github.ts";
import { HttpError, logWarn } from "./http.ts";
import { API_LIMIT, API_WINDOW_MS, take } from "./ratelimit.ts";

export const AUTH_CACHE_MS = 10 * 60 * 1000;
const MAX_CACHE_ENTRIES = 1000;
const LOGIN_RE = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$/;

export interface Caller {
  login: string;
}

/** A caller plus the token GitHub verified, for acting as that user. Never put it in a response. */
export interface CallerWithToken extends Caller {
  token: string;
}

interface CacheEntry {
  login: string;
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

function remember(key: string, entry: CacheEntry): void {
  if (cache.size >= MAX_CACHE_ENTRIES) {
    const now = Date.now();
    for (const [k, v] of cache) if (v.expires <= now) cache.delete(k);
    if (cache.size >= MAX_CACHE_ENTRIES) cache.clear();
  }
  cache.set(key, entry);
}

/**
 * Resolves the caller from `Authorization: Bearer <GitHub token>`. Any GitHub user may
 * use the relay. 401 when the token is missing or GitHub rejects it, 403 when the login
 * is in BLOCKED_GITHUB_USERS, 429 when the login made API_LIMIT calls inside the window.
 */
export async function requireCaller(req: Request): Promise<Caller> {
  const { login } = await requireCallerWithToken(req);
  return { login };
}

/** Like requireCaller, and also returns the verified token so the relay can act as the caller. */
export async function requireCallerWithToken(req: Request): Promise<CallerWithToken> {
  const token = bearer(req);
  const key = tokenKey(token);
  const now = Date.now();
  let entry = cache.get(key);
  if (!entry || entry.expires <= now) {
    entry = { login: await loginFor(token), expires: now + AUTH_CACHE_MS };
    remember(key, entry);
  }
  const { login } = entry;
  if (env.blockedUsers().has(login.toLowerCase())) {
    throw new HttpError(403, `${login} is not allowed to use this relay`);
  }
  const slot = take(`api:${login}`, API_LIMIT, API_WINDOW_MS);
  if (!slot.ok) {
    throw new HttpError(429, "too many requests, try again later", {
      "retry-after": String(slot.retryAfterSeconds),
    });
  }
  return { login, token };
}
