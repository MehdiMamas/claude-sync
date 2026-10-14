import { randomUUID } from "node:crypto";
import { requireCaller } from "../lib/auth.ts";
import { env } from "../lib/env.ts";
import { buildIssueBody, type Feedback, logsNeedFile, parseFeedback } from "../lib/feedback.ts";
import { discard, github, GitHubError, githubJson, type Milestone, repoPath } from "../lib/github.ts";
import { handler, HttpError, json, logError } from "../lib/http.ts";
import { pickSprint } from "../lib/milestone.ts";
import { FEEDBACK_LIMIT, FEEDBACK_WINDOW_MS, take } from "../lib/ratelimit.ts";

const ASSETS_BRANCH = "feedback-assets";
// Vercel caps request bodies at 4.5 MB; refuse early above that.
const MAX_BODY_BYTES = 4_500_000;

function refPath(branch: string): string {
  return branch.split("/").map(encodeURIComponent).join("/");
}

/** Creates `feedback-assets` from the default branch head when it does not exist. */
async function ensureAssetsBranch(repo: string, token: string): Promise<void> {
  const base = repoPath(repo);
  const res = await github(`${base}/git/ref/heads/${ASSETS_BRANCH}`, { token });
  discard(res);
  if (res.ok) return;
  if (res.status !== 404) throw new GitHubError("feedback-assets ref", res.status);

  const { default_branch } = await githubJson<{ default_branch: string }>("repo", base, { token });
  const head = await githubJson<{ object: { sha: string } }>(
    "default branch ref",
    `${base}/git/ref/heads/${refPath(default_branch)}`,
    { token },
  );
  const created = await github(`${base}/git/refs`, {
    token,
    method: "POST",
    body: { ref: `refs/heads/${ASSETS_BRANCH}`, sha: head.object.sha },
  });
  discard(created);
  // 422: another request created it first.
  if (!created.ok && created.status !== 422) throw new GitHubError("create feedback-assets", created.status);
}

/** Commits one file to `feedback-assets` and returns a URL that renders it. */
async function commitAsset(repo: string, token: string, path: string, bytes: Buffer, message: string): Promise<string> {
  const res = await github(`${repoPath(repo)}/contents/${refPath(path)}`, {
    token,
    method: "PUT",
    body: { message, content: bytes.toString("base64"), branch: ASSETS_BRANCH },
  });
  discard(res);
  if (!res.ok) throw new GitHubError("commit feedback asset", res.status);
  return `https://github.com/${repo}/blob/${ASSETS_BRANCH}/${path}?raw=true`;
}

async function nearestSprint(repo: string, token: string): Promise<number | null> {
  try {
    const milestones = await githubJson<Milestone[]>(
      "milestones",
      `${repoPath(repo)}/milestones?state=open&per_page=100`,
      { token },
    );
    return pickSprint(milestones, new Date())?.number ?? null;
  } catch (err) {
    logError("feedback milestones", err);
    return null;
  }
}

async function readBody(req: Request): Promise<unknown> {
  const length = Number(req.headers.get("content-length") ?? "0");
  if (length > MAX_BODY_BYTES) throw new HttpError(413, "request body is too large");
  const text = await req.text();
  if (Buffer.byteLength(text) > MAX_BODY_BYTES) throw new HttpError(413, "request body is too large");
  try {
    return JSON.parse(text);
  } catch {
    throw new HttpError(400, "body must be JSON");
  }
}

async function storeAssets(
  repo: string,
  token: string,
  feedback: Feedback,
): Promise<{ screenshotUrl: string | null | "failed"; fullLogsUrl: string | null | "failed" }> {
  const wantLogs = logsNeedFile(feedback);
  if (!feedback.screenshot && !wantLogs) return { screenshotUrl: null, fullLogsUrl: null };

  const now = new Date();
  const dir = `feedback/${now.toISOString().slice(0, 7)}`;
  const id = randomUUID();
  let screenshotUrl: string | null | "failed" = feedback.screenshot ? "failed" : null;
  let fullLogsUrl: string | null | "failed" = wantLogs ? "failed" : null;
  // Best effort: feedback without its screenshot beats no feedback.
  try {
    await ensureAssetsBranch(repo, token);
    if (feedback.screenshot) {
      const { bytes, ext } = feedback.screenshot;
      screenshotUrl = await commitAsset(repo, token, `${dir}/${id}.${ext}`, bytes, `feedback: screenshot ${id}`);
    }
    if (wantLogs) {
      fullLogsUrl = await commitAsset(repo, token, `${dir}/${id}.log`, Buffer.from(feedback.logs, "utf8"), `feedback: logs ${id}`);
    }
  } catch (err) {
    logError("feedback assets", err);
  }
  return { screenshotUrl, fullLogsUrl };
}

/**
 * POST /api/feedback
 * JSON { title, description, kind: "bug"|"idea", appVersion, os, logs?, screenshot? }
 * 201 { number, url } with the new issue.
 */
export const POST = handler("feedback", async (req) => {
  const { login } = await requireCaller(req);
  const feedback = parseFeedback(await readBody(req));

  const slot = take(`feedback:${login}`, FEEDBACK_LIMIT, FEEDBACK_WINDOW_MS);
  if (!slot.ok) {
    throw new HttpError(429, "too many feedback posts, try again later", {
      "retry-after": String(slot.retryAfterSeconds),
    });
  }

  const repo = env.feedbackRepo();
  const token = env.relayToken();
  const assets = await storeAssets(repo, token, feedback);
  const milestone = await nearestSprint(repo, token);

  const issue = await githubJson<{ number: number; html_url: string }>("create issue", `${repoPath(repo)}/issues`, {
    token,
    method: "POST",
    body: {
      title: feedback.title,
      body: buildIssueBody({ feedback, login, ...assets }),
      labels: ["feedback", `type:${feedback.kind}`],
      ...(milestone !== null ? { milestone } : {}),
    },
  });
  return json(201, { number: issue.number, url: issue.html_url });
});
