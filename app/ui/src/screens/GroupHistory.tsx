import { useState } from "react";
import type { HistoryItem } from "../generated/HistoryItem";
import { useBackend } from "../store";
import { ago } from "../time";
import { Banner, Chip, go, Spin, useLoad } from "../ui";
import { type Ctx, data } from "./Group";

/** A commit's first line without the `login@device: ` the app adds and the `#12` chips shown apart. */
export const titleOf = (summary: string) =>
  summary
    .replace(/^\S+@\S+: /, "")
    .replace(/\s*\(?#\d+\)?/g, "")
    .trim();

const at = (iso: string | null, now: number) => {
  const t = iso ? Date.parse(iso) / 1000 : Number.NaN;
  return Number.isNaN(t) ? "" : ago(t, now);
};

/** The engine's before/now change notes (Parity 35). */
export const Notes = ({ h }: { h: HistoryItem }) =>
  h.before || h.now ? (
    <div className="sm">
      {h.before && (
        <>
          <b>Before:</b> {h.before}{" "}
        </>
      )}
      {h.now && (
        <>
          <b>Now:</b> {h.now}
        </>
      )}
    </div>
  ) : null;

const first = 3;
const more = 20;
const unsigned = "Claude Code is not signed in on this machine, so it cannot explain changes.";

/**
 * v2-ScreenGroupHistory, NoHistory, Revert, Helper and HelperUnavailable. The route after
 * `history` is `explain/<sha>` or `revert/<sha>`.
 */
export function History(c: Ctx) {
  const { g, call, snap } = c;
  const [limit, setLimit] = useState(first);
  const [list] = useLoad(
    () => data(call, { kind: "history", data: { repo: g.repo, limit } }, "history"),
    `${g.repo}${limit}${g.lastSync}`,
  );
  const [verb, sha] = [c.sub[1], c.sub[2]];
  const item = list?.find((h) => h.sha === sha);
  if (verb === "revert") return <Revert {...c} item={item} sha={sha ?? ""} />;
  if (verb === "explain") return <Explain {...c} item={item} list={list} />;
  return (
    <List
      {...c}
      list={list}
      limit={limit}
      more={() => setLimit(more)}
      banner={null}
      me={snap.login}
    />
  );
}

function List(
  c: Ctx & {
    list: HistoryItem[] | undefined;
    limit: number;
    more: () => void;
    banner: string | null;
    me: string | null;
  },
) {
  const { g, list, me } = c;
  const backend = useBackend();
  if (!list) return <p className="muted">Loading…</p>;
  if (list.length === 0)
    return (
      <div className="card">
        <div style={{ textAlign: "center", padding: "36px 0" }}>
          <h2>No changes yet</h2>
          <p className="muted">Changes appear here once someone shares a file.</p>
          <a className="b" href={`#/group/${g.repo}/edits`}>
            Share a file
          </a>
        </div>
      </div>
    );
  return (
    <>
      {c.banner && (
        <Banner
          kind="warn"
          actions={
            <a className="b" href={`#/group/${g.repo}/history`}>
              Close
            </a>
          }
        >
          {c.banner}
        </Banner>
      )}
      <div className="card">
        <h2>Changes</h2>
        {list.map((h, i) => {
          // the oldest commit has no parent to go back to
          const root = !c.banner && i === list.length - 1 && list.length < c.limit;
          const mine = h.author === me;
          return (
            <div className="row" key={h.sha}>
              <div>
                <b>{titleOf(h.summary)}</b>{" "}
                {h.issues.map((n) => (
                  <Chip key={n}>#{n}</Chip>
                ))}
                <div className="sm muted">
                  {[mine ? "you" : h.author, at(h.at, c.now)].filter(Boolean).join(" · ")}
                </div>
                <Notes h={h} />
              </div>
              <div className="acts">
                {!mine && !c.banner && (
                  <a className="b g" href={`#/group/${g.repo}/history/explain/${h.sha}`}>
                    Explain
                  </a>
                )}
                {h.issues.length > 0 && (
                  <button
                    type="button"
                    className="b g"
                    onClick={() =>
                      void backend.openTarget(`https://github.com/${g.repo}/commit/${h.sha}`)
                    }
                  >
                    View on GitHub
                  </button>
                )}
                {g.isHost && !root && (
                  <a className="b" href={`#/group/${g.repo}/history/revert/${h.sha}`}>
                    Revert
                  </a>
                )}
              </div>
            </div>
          );
        })}
        {list.length >= c.limit && c.limit < more && (
          <div className="acts" style={{ marginTop: 8 }}>
            <button type="button" className="b" onClick={c.more}>
              Show more
            </button>
          </div>
        )}
      </div>
    </>
  );
}

/** The helper: what a commit changed, in words, from the local `claude` (no API key). */
function Explain(c: Ctx & { item: HistoryItem | undefined; list: HistoryItem[] | undefined }) {
  const { item, call } = c;
  const [res] = useLoad(async () => {
    if (!item) return undefined;
    const text = [
      `${item.author}: ${titleOf(item.summary)}`,
      item.before && `Before: ${item.before}`,
      item.now && `Now: ${item.now}`,
    ]
      .filter(Boolean)
      .join("\n");
    const r = await call({ kind: "explain", data: { task: "activity", text } });
    if ("err" in r) return { err: r.err };
    return r.ev.kind === "text" ? { text: r.ev.data } : undefined;
  }, item?.sha ?? "");
  if (!c.list) return <p className="muted">Loading…</p>;
  if (!item)
    return (
      <p className="muted">
        That change is not in the list. <a href={`#/group/${c.g.repo}/history`}>Back to History</a>
      </p>
    );
  if (!res)
    return (
      <p className="muted">
        <Spin />
        Asking Claude…
      </p>
    );
  if (res.err !== undefined)
    return (
      <List
        {...c}
        list={c.list.filter((h) => h.sha === item.sha)}
        limit={first}
        more={() => {}}
        me={c.snap.login}
        banner={/not signed in/i.test(res.err) ? unsigned : res.err}
      />
    );
  return (
    <div className="card">
      <h2>What changed (from your Claude login)</h2>
      <p style={{ margin: "0 0 8px", whiteSpace: "pre-wrap" }}>{res.text}</p>
      <p className="sm muted">Made by the claude CLI on this machine. No API key is used.</p>
      <a className="b" href={`#/group/${c.g.repo}/history`}>
        Close
      </a>
    </div>
  );
}

/** v2-ScreenGroupRevert: what the revert would undo, then a new commit. */
function Revert(c: Ctx & { item: HistoryItem | undefined; sha: string }) {
  const { g, call } = c;
  const [got] = useLoad(async () => {
    const r = await call({ kind: "revertPreview", data: { repo: g.repo, sha: c.sha } });
    return "err" in r
      ? { err: r.err }
      : { plan: r.ev.kind === "revertPlan" ? r.ev.data : undefined };
  }, c.sha);
  const plan = got && "plan" in got ? got.plan : undefined;
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const blocked = plan?.files.find((f) => f.blocked)?.blocked ?? null;
  const send = async () => {
    setBusy(true);
    const r = await call({ kind: "revert", data: { repo: g.repo, sha: c.sha } });
    setBusy(false);
    if ("err" in r) return setErr(r.err);
    c.toast("Reverted. Members get it on their next sync.");
    go(`group/${g.repo}/history`);
  };
  const mark = { delete: "-", recreate: "+", restore: "~" } as Record<string, string>;
  return (
    <div className="dlg" style={{ position: "fixed" }}>
      <div className="card" role="dialog" aria-label="Revert">
        {!got && (
          <p className="muted">
            <Spin />
            Checking what it changed…
          </p>
        )}
        {plan && (
          <>
            <h2>Revert "{titleOf(plan.summary)}"?</h2>
            <div className="diff">
              {plan.files.map((f) => (
                <div className={f.action === "delete" ? "del" : "add"} key={f.path}>
                  {mark[f.action] ?? "~"} {f.path}
                </div>
              ))}
            </div>
            <p className="muted">
              Makes a new commit that undoes it. Members get it on their next sync.
            </p>
          </>
        )}
        {(err ?? (got && "err" in got ? got.err : null) ?? blocked) && (
          <div className="err">{err ?? (got && "err" in got ? got.err : null) ?? blocked}</div>
        )}
        <div className="acts">
          <button
            type="button"
            className="b dp"
            disabled={!plan || busy || !!blocked}
            onClick={send}
          >
            Revert
          </button>
          <a className="b" href={`#/group/${g.repo}/history`}>
            Cancel
          </a>
        </div>
      </div>
    </div>
  );
}
