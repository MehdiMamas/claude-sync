import { useState } from "react";
import { Banner, Chip, go, Spin, useLoad } from "../ui";
import { type Ctx, data } from "./Group";

const name = (path: string) => path.replace(/\/SKILL\.md$/, "");
const link = (c: Ctx, sub: string) => `#/group/${c.g.repo}/${sub}`;

/** v2-ScreenGroupLocalEdits, EditKept, EditUndone and Shared: edits to delivered files. */
export function Edits(c: Ctx) {
  const { g, call } = c;
  const [edits, reload] = useLoad(
    () => data(call, { kind: "editsList", data: { repo: g.repo } }, "edits"),
    g.repo,
  );
  const [items] = useLoad(
    () => data(call, { kind: "contents", data: { repo: g.repo } }, "contents"),
    g.repo,
  );
  const others = c.snap.groups.filter((x) => x.repo !== g.repo && !x.off);
  const hasMd = items?.some((i) => i.kind === "claudeMd");
  const act = async (cmd: "keepMine" | "undoEdit", path: string) => {
    const r = await call({ kind: cmd, data: { repo: g.repo, path } });
    if ("err" in r) return c.toast(r.err);
    c.toast(
      cmd === "keepMine"
        ? "Kept yours. It is not shared."
        : `Undone: the group version of ${name(path)} is back`,
    );
    reload();
  };
  return (
    <div className="card">
      <h2>Your local edits</h2>
      {edits?.length === 0 && (
        <p className="muted" style={{ margin: 0 }}>
          Nothing left to share.
        </p>
      )}
      {(edits ?? []).map((e) => (
        <div className="row" key={e.path}>
          <div>
            <span className="mono">{e.path}</span>
            {e.kept && (
              <>
                {" "}
                <Chip kind="n">Kept yours</Chip>
                <div className="sm muted">
                  stays on your machines, not shared; a group change to this file asks you first
                </div>
              </>
            )}
          </div>
          <div className="acts">
            <a className="b p" href={link(c, `share/${e.path}`)}>
              Share
            </a>
            <button type="button" className="b" onClick={() => act("undoEdit", e.path)}>
              Undo
            </button>
            {!e.kept && (
              <button type="button" className="b" onClick={() => act("keepMine", e.path)}>
                Keep mine
              </button>
            )}
          </div>
        </div>
      ))}
      {hasMd && others.length > 0 && (
        <div className="row">
          <div>
            <span className="mono">CLAUDE.md</span>
          </div>
          <div className="acts">
            <a className="b" href={link(c, "copy")}>
              Copy to group
            </a>
          </div>
        </div>
      )}
    </div>
  );
}

/** v2-ScreenGroupShare and ShareBlocked: the diff and the secret scan, then one click to share. */
export function Share(c: Ctx) {
  const { g, call } = c;
  const path = c.sub.slice(1).join("/");
  const [pv] = useLoad(
    () => data(call, { kind: "sharePreview", data: { repo: g.repo, path } }, "sharePreview"),
    path,
  );
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const problem = err ?? pv?.secret ?? null;
  const hit = problem?.match(/secret found in (.+):(\d+) \((.+?)\);/);
  const send = async () => {
    setBusy(true);
    const r = await call({ kind: "shareEdit", data: { repo: g.repo, path } });
    setBusy(false);
    if ("err" in r) return setErr(r.err);
    c.toast(
      r.ev.kind === "proposed"
        ? `Proposed as #${r.ev.data}. A member must approve it.`
        : "Shared. Members get it on their next sync.",
    );
    go(`group/${g.repo}/edits`);
  };
  return (
    <>
      {problem && (
        <Banner kind="err">
          {hit
            ? `Not shared: ${hit[1]} line ${hit[2]} looks like a secret (${hit[3]}). Remove it, or mark the line claude-sync: allow-secret.`
            : `Not shared: ${problem}`}
        </Banner>
      )}
      <div className="card">
        <h2>Share {name(path)}</h2>
        {!pv && (
          <p className="muted">
            <Spin />
            Checking…
          </p>
        )}
        {pv && (
          <div className="diff">
            {pv.added.map((l, i) => (
              <div className="add" key={i}>
                + {l}
              </div>
            ))}
          </div>
        )}
        {pv && !pv.secret && !err && <p className="sm muted">Checked for secrets: none found.</p>}
        <div className="acts">
          {pv && !pv.secret && (
            <button type="button" className="b p" disabled={busy} onClick={send}>
              Share with group
            </button>
          )}
          <a className="b" href={link(c, "edits")}>
            {pv?.secret ? "Back" : "Cancel"}
          </a>
        </div>
      </div>
    </>
  );
}

/** v2-ScreenGroupContents: what this group gives this device. */
export function Contents(c: Ctx) {
  const { g, call } = c;
  const [items] = useLoad(
    () => data(call, { kind: "contents", data: { repo: g.repo } }, "contents"),
    g.repo + g.pluginProblems.length,
  );
  const [edits] = useLoad(
    () => data(call, { kind: "editsList", data: { repo: g.repo } }, "edits"),
    g.repo,
  );
  const n = (kind: string) => items?.filter((i) => i.kind === kind).length ?? 0;
  const plugins = n("plugin");
  const settings = items?.find((i) => i.kind === "settings");
  return (
    <div className="card">
      <h2>In this group</h2>
      {!items && (
        <p className="muted">
          <Spin />
          Loading…
        </p>
      )}
      {items?.some((i) => i.kind === "claudeMd") && (
        <div className="row">
          <div>
            <b>CLAUDE.md</b>
          </div>
        </div>
      )}
      {items && (
        <div className="row">
          <div>
            <b>Skills</b> {n("skill")} · <b>Agents</b> {n("agent")} · <b>Commands</b> {n("command")}
          </div>
          <div className="acts">
            <a className="b" href={link(c, "edits")}>
              Local edits ({edits?.length ?? 0})
            </a>
          </div>
        </div>
      )}
      {plugins > 0 && (
        <div className="row">
          <div>
            <b>Plugins</b> {plugins}{" "}
            {g.pluginProblems.length > 0 && (
              <Chip kind="warn">{g.pluginProblems.length} failed</Chip>
            )}
          </div>
          <div className="acts">
            {g.pluginProblems.length > 0 && (
              <button
                type="button"
                className="b"
                onClick={() => void call({ kind: "pluginsRetry", data: { repo: g.repo } })}
              >
                Retry
              </button>
            )}
          </div>
        </div>
      )}
      {n("jobs") > 0 && (
        <div className="row">
          <div>
            <b>Jobs</b> {c.snap.jobs.filter((j) => j.repo === g.repo).length}
          </div>
          <div className="acts">
            <a className="b" href={link(c, "jobs")}>
              Open
            </a>
          </div>
        </div>
      )}
      {settings && (
        <div className="row">
          <div>
            <b>Settings</b> {settings.name}
          </div>
        </div>
      )}
    </div>
  );
}

/** v2-ScreenGroupCopy: CLAUDE.md lines to another group, shown as the diff they would add. */
export function Copy(c: Ctx) {
  const { g, call } = c;
  const targets = c.snap.groups.filter((x) => x.repo !== g.repo && !x.off);
  const [to, setTo] = useState(targets[0]?.repo ?? "");
  const [pv] = useLoad(
    () =>
      to
        ? data(call, { kind: "copyPreview", data: { from: g.repo, to } }, "copyPreview")
        : Promise.resolve(undefined),
    to,
  );
  const [err, setErr] = useState<string | null>(null);
  const lines = pv?.claudeMd.split("\n").filter((l) => l.trim()) ?? [];
  const send = async () => {
    const r = await call({
      kind: "copy",
      data: { from: g.repo, to, req: { claudeMd: lines.join("\n"), skills: [], agents: [] } },
    });
    if ("err" in r) return setErr(r.err);
    c.toast(
      r.ev.kind === "copied" && r.ev.data.proposed !== null
        ? `Proposed as #${r.ev.data.proposed} in ${to}`
        : `Copied to ${to}`,
    );
    go(`group/${g.repo}/contents`);
  };
  return (
    <div className="card">
      <h2>Copy CLAUDE.md lines to another group</h2>
      <label className="fld">
        <span>To group</span>
        <select className="in" value={to} onChange={(e) => setTo(e.target.value)}>
          {targets.map((t) => (
            <option key={t.repo}>{t.repo}</option>
          ))}
        </select>
      </label>
      <div className="diff">
        {lines.map((l, i) => (
          <div className="add" key={i}>
            + {l}
          </div>
        ))}
      </div>
      {err && <div className="err">{err}</div>}
      <div className="acts">
        <button
          type="button"
          className="b p"
          disabled={!pv?.canPush || lines.length === 0}
          onClick={send}
        >
          Copy
        </button>
        <a className="b" href={link(c, "edits")}>
          Cancel
        </a>
      </div>
    </div>
  );
}
