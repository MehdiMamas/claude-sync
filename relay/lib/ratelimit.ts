// Sliding-window limiter, in memory per function instance (good enough for a hobby).
// Keys are per GitHub login, so one account cannot use up another's budget.

/** Every authenticated call, per login. The app checks for updates every 6 hours. */
export const API_LIMIT = 120;
export const API_WINDOW_MS = 60 * 60 * 1000;
/** Feedback posts per login. Each one becomes a public GitHub issue. */
export const FEEDBACK_LIMIT = 5;
export const FEEDBACK_WINDOW_MS = 60 * 60 * 1000;
// Past this many keys, keys whose last hit is older than an hour are dropped.
const MAX_KEYS = 10_000;
const PRUNE_AFTER_MS = 60 * 60 * 1000;

const hits = new Map<string, number[]>();

export function resetRateLimits(): void {
  hits.clear();
}

export type Take = { ok: true } | { ok: false; retryAfterSeconds: number };

function prune(now: number): void {
  for (const [k, times] of hits) {
    if ((times[times.length - 1] ?? 0) <= now - PRUNE_AFTER_MS) hits.delete(k);
  }
}

/** Records one hit for `key` unless it already has `limit` hits inside the window. */
export function take(key: string, limit = FEEDBACK_LIMIT, windowMs = FEEDBACK_WINDOW_MS): Take {
  const now = Date.now();
  const k = key.toLowerCase();
  const recent = (hits.get(k) ?? []).filter((t) => t > now - windowMs);
  if (recent.length >= limit) {
    hits.set(k, recent);
    const oldest = recent[0] ?? now;
    return { ok: false, retryAfterSeconds: Math.max(1, Math.ceil((oldest + windowMs - now) / 1000)) };
  }
  if (!hits.has(k) && hits.size >= MAX_KEYS) prune(now);
  recent.push(now);
  hits.set(k, recent);
  return { ok: true };
}
