import { type ComponentType, type ReactNode, useState } from "react";
import type { Command } from "../generated/Command";
import type { Event } from "../generated/Event";
import type { GroupView } from "../generated/GroupView";
import type { Snapshot } from "../generated/Snapshot";
import { ago } from "../time";
import { Banner, Chip, go, Spin, useLoad } from "../ui";
import { Contents, Copy, Edits, Share } from "./GroupEdits";
import { History, Notes, titleOf } from "./GroupHistory";
import { JobAccept, Jobs } from "./GroupJobs";
import { Members } from "./GroupMembers";
import { Proposals, Propose } from "./GroupProposals";
import { Review } from "./GroupReview";
import type { Flash } from "./Groups";
import { Later } from "./Shell";

export type Call = (cmd: Command) => Promise<{ ev: Event } | { err: string }>;

/** What every page of a group gets. `sub` is the route after `group/<owner>/<name>/`. */
export type Ctx = {
  snap: Snapshot;
  g: GroupView;
  now: number;
  flash: Flash;
  call: Call;
  toast: (text: string) => void;
  syncNow: () => void;
  sub: string[];
};

/** Runs a command and returns the data of the event it answers with (`undefined` on any failure). */
export async function data<K extends Event["kind"]>(
  call: Call,
  cmd: Command,
  kind: K,
): Promise<Extract<Event, { kind: K }> extends { data: infer D } ? D : never | undefined> {
  const r = await call(cmd);
  // biome-ignore lint/suspicious/noExplicitAny: the kind check above narrows it
  return ("ev" in r && r.ev.kind === kind ? (r.ev as any).data : undefined) as never;
}

/**
 * The tabs of a group page, in order. A later part adds its tab by putting its page in `pages`
 * (keyed by the first route segment after the group) and, if one tab owns several pages, its
 * highlighted tab in `tabOf`.
 */
const tabs = [
  ["", "Overview"],
  ["review", "Review"],
  ["contents", "Contents"],
  ["members", "Members"],
  ["history", "History"],
] as const;

export const pages: Record<string, ComponentType<Ctx>> = {
  "": Overview,
  contents: Contents,
  edits: Edits,
  share: Share,
  copy: Copy,
  review: Review,
  members: Members,
  proposals: Proposals,
  propose: Propose,
  history: History,
  jobs: JobsPage,
};
export const tabOf: Record<string, string> = {
  edits: "contents",
  share: "contents",
  copy: "contents",
  proposals: "",
  propose: "",
  jobs: "contents",
};

/** `jobs` is the list; `jobs/<id>` is one job waiting for its yes. */
function JobsPage(c: Ctx) {
  return c.sub[1] ? <JobAccept {...c} /> : <Jobs {...c} />;
}

const chip = (g: GroupView): [string, "ok" | "warn" | "err" | "n"] => {
  switch (g.status) {
    case "synced":
      return ["Synced", "ok"];
    case "syncing":
      return ["Syncing", "n"];
    case "off":
      return ["Off", "n"];
    case "failed":
      return ["Failed", "err"];
    default:
      return [g.status === "offline" ? "Offline" : "Waiting", "warn"];
  }
};

/** The group page: header, actions, tabs, and the page of the route. v2-ScreenGroup and its siblings. */
export function GroupPage(c: Omit<Ctx, "g"> & { g: GroupView | undefined; leaving: boolean }) {
  const { g } = c;
  if (!g) {
    return (
      <>
        <h1>Group not found</h1>
        <p className="sub">
          You are not in this group. <a href="#/groups">Back to Groups</a>
        </p>
      </>
    );
  }
  const created = c.flash.created?.repo === g.repo ? c.flash.created : null;
  const [label, kind] = chip(g);
  const head = (
    <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start" }}>
      <div>
        <h1>{g.repo}</h1>
        <p className="sub">
          <Chip kind={g.isHost ? "" : "n"}>{g.isHost ? "Host" : "Member"}</Chip>{" "}
          <Chip kind={kind}>{label}</Chip>{" "}
          {g.off
            ? "turned off on all your machines"
            : `${g.isHost ? `${g.members.length} ${g.members.length === 1 ? "member" : "members"}` : `host ${g.host}`} · ${created ? "just created" : g.lastSync === null ? "not synced yet" : `last sync ${ago(g.lastSync, c.now)}`}`}
        </p>
      </div>
      <div className="acts">
        {g.off ? (
          <button
            type="button"
            className="b p"
            onClick={() => void c.call({ kind: "setGroupOff", data: { repo: g.repo, off: false } })}
          >
            Turn on
          </button>
        ) : (
          <>
            {g.status === "syncing" ? (
              <span className="b">
                <Spin />
                Syncing…
              </span>
            ) : (
              <button type="button" className="b" onClick={c.syncNow}>
                Sync now
              </button>
            )}
            {g.isHost && !created && (
              <button
                type="button"
                className="b"
                onClick={() =>
                  void c.call({ kind: "setGroupOff", data: { repo: g.repo, off: true } })
                }
              >
                Turn off
              </button>
            )}
            {!g.isHost && (
              <a className="b d" href={`#/group/${g.repo}/leave`}>
                Leave
              </a>
            )}
          </>
        )}
      </div>
    </div>
  );
  if (g.off) {
    return (
      <>
        {head}
        <Banner kind="info">
          This group is off for you. Its files were removed from ~/.claude; your own files stayed.
          Members still get it.
        </Banner>
        <div className="card">
          <h2>What stays</h2>
          <p className="muted" style={{ margin: 0 }}>
            Your choice follows you to every machine. Turn it on again any time.
          </p>
        </div>
      </>
    );
  }
  const sub = c.sub[0] ?? "";
  const on = tabOf[sub] ?? sub;
  const Page = pages[sub];
  return (
    <>
      {head}
      <div className="tabs">
        {tabs.map(([id, name]) => (
          <a
            key={id}
            href={`#/group/${g.repo}${id ? `/${id}` : ""}`}
            className={on === id ? "on" : ""}
            aria-current={on === id ? "page" : undefined}
          >
            {name}
            {id === "review" && g.pendingReview > 0 && (
              <>
                {" "}
                <Chip>{g.pendingReview}</Chip>
              </>
            )}
          </a>
        ))}
      </div>
      {created && (
        <Banner kind={created.lock === "on" ? "ok" : "warn"}>
          {created.lock === "on"
            ? "Group created. The repo is private and only you can push to main."
            : "GitHub can't lock this repo on your plan; the app still only sends changes from your machine."}
        </Banner>
      )}
      {created && <Invite {...c} g={g} />}
      {!created &&
        !c.leaving &&
        (Page ? (
          <Page {...c} g={g} />
        ) : (
          <Later title={tabs.find((t) => t[0] === on)?.[1] ?? sub} part="8c" />
        ))}
      {c.leaving && (
        <div className="dlg" style={{ position: "fixed" }}>
          <div className="card" role="dialog" aria-label={`Leave ${g.repo}?`}>
            <h2>Leave {g.repo}?</h2>
            <p className="muted">
              Its files leave ~/.claude on all your machines. Your own files stay.
            </p>
            <div className="acts">
              <button
                type="button"
                className="b dp"
                onClick={async () => {
                  const r = await c.call({ kind: "leave", data: { repo: g.repo } });
                  if ("err" in r) return c.toast(r.err);
                  go("groups");
                }}
              >
                Leave
              </button>
              <a className="b" href={`#/group/${g.repo}`}>
                Cancel
              </a>
            </div>
          </div>
        </div>
      )}
    </>
  );
}

/** The invite form of a group that was just created (v2-ScreenGroupCreated). */
function Invite(c: Ctx) {
  const [login, setLogin] = useState("");
  const [err, setErr] = useState<string | null>(null);
  return (
    <form
      className="card"
      onSubmit={async (e) => {
        e.preventDefault();
        const r = await c.call({
          kind: "addMember",
          data: { repo: c.g.repo, login: login.trim() },
        });
        if ("err" in r) return setErr(r.err);
        go(`group/${c.g.repo}/members`);
      }}
    >
      <h2>Invite members</h2>
      <label className="fld">
        <span>GitHub name</span>
        <input
          className={`in${err ? " bad" : ""}`}
          value={login}
          placeholder="sam-member"
          onChange={(e) => setLogin(e.target.value)}
          aria-invalid={err ? true : undefined}
        />
        {err && <div className="err">{err}</div>}
      </label>
      <button type="submit" className="b p" disabled={!login.trim()}>
        Send invite
      </button>
    </form>
  );
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
const when = (iso: string | null, now: number) => {
  const t = iso ? Date.parse(iso) / 1000 : Number.NaN;
  return Number.isNaN(t) ? "" : ` · ${ago(t, now)}`;
};

/** v2-ScreenGroup (host) and v2-ScreenGroupGuest (member): what needs the person, then recent news. */
function Overview(c: Ctx) {
  const { g, call, snap } = c;
  const [lines] = useLoad(
    () => data(call, { kind: "reviewList", data: { repo: g.repo } }, "review"),
    `${g.repo}${g.pendingReview}`,
  );
  const [props] = useLoad(
    () => data(call, { kind: "proposals", data: { repo: g.repo } }, "proposals"),
    g.repo,
  );
  const [edits] = useLoad(
    () => data(call, { kind: "editsList", data: { repo: g.repo } }, "edits"),
    g.repo,
  );
  const [recent] = useLoad(
    () => data(call, { kind: "history", data: { repo: g.repo, limit: 2 } }, "history"),
    g.repo,
  );
  const authors = [...new Set((lines ?? []).map((l) => l.author))].join(", ");
  const others = (props ?? []).filter((p) => !p.mine && !p.approvedByMe);
  const mine = (props ?? []).filter((p) => p.mine);
  const waiting = snap.jobs.filter((j) => j.repo === g.repo && !j.accepted);
  const open = (sub: string) => `#/group/${g.repo}/${sub}`;
  const rows: { key: string; text: ReactNode; to: string; label: string; primary?: boolean }[] = [];
  if (g.pendingReview > 0)
    rows.push({
      key: "review",
      text: `${plural(g.pendingReview, "CLAUDE.md line", "CLAUDE.md lines")}${authors ? ` from ${authors}` : ""}`,
      to: open("review"),
      label: "Review",
      primary: true,
    });
  if (others.length > 0)
    rows.push({
      key: "proposals",
      text: g.isHost
        ? others.length === 1
          ? `1 proposal: "${others[0]?.title}"`
          : `${others.length} proposals`
        : `${plural(others.length, "proposal", "proposals")} by ${[...new Set(others.map((p) => p.author))].join(", ")} ${others.length === 1 ? "needs" : "need"} another member's approval`,
      to: open("proposals"),
      label: "Open",
    });
  if (edits && edits.length > 0)
    rows.push({
      key: "edits",
      text: g.isHost
        ? `${edits.length} of your local edits ${edits.length === 1 ? "is" : "are"} not shared`
        : `${edits.length} of your local edits ${edits.length === 1 ? "differs" : "differ"} from the group`,
      to: open("edits"),
      label: "Open",
    });
  if (waiting.length > 0)
    rows.push({
      key: "jobs",
      text: `${plural(waiting.length, "job waits", "jobs wait")} for your yes: ${waiting.map((j) => j.title).join(", ")}`,
      to: open("jobs"),
      label: "Open",
    });
  return (
    <>
      <div className="card">
        <h2>Needs you</h2>
        {rows.length === 0 && (
          <p className="muted" style={{ margin: 0 }}>
            Nothing needs you right now.
          </p>
        )}
        {rows.map((r) => (
          <div className="row" key={r.key}>
            <div>{r.text}</div>
            <div className="acts">
              <a className={`b${r.primary ? " p" : ""}`} href={r.to}>
                {r.label}
              </a>
            </div>
          </div>
        ))}
      </div>
      {g.isHost ? (
        <div className="card">
          <h2>Recent</h2>
          {recent?.length === 0 && (
            <p className="muted" style={{ margin: 0 }}>
              Nothing yet.
            </p>
          )}
          {(recent ?? []).map((h) => (
            <div className="row" key={h.sha}>
              <div>
                {titleOf(h.summary)}
                {when(h.at, c.now)}
                <Notes h={h} />
              </div>
              <div className="acts">
                <a
                  className="b g"
                  href={open(h.author === snap.login ? "history" : `history/explain/${h.sha}`)}
                >
                  {h.author === snap.login ? "History" : "Explain"}
                </a>
              </div>
            </div>
          ))}
        </div>
      ) : (
        <div className="card">
          <h2>Your proposals</h2>
          <div className="row">
            <div>
              {mine.length === 0 && <span className="muted">None open.</span>}
              {mine.map((p) => (
                <span key={p.number}>
                  "{p.title}" <Chip kind="warn">Open</Chip>{" "}
                </span>
              ))}
            </div>
            <div className="acts">
              <a className="b" href={open("propose")}>
                Propose a change
              </a>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
