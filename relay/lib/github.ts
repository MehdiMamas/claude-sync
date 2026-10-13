// Minimal GitHub REST client on the global fetch. Tokens are passed per call and
// never logged.

export const GITHUB_API = "https://api.github.com";

export interface GitHubRequest {
  token: string;
  method?: string;
  body?: unknown;
  accept?: string;
  redirect?: RequestInit["redirect"];
}

export async function github(path: string, opts: GitHubRequest): Promise<Response> {
  const headers: Record<string, string> = {
    accept: opts.accept ?? "application/vnd.github+json",
    authorization: `Bearer ${opts.token}`,
    "user-agent": "claude-sync-relay",
    "x-github-api-version": "2022-11-28",
  };
  const init: RequestInit = { method: opts.method ?? "GET", headers };
  if (opts.body !== undefined) {
    headers["content-type"] = "application/json";
    init.body = JSON.stringify(opts.body);
  }
  if (opts.redirect) init.redirect = opts.redirect;
  return fetch(`${GITHUB_API}${path}`, init);
}

/** Error for an unexpected GitHub status. The message names the call, never a token. */
export class GitHubError extends Error {
  readonly status: number;
  constructor(what: string, status: number) {
    super(`GitHub ${what} returned ${status}`);
    this.status = status;
  }
}

export async function githubJson<T>(what: string, path: string, opts: GitHubRequest): Promise<T> {
  const res = await github(path, opts);
  if (!res.ok) {
    await res.body?.cancel();
    throw new GitHubError(what, res.status);
  }
  return (await res.json()) as T;
}

export function repoPath(repo: string): string {
  const [owner, name] = repo.split("/");
  return `/repos/${encodeURIComponent(owner ?? "")}/${encodeURIComponent(name ?? "")}`;
}

export interface ReleaseAsset {
  id: number;
  name: string;
  size: number;
}

export interface Release {
  tag_name: string;
  name: string | null;
  body: string | null;
  draft: boolean;
  prerelease: boolean;
  published_at: string | null;
  assets: ReleaseAsset[];
}

export interface Milestone {
  number: number;
  title: string;
  state: string;
  due_on: string | null;
}
