import type { GroupView } from "../generated/GroupView";
import type { Snapshot } from "../generated/Snapshot";
import { ago, clock } from "../time";
import { Banner, Chip, Spin } from "../ui";

export type Flash = {
  toast: string | null;
  joining: string | null;
  joinError: string | null;
  fresh: { repos: string[]; files: number } | null;
};

type Props = {
  snap: Snapshot;
  now: number;
  flash: Flash;
  act: {
    syncNow: () => void;
    retryPlugins: (repo: string) => void;
    setOff: (repo: string, off: boolean) => void;
    allOn: () => void;
    move: (repo: string, dir: -1 | 1) => void;
    accept: (id: number, repo: string) => void;
    decline: (id: number) => void;
  };
};

const retryText = (secs: number) => (secs <= 90 ? "1 minute" : `${Math.round(secs / 60)} minutes`);

function sub(g: GroupView, p: Props) {
  const t = (n: number | null) => (n === null ? "" : ago(n, p.now));
  const fresh = p.flash.fresh?.repos.includes(g.repo)
    ? ` · ${p.flash.fresh.files} files updated`
    : "";
  switch (g.status) {
    case "off":
      return "Turned off on all your machines";
    case "syncing":
      return (
        <>
          <Spin />
          Checking GitHub…
        </>
      );
    case "rateLimited":
      return `Waiting for GitHub until ${p.snap.rateLimitedUntil ? clock(p.snap.rateLimitedUntil) : "later"}`;
    case "offline":
      return g.lastSync === null ? "Offline" : `Offline · last sync ${t(g.lastSync)}`;
    case "failed":
      return g.lastSync === null ? "Last sync failed" : `Last sync failed ${t(g.lastSync)}`;
    default:
      return `Synced ${t(g.lastSync)}${fresh} · ${g.members.length} members`;
  }
}

/** v2-ScreenGroups, plus its Syncing, Synced, Reordered, Offline, RateLimited, SyncError, Plugins,
 * AllOff, Empty, Loading, Accepting and JoinError states. */
export function Groups(p: Props) {
  const { snap, flash, act } = p;
  const gs = snap.groups;
  const allOff = gs.length > 0 && gs.every((g) => g.status === "off");
  const loading = gs.length === 0 && snap.syncing;
  const failed = gs.find((g) => g.status === "failed");
  const offline = gs.some((g) => g.status === "offline");
  const limited = snap.rateLimitedUntil !== null;
  const withPlugins = gs.find((g) => g.pluginProblems.length > 0);
  const problems = gs.flatMap((g) => g.pluginProblems);
  const nonOff = gs.filter((g) => g.status !== "off");
  const invitations = snap.invitations.filter((v) => v.repo !== flash.joining);

  return (
    <>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start" }}>
        <div>
          <h1>Groups</h1>
          <p className="sub">Order decides which group wins when two set the same thing.</p>
        </div>
        <div className="acts">
          {snap.syncing && !loading ? (
            <span className="b">
              <Spin />
              Syncing…
            </span>
          ) : (
            <>
              <button type="button" className="b" onClick={act.syncNow}>
                Sync now
              </button>
              {!limited && (
                <a className="b" href="#/groups/join">
                  Join
                </a>
              )}
              <a className="b p" href="#/groups/new">
                New group
              </a>
            </>
          )}
        </div>
      </div>

      {flash.joining && (
        <Banner
          kind="info"
          actions={
            <a className="b" href={`#/group/${flash.joining}`}>
              Open group
            </a>
          }
        >
          <Spin />
          Joining {flash.joining} and placing its files…
        </Banner>
      )}
      {flash.joinError && (
        <Banner
          kind="err"
          actions={
            <a className="b" href="#/groups/join">
              Details
            </a>
          }
        >
          {flash.joinError}
        </Banner>
      )}
      {limited && (
        <Banner kind="warn">
          GitHub asked us to slow down. Syncing waits until {clock(snap.rateLimitedUntil ?? 0)} and
          starts again by itself.
        </Banner>
      )}
      {failed && (
        <Banner
          kind="err"
          actions={
            <>
              <button type="button" className="b" onClick={act.syncNow}>
                Try now
              </button>
              <a className="b" href="#/feedback">
                Report
              </a>
            </>
          }
        >
          {failed.repo} did not sync: {(failed.error ?? "unknown error").replace(/\.$/, "")}. It
          tries again in {retryText(snap.intervalSecs)}.
        </Banner>
      )}
      {offline && !failed && (
        <Banner
          kind="warn"
          actions={
            <button type="button" className="b" onClick={act.syncNow}>
              Try now
            </button>
          }
        >
          You are offline. Your files keep working; syncing starts again when the connection is
          back.
        </Banner>
      )}
      {withPlugins && (
        <Banner
          kind="warn"
          actions={
            <>
              <button
                type="button"
                className="b"
                onClick={() => act.retryPlugins(withPlugins.repo)}
              >
                Retry
              </button>
              <a className="b" href={`#/group/${withPlugins.repo}/contents`}>
                Details
              </a>
            </>
          }
        >
          {problems.length} group plugin {problems.length === 1 ? "problem" : "problems"}:{" "}
          {problems.join(" ")}
        </Banner>
      )}
      {allOff && (
        <Banner
          kind="info"
          actions={
            <button type="button" className="b p" onClick={act.allOn}>
              Turn all on
            </button>
          }
        >
          All groups are off on all your machines. Their files left ~/.claude; your own files
          stayed.
        </Banner>
      )}

      {loading && (
        <div className="card">
          <p className="muted">
            <Spin />
            Loading your groups…
          </p>
        </div>
      )}
      {!loading && gs.length === 0 && (
        <div className="card">
          <div style={{ textAlign: "center", padding: "40px 0" }}>
            <h2>No groups yet</h2>
            <p className="muted">
              Start one and invite friends, or join a group you were invited to.
            </p>
            <div className="acts" style={{ justifyContent: "center" }}>
              <a className="b p" href="#/groups/new">
                New group
              </a>
              <a className="b" href="#/groups/join">
                Join a group
              </a>
            </div>
          </div>
        </div>
      )}
      {gs.length > 0 && (
        <div className="card">
          {gs.map((g) => {
            const i = nonOff.indexOf(g);
            const off = g.status === "off";
            return (
              <div className="row" key={g.repo}>
                <div>
                  {off ? (
                    <b>{g.repo}</b>
                  ) : (
                    <a href={`#/group/${g.repo}`}>
                      <b>{g.repo}</b>
                    </a>
                  )}{" "}
                  {off ? (
                    <Chip kind="n">Off</Chip>
                  ) : g.isHost ? (
                    <Chip>Host</Chip>
                  ) : (
                    <Chip kind="n">Member</Chip>
                  )}
                  {!(off && allOff) && <div className="sm muted">{sub(g, p)}</div>}
                </div>
                <div className="acts">
                  {off ? (
                    <button type="button" className="b" onClick={() => act.setOff(g.repo, false)}>
                      Turn on
                    </button>
                  ) : (
                    <>
                      {g.pendingReview > 0 && <Chip kind="warn">{g.pendingReview} to review</Chip>}
                      {i < nonOff.length - 1 && (
                        <button type="button" className="b g" onClick={() => act.move(g.repo, 1)}>
                          Move down
                        </button>
                      )}
                      {i > 0 && (
                        <button type="button" className="b g" onClick={() => act.move(g.repo, -1)}>
                          Move up
                        </button>
                      )}
                    </>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}
      {snap.syncing && gs.length > 0 && (
        <p className="sm muted">Unchanged groups cost no GitHub requests.</p>
      )}

      {invitations.length > 0 && (
        <div className="card">
          <h2>Invitations</h2>
          {invitations.map((v) => (
            <div className="row" key={v.id}>
              <div>
                <b>{v.repo}</b>
                <div className="sm muted">from {v.inviter}</div>
                <div className="sm muted">
                  Plugins in this group run code on every member's machine; only join groups whose
                  members you trust.
                </div>
              </div>
              <div className="acts">
                <button type="button" className="b p" onClick={() => act.accept(v.id, v.repo)}>
                  Accept
                </button>
                <button type="button" className="b" onClick={() => act.decline(v.id)}>
                  Decline
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </>
  );
}
