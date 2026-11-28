import { useEffect, useRef } from "react";
import type { JobView } from "../generated/JobView";
import { useBackend } from "../store";
import { ago } from "../time";
import { Banner, Chip, Go, go } from "../ui";
import type { Ctx } from "./Group";
import { Toggle } from "./GroupMembers";

const days = ["Sundays", "Mondays", "Tuesdays", "Wednesdays", "Thursdays", "Fridays", "Saturdays"];
const list = (a: string[]) =>
  a.length < 2 ? a.join("") : `${a.slice(0, -1).join(", ")} and ${a.at(-1)}`;
const nums = (field: string) =>
  field.split(",").flatMap((p) => {
    const [a, b] = p.split("-").map(Number);
    if (a === undefined || Number.isNaN(a)) return [];
    return Array.from({ length: (b ?? a) - a + 1 }, (_, i) => a + i);
  });
const two = (n: number) => String(n).padStart(2, "0");

/** The cron subset a job may use (exact minutes and hours, weekdays) in words. */
export function when(cron: string) {
  const [min = "0", hour = "0", , , dow = "*"] = cron.trim().split(/\s+/);
  const times = nums(hour).flatMap((h) => nums(min).map((m) => `${two(h)}:${two(m)}`));
  const every = dow === "*";
  const names = list(nums(dow).map((d) => days[d % 7] ?? ""));
  return {
    at: times.join(", "),
    days: every ? "Every day" : names,
    on: every ? "every day" : `on ${names}`,
  };
}
const sched = (j: JobView) => `${when(j.cron).days} ${when(j.cron).at}`;

const result = (r: string) => (r === "ok" ? "done" : r === "failed" ? "failed" : r);
const lastRun = (j: JobView, now: number) =>
  j.lastRun > 0 ? `last run: ${result(j.lastResult)}, ${ago(j.lastRun, now)}` : "";

/** v2-ScreenGroupJobs, JobScheduled and JobRan: the group's jobs, the switch per job, run now, open log. */
export function Jobs(c: Ctx) {
  const { g, call, snap } = c;
  const backend = useBackend();
  const jobs = snap.jobs.filter((j) => j.repo === g.repo);
  const running = useRef<{ id: string; at: number } | null>(null);
  // Run now starts the job in its own process: say when the result shows up in the snapshot.
  useEffect(() => {
    const w = running.current;
    const j = jobs.find((x) => x.id === w?.id);
    if (w && j && j.lastRun !== w.at) {
      running.current = null;
      c.toast(`${j.title} finished`);
    }
  });
  const run = async (j: JobView) => {
    running.current = { id: j.id, at: j.lastRun };
    const r = await call({ kind: "jobRunNow", data: { repo: g.repo, id: j.id } });
    if ("err" in r) {
      running.current = null;
      return c.toast(r.err);
    }
    c.toast(`${j.title} started`);
  };
  const log = async (j: JobView) => {
    const r = await call({ kind: "jobFile", data: { repo: g.repo, id: j.id, report: false } });
    if ("err" in r) return c.toast(r.err);
    if (r.ev.kind === "path" && r.ev.data) void backend.openTarget(r.ev.data);
    else c.toast("There is no log yet.");
  };
  const turn = async (j: JobView) => {
    const r = await call({ kind: "jobEnable", data: { repo: g.repo, id: j.id, on: !j.enabled } });
    if ("err" in r) c.toast(r.err);
  };
  const auto = async () => {
    const r = await call({
      kind: "jobsAutoAccept",
      data: { repo: g.repo, on: !g.jobsAutoAccept },
    });
    c.toast(
      "err" in r
        ? r.err
        : g.jobsAutoAccept
          ? "New or changed jobs need your yes again."
          : "New or changed jobs from this group now run without asking.",
    );
  };
  return (
    <>
      <div className="card">
        <h2>Jobs</h2>
        {jobs.length === 0 && (
          <p className="muted" style={{ margin: 0 }}>
            This group has no scheduled jobs.
          </p>
        )}
        {jobs.map((j) => {
          const skipped = j.lastResult.startsWith("skipped");
          const bad = j.lastRun > 0 && !skipped && j.lastResult !== "ok";
          const chip = !j.accepted ? (
            <Chip kind="warn">Waiting for your yes</Chip>
          ) : !j.enabled ? (
            <Chip kind="n">Off</Chip>
          ) : skipped ? (
            <Chip kind="n">Skipped</Chip>
          ) : bad ? (
            <Chip kind="err">Failed</Chip>
          ) : (
            <Chip kind="ok">On</Chip>
          );
          const sub = !j.accepted
            ? `${sched(j)} · runs claude with a ${j.prompt.length}-character prompt · ${j.timeoutMinutes} min limit`
            : [sched(j), lastRun(j, c.now) || (j.enabled ? "OS task written" : "")]
                .filter(Boolean)
                .join(" · ");
          return (
            <div className="row" key={j.id}>
              <div>
                <b>{j.title}</b> {chip}
                <div className="sm muted">{sub}</div>
              </div>
              <div className="acts">
                {!j.accepted ? (
                  <a className="b p" href={`#/group/${g.repo}/jobs/${j.id}`}>
                    Review
                  </a>
                ) : (
                  <>
                    {j.lastRun > 0 && (
                      <button type="button" className="b g" onClick={() => log(j)}>
                        Open log
                      </button>
                    )}
                    <button type="button" className="b" onClick={() => run(j)}>
                      Run now
                    </button>
                    <Toggle
                      on={j.enabled}
                      label={j.enabled ? "Turn off" : "Turn on"}
                      set={() => turn(j)}
                    />
                  </>
                )}
              </div>
            </div>
          );
        })}
      </div>
      <div className="card">
        <div className="row">
          <div>Accept new jobs from this group without asking</div>
          <div className="acts">
            <Toggle
              on={g.jobsAutoAccept}
              label={g.jobsAutoAccept ? "Turn off" : "Turn on"}
              set={auto}
            />
          </div>
        </div>
        {g.jobsAutoAccept && (
          <Banner kind="warn">
            New or changed jobs run on your devices without your review, with the permissions the
            group gives them (bypassPermissions included). Jobs you turned off stay off.
          </Banner>
        )}
      </div>
    </>
  );
}

/** v2-ScreenGroupJobAccept: the whole job in plain words, then the person's yes. */
export function JobAccept(c: Ctx) {
  const { g, call, snap } = c;
  const j = snap.jobs.find((x) => x.repo === g.repo && x.id === c.sub[1]);
  const back = `group/${g.repo}/jobs`;
  if (!j || j.accepted) return <Go to={back} />;
  const w = when(j.cron);
  const accept = async () => {
    const r = await call({ kind: "jobAccept", data: { repo: g.repo, id: j.id } });
    if ("err" in r) return c.toast(r.err);
    c.toast(`Scheduled: ${j.title} runs ${w.on} at ${w.at}`);
    go(back);
  };
  return (
    <div className="card">
      <h2>{j.title}</h2>
      {j.mode === "bypassPermissions" && (
        <Banner kind="warn">
          Permission mode bypassPermissions: Claude runs tools without asking. Accept only if you
          trust this group.
        </Banner>
      )}
      <p style={{ margin: "0 0 6px" }}>
        {w.days} at {w.at}. Timeout {j.timeoutMinutes} minutes. Folder {j.cwd}. Permission mode:{" "}
        <b>{j.mode}</b>.
        {j.args.length > 0 && (
          <>
            {" "}
            Extra args: <span className="mono">{j.args.join(" ")}</span> (allowed: --chrome,
            --model, --effort).
          </>
        )}
      </p>
      <div className="diff" style={{ whiteSpace: "pre-wrap" }}>
        {j.prompt}
      </div>
      <p className="sm muted">Nothing runs until you accept. You can turn it off any time.</p>
      <div className="acts">
        <button type="button" className="b p" onClick={accept}>
          Accept and schedule
        </button>
        <a className="b" href={`#/${back}`}>
          Not now
        </a>
      </div>
    </div>
  );
}
