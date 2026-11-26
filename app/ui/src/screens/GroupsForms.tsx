import { useState } from "react";
import type { Snapshot } from "../generated/Snapshot";
import { Banner } from "../ui";

/** GitHub repo name rules, checked before any call (ScenarioGroupHappy step 1). */
export function nameProblem(name: string): string | null {
  if (!/^[A-Za-z0-9._-]{1,100}$/.test(name) || name === "." || name === "..")
    return "Use letters, numbers, dots, dashes and underscores only.";
  return null;
}

/** v2-ScreenGroupsCreate and ScreenGroupsCreateError. */
export function NewGroup(p: { login: string; create: (name: string) => Promise<string | null> }) {
  const [name, setName] = useState("claude-group");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const local = nameProblem(name);
    if (local) return setErr(local);
    setBusy(true);
    setErr(await p.create(name));
    setBusy(false);
  };
  return (
    <>
      <h1>New group</h1>
      <p className="sub">Creates a private GitHub repo under your account. You are its host.</p>
      <form className="card" onSubmit={submit}>
        <label className="fld">
          <span>Name</span>
          <input
            className={`in${err ? " bad" : ""}`}
            value={name}
            onChange={(e) => setName(e.target.value)}
            aria-invalid={err ? true : undefined}
            aria-describedby={err ? "name-err" : undefined}
          />
          {err && (
            <div className="err" id="name-err">
              {err}
            </div>
          )}
        </label>
        <label className="fld">
          <span>Owner</span>
          <input className="in" value={`${p.login} (you)`} readOnly />
        </label>
        {!err && (
          <p className="sm muted">
            Members get your shared settings, CLAUDE.md, skills, agents and plugins. Nothing is
            shared until you choose it.
          </p>
        )}
        <div className="acts">
          <button type="submit" className="b p" disabled={busy}>
            Create group
          </button>
          <a className="b" href="#/groups">
            Cancel
          </a>
        </div>
      </form>
    </>
  );
}

/** v2-ScreenGroupsJoin. */
export function Join(p: {
  snap: Snapshot;
  accept: (id: number, repo: string) => void;
  decline: (id: number) => void;
  join: (repo: string) => void;
}) {
  const { invitations, discoverable } = p.snap;
  return (
    <>
      <h1>Join a group</h1>
      <p className="sub">Invitations from GitHub and groups you can already read.</p>
      <Banner kind="warn">
        Plugins in this group run code on every member's machine; only join groups whose members you
        trust.
      </Banner>
      {invitations.length > 0 && (
        <div className="card">
          <h2>Invitations</h2>
          {invitations.map((v) => (
            <div className="row" key={v.id}>
              <div>
                <b>{v.repo}</b>
                <div className="sm muted">from {v.inviter}</div>
              </div>
              <div className="acts">
                <button type="button" className="b p" onClick={() => p.accept(v.id, v.repo)}>
                  Accept
                </button>
                <button type="button" className="b" onClick={() => p.decline(v.id)}>
                  Decline
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
      <div className="card">
        <h2>You can join</h2>
        {discoverable.length === 0 && <p className="sm muted">No other groups found.</p>}
        {discoverable.map((r, i) => (
          <div className="row" key={r}>
            <div>
              <b>{r}</b>
              <div className="sm muted">you have read access</div>
            </div>
            <div className="acts">
              <button type="button" className={i === 0 ? "b p" : "b"} onClick={() => p.join(r)}>
                Join
              </button>
            </div>
          </div>
        ))}
      </div>
      <a className="b g" href="#/groups">
        Back to groups
      </a>
    </>
  );
}
