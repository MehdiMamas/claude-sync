import { useState } from "react";
import type { ClashView } from "../generated/ClashView";
import type { ReviewChoice } from "../generated/ReviewChoice";
import type { ReviewLine } from "../generated/ReviewLine";
import { ago } from "../time";
import { Banner, Spin, useLoad } from "../ui";
import { type Ctx, data } from "./Group";

const choices: [ReviewChoice, string][] = [
  ["reviewFirst", "Review first"],
  ["followGroup", "Follow group"],
  ["autoAccept", "Auto-accept"],
];
const n = (k: number) => `${k} ${k === 1 ? "line" : "lines"}`;
const count = (items: ReviewLine[]) => items.reduce((a, i) => a + i.lines.length, 0);
const mark = (i: ReviewLine) => (i.add ? "+" : "-");
const asText = (items: ReviewLine[]) =>
  items.flatMap((i) => i.lines.map((l) => `${mark(i)} ${l}`)).join("\n");

/** Pending lines of one commit author and time, as one card (v2-ScreenGroupReview). */
const cards = (items: ReviewLine[]) => {
  const out: ReviewLine[][] = [];
  for (const i of items) {
    const last = out[out.length - 1];
    if (last?.[0]?.author === i.author && last[0].at === i.at) last.push(i);
    else out.push([i]);
  }
  return out;
};

/** v2-ScreenGroupReview, ReviewLines, ReviewClash, ReviewDeclined, ReviewDone and ReviewGuest. */
export function Review(c: Ctx) {
  const { g, call } = c;
  const key = `${g.repo}:${g.pendingReview}`;
  const [items, reload] = useLoad(
    () => data(call, { kind: "reviewList", data: { repo: g.repo } }, "review"),
    key,
  );
  const [all, reloadClashes] = useLoad(() => data(call, { kind: "clashes" }, "clashes"), key);
  const [lines, setLines] = useState(false);
  const [pick, setPick] = useState<Record<string, boolean>>({});
  const [done, setDone] = useState<{ yes: number; no: number } | null>(null);
  const [said, setSaid] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [gone, setGone] = useState<string[]>([]);

  const mine = (all ?? []).filter(
    (x) => x.a.source === null && x.b.source === g.repo && !gone.includes(x.b.text),
  );
  const decide = async (list: ReviewLine[], accept: boolean[]) => {
    for (const yes of [true, false]) {
      const ids = list.filter((_, i) => accept[i] === yes).map((i) => i.id);
      if (ids.length === 0) continue;
      const r = await call({ kind: "reviewDecide", data: { repo: g.repo, ids, accept: yes } });
      if ("err" in r) return c.toast(r.err);
    }
    const yes = count(list.filter((_, i) => accept[i]));
    const no = count(list) - yes;
    setLines(false);
    setDone({ yes, no });
    c.toast([yes && `${n(yes)} accepted`, no && `${n(no)} declined`].filter(Boolean).join(", "));
    reload();
  };
  const explain = async (id: string, list: ReviewLine[]) => {
    setBusy(id);
    const r = await call({ kind: "explain", data: { task: "claudeMdReview", text: asText(list) } });
    setBusy(null);
    if ("err" in r) return c.toast(r.err);
    if (r.ev.kind === "text") setSaid((s) => ({ ...s, [id]: (r.ev as { data: string }).data }));
  };
  const resolve = async (x: ClashView, keepMine: boolean) => {
    const r = await call(
      keepMine
        ? { kind: "dropLine", data: { repo: g.repo, line: x.b.text } }
        : { kind: "useGroupVersion", data: { repo: g.repo } },
    );
    if ("err" in r) return c.toast(r.err);
    setGone((l) => [...l, x.b.text]);
    reloadClashes();
    reload();
  };

  if (!items) return <p className="muted">Loading…</p>;

  if (mine.length > 0)
    return (
      <>
        <Banner kind="warn">
          {n(mine.length)} {mine.length === 1 ? "clashes" : "clash"} with your own CLAUDE.md.
        </Banner>
        {mine.map((x) => (
          <div className="card" key={`${x.a.text}|${x.b.text}`}>
            <h2>Clash</h2>
            <p style={{ margin: "0 0 4px" }}>
              Group: <span className="mono">{x.b.text}</span>
            </p>
            <p style={{ margin: 0 }}>
              Yours: <span className="mono">{x.a.text}</span>
            </p>
            <div className="acts" style={{ marginTop: 10 }}>
              <button type="button" className="b p" onClick={() => resolve(x, true)}>
                Keep mine
              </button>
              <button type="button" className="b" onClick={() => resolve(x, false)}>
                Use the group line
              </button>
              <button
                type="button"
                className="b"
                onClick={() =>
                  c.toast("Open ~/.claude/CLAUDE.md in your editor and change your line.")
                }
              >
                Edit line
              </button>
            </div>
          </div>
        ))}
      </>
    );

  if (items.length === 0) {
    const declined = done !== null && done.yes === 0 && done.no > 0;
    return (
      <div className="card">
        <div style={{ textAlign: "center", padding: "36px 0" }}>
          <h2>Nothing to review</h2>
          <p className="muted">
            {declined
              ? "The declined lines stay out of ~/.claude/CLAUDE.md. A later change to them asks again."
              : "Lines you accepted are in ~/.claude/CLAUDE.md. Declined lines stay out."}
          </p>
          <a className="b" href={`#/group/${g.repo}`}>
            Back to overview
          </a>
        </div>
      </div>
    );
  }

  return (
    <>
      {cards(items).map((list) => {
        const first = list[0] as ReviewLine;
        const id = list.map((i) => i.id).join(",");
        const picked = list.map((i) => pick[i.id] ?? i.add);
        return (
          <div className="card" key={id}>
            <h2>
              CLAUDE.md from {first.author} · {ago(first.at, c.now)}
            </h2>
            {lines ? (
              <>
                {list.map((i, k) => (
                  <div className="row" key={i.id}>
                    <div>
                      <button
                        type="button"
                        className={`chip chipb ${picked[k] ? "ok" : "n"}`}
                        aria-pressed={picked[k]}
                        onClick={() => setPick((p) => ({ ...p, [i.id]: !picked[k] }))}
                      >
                        {picked[k] ? "Accept" : "Decline"}
                      </button>{" "}
                      <span className="mono">
                        {i.lines.map((l) => `${mark(i)} ${l}`).join(" ")}
                      </span>
                    </div>
                  </div>
                ))}
                <div className="acts">
                  <button type="button" className="b p" onClick={() => decide(list, picked)}>
                    Apply my choices
                  </button>
                  <button type="button" className="b" onClick={() => setLines(false)}>
                    Back
                  </button>
                </div>
              </>
            ) : (
              <>
                <div className="diff">
                  {list.flatMap((i) =>
                    i.lines.map((l) => (
                      <div className={i.add ? "add" : "del"} key={`${i.id}${l}`}>
                        {mark(i)} {l}
                      </div>
                    )),
                  )}
                </div>
                {said[id] && (
                  <p className="sm muted" style={{ whiteSpace: "pre-wrap" }}>
                    {said[id]}
                  </p>
                )}
                <div className="acts">
                  <button
                    type="button"
                    className="b p"
                    onClick={() =>
                      decide(
                        list,
                        list.map(() => true),
                      )
                    }
                  >
                    {list.length === 1 ? "Accept" : "Accept all"}
                  </button>
                  {list.length > 1 && (
                    <button type="button" className="b" onClick={() => setLines(true)}>
                      Choose lines
                    </button>
                  )}
                  <button
                    type="button"
                    className="b"
                    onClick={() =>
                      decide(
                        list,
                        list.map(() => false),
                      )
                    }
                  >
                    Decline
                  </button>
                  <button
                    type="button"
                    className="b g"
                    disabled={busy === id}
                    onClick={() => explain(id, list)}
                  >
                    {busy === id && <Spin />}
                    Explain
                  </button>
                </div>
              </>
            )}
          </div>
        );
      })}
      {!lines && (
        <div className="card">
          <h2>Your choice for this group</h2>
          <div className="row">
            <div>
              {choices.map(([v, label]) => (
                <span key={v}>
                  <button
                    type="button"
                    className={`chip chipb${g.reviewChoice === v ? " ok" : ""}`}
                    aria-pressed={g.reviewChoice === v}
                    onClick={() =>
                      void call({ kind: "setReviewChoice", data: { repo: g.repo, choice: v } })
                    }
                  >
                    {v === "followGroup"
                      ? `${label} (${g.autoAcceptClaudeMd ? "accepts without review" : "review first"})`
                      : label}
                  </button>{" "}
                </span>
              ))}
            </div>
          </div>
          <p className="sm muted">
            The group default is set by the host (
            {g.autoAcceptClaudeMd ? "on: accepted without review" : "off: review first"}). Your
            choice follows you to every machine.
          </p>
        </div>
      )}
    </>
  );
}
