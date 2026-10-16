// The group's "Recent changes" card: commits as an activity feed (see activity.ts).
import { api, errorText, groupKey, type CommitInfo, type GroupContents, type GroupId, type RevertPlan } from "../api";
import { collapse, groupByDay, parseCommit, timeLabel, type Activity, type FeedContext, type Kind } from "../activity";
import { avatar } from "../avatar";
import { errorBox, h, replace, spinner, toast } from "../dom";
import { store } from "../store";
import { explainButton } from "./helper";

/** Entries shown at first and added per "Show more". */
const PAGE = 8;
/** Commits fetched at first (one API call either way), then the backend's cap. */
const FIRST_FETCH = 30;
const MAX_FETCH = 100;

// 16px line icons, drawn with currentColor. Static strings, never user text.
const ICONS: Record<Kind, string> = {
  created: '<path d="M4 14V2.5M4 3h7.5l-1.5 2.5L11.5 8H4"/>',
  memberAdded: '<circle cx="6" cy="5" r="2.5"/><path d="M1.5 14c0-2.6 2-4.5 4.5-4.5s4.5 1.9 4.5 4.5M12.5 5v4M10.5 7h4"/>',
  memberRemoved: '<circle cx="6" cy="5" r="2.5"/><path d="M1.5 14c0-2.6 2-4.5 4.5-4.5s4.5 1.9 4.5 4.5M10.5 7h4"/>',
  skill: '<path d="M9 1.5 3.5 9H8l-1 5.5L12.5 7H8z"/>',
  agent: '<rect x="2.5" y="5" width="11" height="8" rx="2"/><path d="M8 2v3M6 9h0M10 9h0"/>',
  command: '<rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="m4.5 6.5 2 1.5-2 1.5M8 10h3.5"/>',
  style: '<path d="M2.5 13.5c2 0 3-1 3-2.5a1.5 1.5 0 0 0-3 0M5 10l7.5-7.5 1 1L6 11"/>',
  mod: '<path d="M6 2.5h2.5v2a1.25 1.25 0 0 0 2.5 0v-2h2.5V6h-2a1.25 1.25 0 0 0 0 2.5h2v5h-5v-2a1.25 1.25 0 0 0-2.5 0v2h-4V8.5h2a1.25 1.25 0 0 0 0-2.5h-2V2.5z"/>',
  memory: '<path d="M4 1.5h5.5L12.5 4.5v10h-8.5zM9.5 1.5v3h3M6 8h4.5M6 10.5h4.5"/>',
  settings: '<path d="M2 4.5h7M12 4.5h2M2 11.5h2M7 11.5h7"/><circle cx="10.5" cy="4.5" r="1.5"/><circle cx="5.5" cy="11.5" r="1.5"/>',
  job: '<circle cx="8" cy="8" r="6"/><path d="M8 4.5V8l2.5 1.5"/>',
  other: '<path d="M10.5 2.5 13.5 5.5 6 13H3v-3z"/>',
};

function icon(kind: Kind): HTMLElement {
  const el = h("span", { class: `act-icon k-${kind}`, "aria-hidden": "true" });
  el.innerHTML = `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">${ICONS[kind]}</svg>`;
  return el;
}

const fullTime = (unix: number) => new Date(unix * 1000).toLocaleString(undefined, { dateStyle: "full", timeStyle: "short" });

let seq = 0;

/** `expanded` holds the entries whose text is open, so "Show more" keeps them open. */
function entry(
  a: Activity,
  now: Date,
  commitUrl: (sha: string) => string,
  expanded: Set<string>,
  /** Host only: reverts the commit and reloads the feed. Null for members. */
  revert: ((sha: string) => Promise<void>) | null,
  id: GroupId,
): HTMLElement {
  const who = a.isYou ? "You" : a.login;
  const count = a.shas.length;
  const time = h("time", { datetime: new Date(a.atUnix * 1000).toISOString(), title: fullTime(a.atUnix) }, timeLabel(a.atUnix, now));
  const open = h("button", { class: "link act-link", type: "button", "aria-label": `View this change on GitHub: ${a.title}` }, "View on GitHub");
  open.addEventListener("click", () => api.openUrl(commitUrl(a.sha)).catch((e) => toast(errorText(e), "error")));

  let detail: HTMLElement | false = false;
  let more: HTMLButtonElement | false = false;
  if (a.detail) {
    const id = `act-detail-${++seq}`;
    const d = h("div", { class: "act-detail clamp", id }, a.detail);
    const btn = h("button", { class: "link act-more", type: "button", "aria-expanded": "false", "aria-controls": id, hidden: true }, "more");
    const show = (open: boolean) => {
      d.classList.toggle("clamp", !open);
      btn.setAttribute("aria-expanded", String(open));
      btn.textContent = open ? "less" : "more";
      if (open) expanded.add(a.sha);
      else expanded.delete(a.sha);
    };
    btn.addEventListener("click", () => show(btn.getAttribute("aria-expanded") !== "true"));
    // Offer "more" only when the text is cut off: compare its height with and without the clamp.
    requestAnimationFrame(() => {
      const clamped = d.getBoundingClientRect().height;
      d.classList.remove("clamp");
      const full = d.getBoundingClientRect().height;
      d.classList.add("clamp");
      if (full > clamped + 2) {
        btn.hidden = false;
        if (expanded.has(a.sha)) show(true);
      }
    });
    detail = d;
    more = btn;
  }

  // Host only, one commit per entry (a folded entry stands for several).
  const panel = h("div", { class: "revert-panel", hidden: true });
  let revertBtn: HTMLButtonElement | false = false;
  if (revert && a.shas.length === 1) {
    const btn = h("button", { class: "link act-link", type: "button", "aria-label": `Revert this change: ${a.title}` }, "Revert");
    btn.addEventListener("click", async () => {
      btn.disabled = true;
      try {
        showPlan(await api.revertPreview(id, a.sha));
      } catch (e) {
        toast(errorText(e), "error");
      } finally {
        btn.disabled = false;
      }
    });
    const close = () => {
      panel.hidden = true;
      replace(panel);
    };
    const showPlan = (plan: RevertPlan) => {
      const blocked = plan.files.filter((f) => f.blocked);
      const verb = { restore: "restored to the older version", delete: "deleted (this change added it)", recreate: "brought back (this change deleted it)" };
      const go = h("button", { type: "button", class: "danger" }, "Revert this change");
      go.disabled = blocked.length > 0 || plan.files.length === 0;
      go.addEventListener("click", async () => {
        go.disabled = true;
        go.textContent = "Reverting…";
        try {
          await revert(a.sha);
        } catch (e) {
          toast(errorText(e), "error");
          go.disabled = false;
          go.textContent = "Revert this change";
        }
      });
      panel.hidden = false;
      replace(
        panel,
        h("p", { class: "small" }, `Reverting makes a new change that undoes this one. These files will be:`),
        plan.files.length === 0
          ? h("p", { class: "muted small" }, "Nothing to revert: the files already match what they were before.")
          : h(
              "ul",
              { class: "small" },
              plan.files.map((f) =>
                h("li", null, h("code", null, f.path), ` ${verb[f.action]}`, f.blocked && h("div", { class: "error", role: "alert" }, f.blocked)),
              ),
            ),
        h("div", { class: "row" }, go, h("button", { type: "button", class: "link", onclick: close }, "Cancel")),
      );
    };
    revertBtn = btn;
  }

  return h(
    "li",
    { class: "act" },
    avatar(a.login, 28),
    h(
      "div",
      { class: "act-body" },
      h(
        "div",
        { class: "act-head" },
        icon(a.kind),
        h("span", { class: "act-title" }, a.title),
        a.chips.slice(0, 4).map((c) => h("span", { class: "chip" }, c)),
        a.chips.length > 4 && h("span", { class: "chip more-chips", title: a.chips.slice(4).join(", ") }, `+${a.chips.length - 4}`),
        count > 1 &&
          h(
            "span",
            { class: "tag", title: `${count} similar changes from ${fullTime(a.firstAtUnix)} to ${fullTime(a.atUnix)}` },
            `${count} changes`,
          ),
      ),
      detail,
      more,
      h(
        "div",
        { class: "act-meta muted small" },
        h("span", { class: "act-who" }, who),
        a.device && [h("span", { "aria-hidden": "true" }, "·"), h("span", { class: "device", title: "Device" }, a.device)],
        h("span", { "aria-hidden": "true" }, "·"),
        time,
        h("span", { "aria-hidden": "true" }, "·"),
        open,
        revertBtn && [h("span", { "aria-hidden": "true" }, "·"), revertBtn],
      ),
      panel,
    ),
  );
}

/**
 * `contents` names the group's mods and skills so hand-written messages such as
 * `setup-scout: …` are recognised; the feed works without it.
 */
export function historyCard(id: GroupId, contents: Promise<GroupContents>, isHost = false): HTMLElement {
  const body = h("div", { class: "feed-body", "aria-live": "polite", "aria-busy": "true" }, spinner("Loading recent changes"));
  const commitUrl = (sha: string) => `https://github.com/${encodeURIComponent(id.owner)}/${encodeURIComponent(id.repo)}/commit/${sha}`;
  const historyUrl = `https://github.com/${encodeURIComponent(id.owner)}/${encodeURIComponent(id.repo)}/commits`;

  let commits: CommitInfo[] = [];
  let fetched = 0;
  let shown = PAGE;
  const expanded = new Set<string>();
  let ctx: FeedContext = { me: store.auth?.login };

  const render = () => {
    body.setAttribute("aria-busy", "false");
    if (commits.length === 0) {
      replace(
        body,
        h(
          "div",
          { class: "empty" },
          h("p", null, "Nothing has changed yet."),
          h("p", { class: "muted small" }, "When someone shares a skill, changes a mod or adds a member, it shows up here."),
        ),
      );
      return;
    }
    const all = collapse(commits.map((c) => parseCommit(c, ctx)));
    const visible = all.slice(0, shown);
    const now = new Date();
    // More may exist on GitHub when the last fetch came back full.
    const moreOnServer = commits.length >= fetched && fetched < MAX_FETCH;
    const canShowMore = all.length > shown || moreOnServer;
    const showMore = h("button", { type: "button", class: "feed-more" }, "Show more");
    showMore.addEventListener("click", async () => {
      shown += PAGE;
      if (all.length < shown && moreOnServer) {
        showMore.disabled = true;
        showMore.textContent = "Loading…";
        try {
          await fetchCommits(MAX_FETCH);
        } catch (e) {
          toast(`Could not load more changes: ${errorText(e)}`, "error");
        }
      }
      render();
      // Keep the keyboard where the reader was.
      body.querySelector<HTMLButtonElement>(".feed-more")?.focus();
    });
    replace(
      body,
      groupByDay(visible, now).map((day) => {
        const hid = `day-${day.key}`;
        return h(
          "section",
          { class: "day", "aria-labelledby": hid },
          h("h3", { class: "day-label", id: hid }, day.label),
          h("ol", { class: "feed", "aria-labelledby": hid }, day.items.map((a) => entry(a, now, commitUrl, expanded, isHost ? revertAndReload : null, id))),
        );
      }),
      h(
        "div",
        { class: "feed-foot" },
        canShowMore
          ? showMore
          : all.length > PAGE && h("span", { class: "muted small" }, fetched >= MAX_FETCH ? `That's the latest ${MAX_FETCH} changes.` : "That's everything."),
        h(
          "button",
          {
            type: "button",
            class: "link small",
            onclick: () => api.openUrl(historyUrl).catch((e) => toast(errorText(e), "error")),
          },
          "Full history on GitHub",
        ),
      ),
    );
  };

  const revertAndReload = async (sha: string) => {
    await api.revertCommit(id, sha);
    toast("Reverted. The group now has the older version.");
    await load();
  };

  const fetchCommits = async (limit: number) => {
    commits = await api.recentCommits(id, limit);
    fetched = limit;
  };

  const load = async () => {
    body.setAttribute("aria-busy", "true");
    replace(body, spinner("Loading recent changes"));
    try {
      const [, c] = await Promise.all([fetchCommits(FIRST_FETCH), contents.catch(() => null)]);
      if (c) {
        ctx = {
          me: store.auth?.login,
          mods: c.marketplaces.flatMap((mp) => [mp.name, ...mp.plugins.map((p) => p.name)]),
          skills: c.skills.map((s) => s.name),
          agents: c.agents.map((s) => s.name),
          commands: c.commands.map((s) => s.name),
        };
      }
      render();
    } catch (e) {
      body.setAttribute("aria-busy", "false");
      replace(body, errorBox(`Could not load the recent changes of ${groupKey(id)}: ${errorText(e)}`, load));
    }
  };
  load();

  return h(
    "section",
    { class: "card", "aria-labelledby": "history-h" },
    h("h2", { id: "history-h" }, "Recent changes"),
    explainButton("activity", "Summarise", () =>
      commits
        .filter((c) => !store.auth?.login || c.author.toLowerCase() !== store.auth.login.toLowerCase())
        .slice(0, 30)
        .map((c) => `${c.author}: ${c.message}`)
        .join("\n\n"),
    ),
    body,
  );
}
