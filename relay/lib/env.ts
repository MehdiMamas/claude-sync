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
  allowedUsers: (): Set<string> => new Set(list(process.env.ALLOWED_GITHUB_USERS).map((s) => s.toLowerCase())),
  groupRepos: (): string[] => list(process.env.GROUP_REPOS).filter((r) => REPO_RE.test(r)),
  publicUrl: (): string | undefined => process.env.RELAY_PUBLIC_URL?.trim().replace(/\/+$/, "") || undefined,
};
