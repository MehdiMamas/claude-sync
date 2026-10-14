import { HttpError } from "./http.ts";
import { keepTail, redact } from "./redact.ts";

export const MAX_TITLE = 200;
export const MAX_DESCRIPTION = 20_000;
export const MAX_SCREENSHOT_BYTES = 3 * 1024 * 1024;
export const MAX_LOG_BYTES = 200 * 1024;
// GitHub rejects issue bodies over 65,536 characters; keep a margin.
export const MAX_ISSUE_BODY = 60_000;

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

/** A backtick fence longer than any backtick run in `text`, so the text cannot close it. */
export function fenceFor(text: string): string {
  const longest = Math.max(0, ...Array.from(text.matchAll(/`+/g), (m) => m[0].length));
  return "`".repeat(Math.max(3, longest + 1));
}

/** Text safe for inline code inside a table cell: no backticks, pipes or angle brackets. */
function cell(s: string): string {
  const clean = s.replace(/[`|<>\\]/g, "").trim();
  return clean ? `\`${clean}\`` : "_unknown_";
}

function lastChars(text: string, max: number): string {
  if (text.length <= max) return text;
  let out = text.slice(text.length - max);
  // Do not start on the second half of a surrogate pair.
  if (/^[\udc00-\udfff]/.test(out)) out = out.slice(1);
  return out;
}

export interface IssueBodyInput {
  feedback: Feedback;
  /** Markdown image URL, "failed" when the upload failed, or null when there was no screenshot. */
  screenshotUrl: string | null | "failed";
  /** Link to the full log file when the logs had to be cut to fit the issue body. */
  fullLogsUrl: string | null | "failed";
}

export function buildIssueBody({ feedback, screenshotUrl, fullLogsUrl }: IssueBodyInput): string {
  const head: string[] = [
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
  ];
  if (screenshotUrl === "failed") {
    head.push("### Screenshot", "", "_The screenshot could not be stored._", "");
  } else if (screenshotUrl) {
    head.push("### Screenshot", "", `![screenshot](${screenshotUrl})`, "");
  }
  if (!feedback.logs) {
    head.push("_No logs attached._");
    return head.join("\n");
  }

  const headText = head.join("\n");
  const overhead = 400; // details, summary, fences and the cut note
  const budget = Math.max(0, MAX_ISSUE_BODY - headText.length - overhead);
  const shown = lastChars(feedback.logs, budget);
  const cut = shown.length < feedback.logs.length;
  const fence = fenceFor(shown);
  const summary = cut ? "Logs (end only, redacted)" : "Logs (redacted)";
  const lines = [headText, "<details>", `<summary>${summary}</summary>`, ""];
  if (cut) {
    lines.push(
      fullLogsUrl && fullLogsUrl !== "failed"
        ? `Only the end fits here. [Full logs (last 200 KB)](${fullLogsUrl}).`
        : "Only the end fits here; the full logs could not be stored.",
      "",
    );
  }
  lines.push(`${fence}text`, shown, fence, "", "</details>");
  return lines.join("\n");
}

/** True when the logs will not fit in the issue body next to the rest. */
export function logsNeedFile(feedback: Feedback): boolean {
  return feedback.logs.length + feedback.description.length + 2_000 > MAX_ISSUE_BODY;
}
