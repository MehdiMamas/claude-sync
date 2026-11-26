import { useState } from "react";
import type { Command } from "../generated/Command";
import { go, Spin, useLoad } from "../ui";
import { type Ctx, data } from "./Group";

const merged = "Approved and merged. Members get it on their next sync.";
const sent = "Approved. The host can merge it now.";
const declined = "Declined. The pull request is closed; the author keeps the change locally.";

/** v2-ScreenGroupProposals, ProposalsGuest, ProposalApproved, ProposalApprovedGuest and ProposalDeclined. */
export function Proposals(c: Ctx) {
  const { g, call, snap } = c;
  const [list, reload] = useLoad(
    () => data(call, { kind: "proposals", data: { repo: g.repo } }, "proposals"),
    g.repo,
  );
  const run = async (cmd: Command, ok: string) => {
    const r = await call(cmd);
    c.toast("err" in r ? r.err : ok);
    reload();
  };
  const who = (l: string) => (l === snap.login ? "you" : l);
  const ok = g.isHost ? merged : sent;
  if (!list) return <p className="muted">Loading…</p>;
  return (
    <>
      <div className="card">
        <h2>Proposals</h2>
        {list.length === 0 && (
          <p className="muted" style={{ margin: 0 }}>
            No open proposals.
          </p>
        )}
        {list.map((p) => {
          const files = p.files.map((f) => f.path).join(", ");
          const note = p.needsUpdate
            ? `main moved since it was opened; ${p.mine ? "you update" : `${p.author} updates`} it from the app`
            : p.mine
              ? "you cannot approve your own"
              : "";
          const sub = [files, note].filter(Boolean).join(" · ");
          return (
            <div className="row" key={p.number}>
              <div>
                <b>{p.title}</b> by {who(p.author)}{" "}
                {p.approvals.length > 0 && (
                  <span className="chip ok">Approved by {p.approvals.map(who).join(", ")}</span>
                )}
                {p.needsUpdate && <span className="chip warn">Needs an update</span>}
                {p.mine && p.approvals.length === 0 && !p.needsUpdate && (
                  <span className="chip n">Waiting for another member</span>
                )}
                {sub && <div className="sm muted">{sub}</div>}
              </div>
              <div className="acts">
                {!p.mine && !p.needsUpdate && (g.isHost || !p.approvedByMe) && (
                  <button
                    type="button"
                    className="b p"
                    onClick={() =>
                      run({ kind: "proposalApprove", data: { repo: g.repo, number: p.number } }, ok)
                    }
                  >
                    {g.isHost ? "Merge" : "Approve"}
                  </button>
                )}
                {g.isHost && !p.mine && (
                  <button
                    type="button"
                    className="b"
                    onClick={() =>
                      run(
                        { kind: "proposalDecline", data: { repo: g.repo, number: p.number } },
                        declined,
                      )
                    }
                  >
                    Decline
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>
      {g.isHost && list.some((p) => !p.mine) && (
        <div className="acts" style={{ marginTop: 12 }}>
          <button
            type="button"
            className="b"
            onClick={() => run({ kind: "proposalsAcceptAll", data: { repo: g.repo } }, merged)}
          >
            Accept all approved
          </button>
          <button
            type="button"
            className="b d"
            onClick={() => run({ kind: "proposalsDeclineAll", data: { repo: g.repo } }, declined)}
          >
            Decline all
          </button>
        </div>
      )}
    </>
  );
}

const label = (path: string) => {
  const m = path.match(/^(skills|agents|commands)\/([^/]+)/);
  return m ? `${m[1]}/${m[2]?.replace(/\.md$/, "")} (your local ${m[1]?.replace(/s$/, "")})` : path;
};

/** v2-ScreenGroupProposeForm: one local edit as a proposal for the host. */
export function Propose(c: Ctx) {
  const { g, call } = c;
  const [edits] = useLoad(
    () => data(call, { kind: "editsList", data: { repo: g.repo } }, "edits"),
    g.repo,
  );
  const [path, setPath] = useState("");
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const chosen = path || edits?.[0]?.path || "";
  const send = async () => {
    setBusy(true);
    const r = await call({ kind: "shareEdit", data: { repo: g.repo, path: chosen } });
    setBusy(false);
    if ("err" in r) return setErr(r.err);
    c.toast(
      r.ev.kind === "proposed"
        ? `Proposed as #${r.ev.data}. A member must approve it.`
        : "Shared. Members get it on their next sync.",
    );
    go(`group/${g.repo}`);
  };
  if (!edits) return <p className="muted">Loading…</p>;
  return (
    <div className="card">
      <h2>Propose a change</h2>
      {edits.length === 0 && (
        <p className="muted">You have no local edits to propose. Change a file first.</p>
      )}
      {edits.length > 0 && (
        <>
          <label className="fld">
            <span>What to propose</span>
            <select className="in" value={chosen} onChange={(e) => setPath(e.target.value)}>
              {edits.map((e) => (
                <option key={e.path} value={e.path}>
                  {label(e.path)}
                </option>
              ))}
            </select>
          </label>
          <label className="fld">
            <span>Note for the host</span>
            <textarea
              className="in"
              style={{ minHeight: 56 }}
              value={note}
              onChange={(e) => setNote(e.target.value)}
            />
          </label>
        </>
      )}
      {err && <div className="err">{err}</div>}
      <div className="acts">
        <button type="button" className="b p" disabled={busy || !chosen} onClick={send}>
          {busy && <Spin />}
          Send proposal
        </button>
        <a className="b" href={`#/group/${g.repo}`}>
          Cancel
        </a>
      </div>
    </div>
  );
}
