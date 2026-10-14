import type { Attachment } from "./attachments.ts";
import { HttpError } from "./http.ts";
import { keepTail, redact } from "./redact.ts";

export const MAX_TITLE = 200;
export const MAX_DESCRIPTION = 20_000;
export const MAX_SCREENSHOT_BYTES = 3 * 1024 * 1024;
export const MAX_LOG_BYTES = 200 * 1024;

export type Kind = "bug" | "idea";

export interface Screenshot {
  bytes: Buffer;
  ext: "png" | "jpg";
}

export interface Feedback {
  title: string;
  description: string;
  kind: Kind;
  appVersion: string;
  os: string;
  /** Redacted and cut to the last 200 KB. */
  logs: string;
  screenshot: Screenshot | null;
}

const PNG_MAGIC = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
const JPEG_MAGIC = Buffer.from([0xff, 0xd8, 0xff]);

function bad(message: string): never {
  throw new HttpError(400, message);
}

// Control characters (newlines included) become spaces.
function oneLine(s: string): string {
  return s.replace(/[\u0000-\u001f\u007f]+/g, " ").trim();
}

function str(body: Record<string, unknown>, key: string, max: number, opts: { optional?: boolean } = {}): string {
  const value = body[key];
  if (value === undefined || value === null) {
    if (opts.optional) return "";
    bad(`${key} is required`);
  }
  if (typeof value !== "string") bad(`${key} must be a string`);
  if (value.length > max) bad(`${key} is longer than ${max} characters`);
  if (!opts.optional && value.trim().length === 0) bad(`${key} is required`);
  return value;
}

export function decodeScreenshot(value: unknown): Screenshot | null {
  if (value === undefined || value === null || value === "") return null;
  if (typeof value !== "string") bad("screenshot must be a base64 string");
  const data = value.replace(/^data:image\/(?:png|jpeg);base64,/, "").replace(/\s+/g, "");
  if (!/^[A-Za-z0-9+/]*={0,2}$/.test(data) || data.length % 4 === 1) bad("screenshot is not valid base64");
  // Check the size before decoding so a huge string is not decoded.
  if (Math.floor((data.length * 3) / 4) - 2 > MAX_SCREENSHOT_BYTES) {
    throw new HttpError(413, "screenshot is larger than 3 MB");
  }
  const bytes = Buffer.from(data, "base64");
  if (bytes.length > MAX_SCREENSHOT_BYTES) throw new HttpError(413, "screenshot is larger than 3 MB");
  if (bytes.subarray(0, 8).equals(PNG_MAGIC)) return { bytes, ext: "png" };
  if (bytes.subarray(0, 3).equals(JPEG_MAGIC)) return { bytes, ext: "jpg" };
  bad("screenshot must be a PNG or JPEG image");
}

/** Validates the POST /api/feedback body and redacts what came from the user's machine. */
export function parseFeedback(raw: unknown): Feedback {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) bad("body must be a JSON object");
  const body = raw as Record<string, unknown>;
  const kind = body.kind;
  if (kind !== "bug" && kind !== "idea") bad('kind must be "bug" or "idea"');
  const logs = str(body, "logs", Number.MAX_SAFE_INTEGER, { optional: true });
  return {
    title: oneLine(redact(str(body, "title", MAX_TITLE))),
    description: redact(str(body, "description", MAX_DESCRIPTION)).trim(),
    kind,
    appVersion: oneLine(str(body, "appVersion", 64)),
    os: oneLine(str(body, "os", 200)),
    // Redact before cutting so a cut can never leave half a secret unmatched.
    logs: keepTail(redact(logs), MAX_LOG_BYTES),
    screenshot: decodeScreenshot(body.screenshot),
  };
}

/** Text safe for inline code inside a table cell: no backticks, pipes or angle brackets. */
function cell(s: string): string {
  const clean = s.replace(/[`|<>\\]/g, "").trim();
  return clean ? `\`${clean}\`` : "_unknown_";
}

/** "screenshot and logs", "screenshot", "logs", or "" when there is nothing to attach. */
function attachedWhat(feedback: Feedback): string {
  return [feedback.screenshot ? "screenshot" : "", feedback.logs ? "logs" : ""].filter(Boolean).join(" and ");
}

/**
 * The public issue body: the description and the environment, nothing from the user's
 * disk. The screenshot and logs go to the private attachments repo (`stored: false`
 * when that failed).
 */
export function buildIssueBody(feedback: Feedback, stored = true): string {
  const what = attachedWhat(feedback);
  const note = !what
    ? "_No screenshot or logs attached._"
    : stored
      ? `_The ${what} went privately to the maintainer._`
      : `_The ${what} could not be stored._`;
  return [
    feedback.description,
    "",
    "---",
    "",
    "| | |",
    "|---|---|",
    `| Kind | ${feedback.kind} |`,
    `| App version | ${cell(feedback.appVersion)} |`,
    `| OS | ${cell(feedback.os)} |`,
    "",
    note,
  ].join("\n");
}

/** The files kept privately for issue `number`: the screenshot, the logs and the report. */
export function attachmentsFor(
  feedback: Feedback,
  issue: { number: number; url: string; login: string; repo: string; createdAt: string },
): Attachment[] {
  if (!attachedWhat(feedback)) return [];
  const dir = `issues/${issue.number}`;
  const files: Attachment[] = [];
  if (feedback.screenshot) {
    files.push({ path: `${dir}/screenshot.${feedback.screenshot.ext}`, bytes: feedback.screenshot.bytes });
  }
  if (feedback.logs) files.push({ path: `${dir}/logs.txt`, bytes: Buffer.from(feedback.logs, "utf8") });
  const report = {
    repo: issue.repo,
    number: issue.number,
    url: issue.url,
    login: issue.login,
    kind: feedback.kind,
    appVersion: feedback.appVersion,
    os: feedback.os,
    createdAt: issue.createdAt,
  };
  files.push({ path: `${dir}/report.json`, bytes: Buffer.from(`${JSON.stringify(report, null, 2)}\n`, "utf8") });
  return files;
}
