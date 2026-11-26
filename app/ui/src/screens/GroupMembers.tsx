import { type FormEvent, useEffect, useState } from "react";
import { Banner, Chip, go } from "../ui";
import type { Ctx } from "./Group";
import { initials } from "./Shell";

const here = (c: Ctx, sub = "") => `group/${c.g.repo}/members${sub}`;
const link = (c: Ctx, sub = "") => `#/${here(c, sub)}`;

function Dialog(c: Ctx & { title: string; text: string; ok: string; run: () => void }) {
  return (
    <div className="dlg" style={{ position: "fixed" }}>
      <div className="card" role="dialog" aria-label={c.title}>
        <h2>{c.title}</h2>
        <p className="muted">{c.text}</p>
        <div className="acts">
          <button type="button" className="b dp" onClick={c.run}>
            {c.ok}
          </button>
          <a className="b" href={link(c)}>
            Cancel
          </a>
        </div>
      </div>
    </div>
  );
}

const Toggle = ({ on, label, set }: { on: boolean; label: string; set: () => void }) => (
  <button type="button" aria-label={label} onClick={set} className="tgb">
    <span className={`tg${on ? " on" : ""}`} />
  </button>
);

/**
 * v2-ScreenGroupMembers, MembersGuest, MembersProposalMode, MemberRemove, MemberRemoved, Invited,
 * InviteCanceled, InviteError, Transfer, TransferPending, TransferExpired and Transferred.
 * The route after `members` says which dialog or notice shows: remove, transfer, requested,
 * expired or transferred, then the login.
 */
export function Members(c: Ctx) {
  const { g, call } = c;
  const [mode, who = ""] = c.sub.slice(1);
  const [login, setLogin] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [fresh, setFresh] = useState<string | null>(null);
  const transferred = mode === "transferred";
  useEffect(() => {
    if (transferred) c.toast("Host transferred");
  }, [transferred]);

  const run = async (cmd: Parameters<Ctx["call"]>[0], ok: string, to = here(c)) => {
    const r = await call(cmd);
    if ("err" in r) return c.toast(r.err);
    c.toast(ok);
    go(to);
  };
  const invite = async (e: FormEvent) => {
    e.preventDefault();
    const name = login.trim();
    const r = await call({ kind: "addMember", data: { repo: g.repo, login: name } });
    if ("err" in r) return setErr(r.err);
    setErr(null);
    setLogin("");
    setFresh(name);
    c.toast(`Invitation sent to ${name}`);
  };

  if (mode === "remove")
    return (
      <Dialog
        {...c}
        title={`Remove ${who}?`}
        text={`${who} loses access to the repo. On their next sync the group files leave their ~/.claude; their own files stay.`}
        ok="Remove"
        run={() =>
          run({ kind: "removeMember", data: { repo: g.repo, login: who } }, `${who} removed`)
        }
      />
    );
  if (mode === "transfer")
    return (
      <Dialog
        {...c}
        title={`Make ${who} the host?`}
        text={`GitHub emails ${who} a transfer request. Nothing changes until they accept; then the repo moves to them and you stay a member.`}
        ok="Send request"
        run={async () => {
          const r = await call({ kind: "transfer", data: { repo: g.repo, new_owner: who } });
          if ("err" in r) return c.toast(r.err);
          go(here(c, `/requested/${who}`));
        }}
      />
    );
  if (transferred)
    return (
      <Banner kind="ok">
        {who} is now the host. You are a member; the repo moved to {g.repo} and the app follows the
        new name.
      </Banner>
    );
  if (mode === "expired")
    return (
      <Banner
        kind="warn"
        actions={
          <>
            <a className="b" href={link(c, `/transfer/${who}`)}>
              Ask again
            </a>
            <a className="b" href={link(c)}>
              Close
            </a>
          </>
        }
      >
        {who} did not accept in time, or declined. You are still the host.
      </Banner>
    );
  if (mode === "requested")
    return (
      <>
        <Banner kind="info">
          Asked {who} to accept the group. Nothing changes until they do. GitHub keeps the request
          for 1 day.
        </Banner>
        <div className="card">
          <h2>Members</h2>
          <div className="row">
            <div>
              <b>{c.snap.login}</b> (you) · host
            </div>
          </div>
          <div className="row">
            <div>
              <b>{who}</b> · transfer requested
            </div>
          </div>
        </div>
      </>
    );

  const pending = g.pendingInvites;
  const people = [g.host, ...g.members.filter((m) => m !== g.host)];
  const settings = g.isHost ? (
    <div className="card">
      <h2>Group settings</h2>
      <div className="row">
        <div>
          Proposal mode: every change goes to a proposal pull request that another member approves
        </div>
        <div className="acts">
          <Toggle
            on={g.proposalMode}
            label="Proposal mode"
            set={() =>
              void call({ kind: "setProposalMode", data: { repo: g.repo, on: !g.proposalMode } })
            }
          />
        </div>
      </div>
      <div className="row">
        <div>Members get CLAUDE.md changes without review (group default)</div>
        <div className="acts">
          <Toggle
            on={g.autoAcceptClaudeMd}
            label="Members get CLAUDE.md changes without review"
            set={() =>
              void call({
                kind: "setAutoAccept",
                data: { repo: g.repo, on: !g.autoAcceptClaudeMd },
              })
            }
          />
        </div>
      </div>
    </div>
  ) : (
    <div className="card">
      <h2>Group settings (host only)</h2>
      <div className="row">
        <div>Proposal mode</div>
        <div className="acts">
          <span className="muted">
            {g.proposalMode
              ? "On: changes go to a proposal pull request that another member approves"
              : "Off: changes go straight to the group"}
          </span>
        </div>
      </div>
      <div className="row">
        <div>CLAUDE.md changes</div>
        <div className="acts">
          <span className="muted">
            Group default: {g.autoAcceptClaudeMd ? "accepted without review" : "review first"}
          </span>
        </div>
      </div>
    </div>
  );

  return (
    <>
      {g.isHost && g.proposalMode && (
        <Banner kind="info">
          Proposal mode is on. Your own changes also go to a proposal that another member approves.
        </Banner>
      )}
      {!err && (
        <div className="card">
          <h2>Members</h2>
          {people.map((m) => (
            <div className="row" key={m}>
              <div>
                <span className="av">{initials(m)}</span> {m}
                {m === c.snap.login && !g.isHost && " (you)"}
                {m === g.host && (
                  <>
                    {" "}
                    <Chip>Host</Chip>
                  </>
                )}
                {pending.includes(m) && (
                  <>
                    {" "}
                    <Chip kind="n">Invited</Chip>
                    {fresh === m && (
                      <div className="sm muted">listed apart until GitHub says it was accepted</div>
                    )}
                  </>
                )}
              </div>
              <div className="acts">
                {g.isHost && m !== g.host && !pending.includes(m) && (
                  <>
                    <a className="b d" href={link(c, `/remove/${m}`)}>
                      Remove
                    </a>
                    <a className="b" href={link(c, `/transfer/${m}`)}>
                      Make host
                    </a>
                  </>
                )}
                {g.isHost && pending.includes(m) && (
                  <button
                    type="button"
                    className="b"
                    onClick={() =>
                      run(
                        { kind: "removeMember", data: { repo: g.repo, login: m } },
                        `Invitation to ${m} canceled`,
                      )
                    }
                  >
                    Cancel invite
                  </button>
                )}
                {!g.isHost && m === c.snap.login && (
                  <a className="b d" href={`#/group/${g.repo}/leave`}>
                    Leave group
                  </a>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
      {err && <Banner kind="err">{err}</Banner>}
      {g.isHost && (
        <form className="card" onSubmit={invite}>
          <h2>Add member</h2>
          {err ? (
            <label className="fld">
              <span>GitHub name</span>
              <input
                className="in bad"
                value={login}
                onChange={(e) => setLogin(e.target.value)}
                aria-invalid
              />
              <div className="err">Check the spelling</div>
            </label>
          ) : (
            <div style={{ display: "flex", gap: 8 }}>
              <input
                className="in"
                style={{ flex: 1 }}
                aria-label="GitHub name"
                placeholder="GitHub name"
                value={login}
                onChange={(e) => setLogin(e.target.value)}
              />
              <button type="submit" className="b p" disabled={!login.trim()}>
                Invite
              </button>
            </div>
          )}
          {err && (
            <button type="submit" className="b p" disabled={!login.trim()}>
              Invite
            </button>
          )}
        </form>
      )}
      {!err && settings}
    </>
  );
}
