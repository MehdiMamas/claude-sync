import { HttpError, logError } from "./http.ts";

const REPO_RE = /^[A-Za-z0-9-]+\/[A-Za-z0-9._-]+$/;

function list(value: string | undefined): string[] {
  return (value ?? "")
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

function required(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) {
    logError("config", new Error(`${name} is not set`));
    throw new HttpError(500, "relay is not configured");
  }
  return value;
}

function repo(name: string, value: string): string {
  if (!REPO_RE.test(value)) {
    logError("config", new Error(`${name} must be owner/repo`));
    throw new HttpError(500, "relay is not configured");
  }
  return value;
}

/** Read on every call so tests (and Vercel env changes) need no module reload. */
export const env = {
  relayToken: (): string => required("RELAY_GITHUB_TOKEN"),
  releasesRepo: (): string => repo("RELEASES_REPO", required("RELEASES_REPO")),
  feedbackRepo: (): string => {
    const value = process.env.FEEDBACK_REPO?.trim();
    return value ? repo("FEEDBACK_REPO", value) : env.releasesRepo();
  },
  /** Logins refused with 403 (abuse); everyone else with a valid GitHub token is allowed. */
  blockedUsers: (): Set<string> => new Set(list(process.env.BLOCKED_GITHUB_USERS).map((s) => s.toLowerCase())),
  /** Install page sign-in (GitHub OAuth App web flow). All three must be set, else the page says it is not configured. */
  oauthClientId: (): string | undefined => process.env.GITHUB_OAUTH_CLIENT_ID?.trim() || undefined,
  oauthClientSecret: (): string | undefined => process.env.GITHUB_OAUTH_CLIENT_SECRET?.trim() || undefined,
  installSessionSecret: (): string | undefined => process.env.INSTALL_SESSION_SECRET?.trim() || undefined,
  installConfigured: (): boolean => !!(env.oauthClientId() && env.oauthClientSecret() && env.installSessionSecret()),
  publicUrl: (): string | undefined => process.env.RELAY_PUBLIC_URL?.trim().replace(/\/+$/, "") || undefined,
};
