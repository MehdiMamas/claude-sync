import type { Milestone } from "./github.ts";

const SPRINT_RE = /^Sprint \d+$/;

/**
 * The nearest sprint (docs/architecture.md §12): among open milestones titled
 * `Sprint N` with a due date, the earliest one due today (UTC) or later; else
 * the one with the latest due date; else none.
 */
export function pickSprint(milestones: readonly Milestone[], now: Date): Milestone | null {
  const today = now.toISOString().slice(0, 10);
  const sprints = milestones
    .filter((m) => m.state === "open" && SPRINT_RE.test(m.title) && typeof m.due_on === "string")
    .map((m) => ({ m, due: (m.due_on as string).slice(0, 10) }));
  const upcoming = sprints.filter((s) => s.due >= today).sort((a, b) => a.due.localeCompare(b.due));
  if (upcoming[0]) return upcoming[0].m;
  const past = sprints.sort((a, b) => b.due.localeCompare(a.due));
  return past[0]?.m ?? null;
}
