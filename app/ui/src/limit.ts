// How a sync error reads: GitHub's rate limit as a time in the user's zone, else the error.
// No DOM: `limit.test.mjs` runs it in Node.

/** "GitHub limit reached, resumes 5:24 PM" (the time in the viewer's zone). */
export function limitText(unix: number): string {
  const time = new Date(unix * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  return `GitHub limit reached, resumes ${time}`;
}

/** What to show in place of `lastError`: the limit text while `rateLimitedUntil` is set. */
export function syncError(s: { lastError: string | null; rateLimitedUntil: number | null }): string | null {
  return s.rateLimitedUntil != null ? limitText(s.rateLimitedUntil) : s.lastError;
}
