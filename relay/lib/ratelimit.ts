// Sliding-window limiter, in memory per function instance (good enough for a hobby).

export const FEEDBACK_LIMIT = 10;
export const FEEDBACK_WINDOW_MS = 60 * 60 * 1000;

const hits = new Map<string, number[]>();

export function resetRateLimits(): void {
  hits.clear();
}

export type Take = { ok: true } | { ok: false; retryAfterSeconds: number };

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
  recent.push(now);
  hits.set(k, recent);
  return { ok: true };
}
