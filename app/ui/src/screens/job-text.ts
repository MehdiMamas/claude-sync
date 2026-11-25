// How a scheduled job reads when a member is asked to accept it: its permission mode, and the
// details shared by the jobs card and the Optional packs card.
import type { PermissionMode } from "../api";
import { h } from "../dom";

export const MODES: Record<PermissionMode, string> = {
  default: "Claude Code asks before edits and commands. Nobody is there to answer, so it skips them.",
  acceptEdits: "Claude Code edits files without asking; it skips other commands that need asking.",
  plan: "Claude Code only reads and plans; it changes nothing.",
  bypassPermissions: "Claude Code runs any command and edits any file without asking.",
};

export function permission(mode: PermissionMode): HTMLElement {
  const text = [h("strong", null, mode), ": ", MODES[mode] ?? ""];
  return mode === "bypassPermissions"
    ? h("div", { class: "notice perm" }, h("div", { class: "notice-text" }, ...text))
    : h("p", { class: "small" }, ...text);
}

/** What a job does, as the accept card shows it (no title, no buttons). */
export interface JobDetails {
  schedule: string;
  timeoutMinutes: number;
  cwd: string;
  prompt: string;
  permissionMode: PermissionMode;
  args: string[];
  report: string | null;
}

export function jobDetails(j: JobDetails): HTMLElement[] {
  return [
    h("p", { class: "small" }, h("strong", null, j.schedule), ` (your time), for at most ${j.timeoutMinutes} min, in `, h("code", null, j.cwd), "."),
    h("p", { class: "small" }, "Claude Code gets this prompt:"),
    h("pre", { class: "logs prompt" }, j.prompt),
    permission(j.permissionMode),
    j.args.length > 0 && h("p", { class: "small" }, "Claude Code options: ", h("code", null, j.args.join(" ")), j.args.includes("--chrome") && " (uses Chrome through Claude in Chrome when it is running)"),
    j.report && h("p", { class: "muted small" }, "Its report: ", h("code", null, j.report)),
    h("p", { class: "muted small" }, "It only runs on this computer. If the group changes it, it stops until you accept it again."),
  ].filter((x): x is HTMLElement => x !== false && x !== null);
}
