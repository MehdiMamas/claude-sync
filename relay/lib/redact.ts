// Same patterns as lib/secrets.sh (docs/architecture.md §9), plus `Bearer <token>`.
// Order matters: the longer, more specific patterns run first so a github_pat is
// reported under its own name. Minimum lengths match lib/secrets.sh; matches
// extend over the rest of the token so no tail is left behind.
const PATTERNS: ReadonlyArray<readonly [name: string, regex: RegExp, replace?: string]> = [
  ["github_pat", /github_pat_[A-Za-z0-9_]{50,}/g],
  ["sk-ant", /sk-ant-[A-Za-z0-9_-]{20,}/g],
  ["ghp", /ghp_[A-Za-z0-9]{36,}/g],
  ["gh-token", /gh[orsu]_[A-Za-z0-9]{36,}/g],
  ["aws-access-key", /AKIA[0-9A-Z]{16,}/g],
  ["slack-token", /xox[abpr]-[A-Za-z0-9-]{10,}/g],
  // The header, plus the key body up to its END line; with no END line (a cut log),
  // the next 20000 characters, so a lone header never leaves the key body behind.
  [
    "private-key",
    /-----BEGIN\s[A-Z\s]*PRIVATE\sKEY-----(?:[\s\S]{0,20000}?-----END\s[A-Z\s]*PRIVATE\sKEY-----|[\s\S]{0,20000})/g,
  ],
  // "access_token": "value" keeps the key and drops the value.
  ["token-field", /("(?:access|refresh)_token"\s*:\s*)"(?:[^"\\]|\\.)*"/g, '$1"[REDACTED:token-field]"'],
  ["bearer", /\b(Bearer)\s+[A-Za-z0-9._~+/=-]+/gi, "$1 [REDACTED:bearer]"],
];

export function redact(text: string): string {
  let out = text;
  for (const [name, regex, replace] of PATTERNS) {
    out = out.replace(regex, replace ?? `[REDACTED:${name}]`);
  }
  return out;
}

/** Keeps the last `maxBytes` bytes of UTF-8 text, never splitting a character. */
export function keepTail(text: string, maxBytes: number): string {
  const bytes = Buffer.from(text, "utf8");
  if (bytes.length <= maxBytes) return text;
  let start = bytes.length - maxBytes;
  // Skip UTF-8 continuation bytes so the cut lands on a character boundary.
  while (start < bytes.length && ((bytes[start] ?? 0) & 0xc0) === 0x80) start++;
  return `[… ${start} earlier bytes cut]\n` + bytes.subarray(start).toString("utf8");
}
