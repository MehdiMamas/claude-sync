// The triage bot: a GitHub App installed only on the claude-sync repos. Every write the
// triage makes (comments, labels, close, lock) uses an installation token of this App, so
// nothing is ever posted under the maintainer's account. There is no fallback to a
// personal token (`gh auth token`, GH_TOKEN, GITHUB_TOKEN).
//
//   TRIAGE_APP_ID               the App's id
//   TRIAGE_APP_INSTALLATION_ID  the installation id on the repo
//   TRIAGE_APP_KEY_FILE         path to the App's PEM private key, mode 0600, kept outside the repo

import { createPrivateKey, sign } from "node:crypto";
import { readFileSync, statSync } from "node:fs";
import { GITHUB_API } from "../lib/github.ts";

export const NOT_CONFIGURED =
  "triage bot not configured: set TRIAGE_APP_ID, TRIAGE_APP_INSTALLATION_ID and TRIAGE_APP_KEY_FILE (path to the GitHub App private key, mode 0600)";

export interface BotConfig {
  appId: string;
  installationId: string;
  keyFile: string;
}

export function botConfig(env: Record<string, string | undefined> = process.env): BotConfig | null {
  const appId = env.TRIAGE_APP_ID?.trim();
  const installationId = env.TRIAGE_APP_INSTALLATION_ID?.trim();
  const keyFile = env.TRIAGE_APP_KEY_FILE?.trim();
  if (!appId || !installationId || !keyFile) return null;
  return { appId, installationId, keyFile };
}

/** Reads the private key; refuses a file that group or others can access. Windows has no
 *  POSIX mode bits (every file reads as 0666), so the check runs only elsewhere. */
export function readKeyFile(path: string): string {
  const st = statSync(path);
  if (!st.isFile()) throw new Error("TRIAGE_APP_KEY_FILE is not a file");
  if (process.platform !== "win32" && (st.mode & 0o077) !== 0) {
    throw new Error(`TRIAGE_APP_KEY_FILE ${path} is accessible by group or others (mode ${(st.mode & 0o777).toString(8)}): run chmod 600 on it`);
  }
  return readFileSync(path, "utf8");
}

const b64url = (b: Buffer | string): string => Buffer.from(b).toString("base64url");

/** An RS256 App JWT: issued 60 s in the past (clock drift), valid 9 minutes (GitHub allows 10). */
export function appJwt(appId: string, pem: string, nowMs: number = Date.now()): string {
  const now = Math.floor(nowMs / 1000);
  const head = b64url(JSON.stringify({ alg: "RS256", typ: "JWT" }));
  const body = b64url(JSON.stringify({ iat: now - 60, exp: now + 9 * 60, iss: appId }));
  const sig = sign("RSA-SHA256", Buffer.from(`${head}.${body}`), createPrivateKey(pem));
  return `${head}.${body}.${b64url(sig)}`;
}

/** Mints an installation token. Throws a clear error when the bot is not configured. */
export async function botToken(
  env: Record<string, string | undefined> = process.env,
  fetchFn: typeof fetch = fetch,
  nowMs: number = Date.now(),
): Promise<string> {
  const cfg = botConfig(env);
  if (!cfg) throw new Error(NOT_CONFIGURED);
  const jwt = appJwt(cfg.appId, readKeyFile(cfg.keyFile), nowMs);
  const res = await fetchFn(`${GITHUB_API}/app/installations/${encodeURIComponent(cfg.installationId)}/access_tokens`, {
    method: "POST",
    headers: {
      accept: "application/vnd.github+json",
      authorization: `Bearer ${jwt}`,
      "user-agent": "claude-sync-triage",
      "x-github-api-version": "2022-11-28",
    },
  });
  if (!res.ok) {
    res.body?.cancel().catch(() => {});
    throw new Error(`GitHub installation token returned ${res.status}`);
  }
  const data = (await res.json()) as { token?: unknown };
  if (typeof data.token !== "string" || !data.token) throw new Error("GitHub installation token response had no token");
  return data.token;
}
