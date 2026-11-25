// The Optional packs card of a group (docs/architecture.md §23): extras that turn on for this
// computer only after Claude helped set them up and the pack's check passed. The words of each
// state are in `packs-copy.ts`; the screens are the blueprint canvas "Opt-in packs".
import { api, errorText, type GroupStatus, type PackView, type PacksOverview } from "../api";
import { busy, errorBox, h, replace, spinner, toast } from "../dom";
import { NEEDS_INTERNET, intro, readyLines, row, type Button } from "../packs-copy";
import { jobDetails } from "./job-text";

const LABELS: Record<Button, string> = {
  setup: "Set up with Claude",
  fix: "Fix with Claude",
  check: "Check again",
  turnOn: "Turn on",
  review: "Turn on…",
  notNow: "Not now",
  turnOff: "Turn off",
  turnOnAgain: "Turn on again",
};

/** The card, or nothing at all when the group has no packs (it removes itself). */
export function packsCard(g: GroupStatus): HTMLElement {
  const id = g.id;
  const body = h("div", null, spinner("Reading the group's packs"));
  const card = h("section", { class: "card packs", "aria-labelledby": "packs-h", hidden: true }, h("h2", { id: "packs-h" }, "Optional packs"), body);
  /** Packs whose panel the member closed with "Not now". */
  const closed = new Set<string>();
  /** Packs whose check runs from this screen right now. */
  const checking = new Set<string>();
  /** What went wrong when a terminal could not open, by pack. */
  const trouble = new Map<string, { problem: string; command: string | null }>();
  let timer: number | undefined;
  let last: PacksOverview | null = null;

  const offline = () => (g.lastError ?? "").startsWith("network error");

  const runCheck = async (p: PackView) => {
    if (checking.has(p.id)) return;
    checking.add(p.id);
    if (last) draw(last);
    try {
      await api.checkPack(id, p.id);
      closed.delete(p.id);
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      checking.delete(p.id);
      await load();
    }
  };

  const start = async (p: PackView) => {
    trouble.delete(p.id);
    const r = await api.startPackSetup(id, p.id);
    if (!r.started && r.problem) trouble.set(p.id, { problem: r.problem, command: r.command });
    await load();
  };

  const act: Record<Button, (p: PackView, btn: HTMLButtonElement) => void> = {
    setup: (p, b) => busy(b, "Opening…", () => start(p)),
    fix: (p, b) => busy(b, "Opening…", () => start(p)),
    check: (p) => void runCheck(p),
    turnOnAgain: (p) => void runCheck(p),
    turnOn: (p, b) =>
      busy(b, "Turning on…", async () => {
        const hashes = Object.fromEntries(p.jobs.map((j) => [j.id, j.hash]));
        show(await api.turnOnPack(id, p.id, hashes));
        toast(`${p.title} is on for this computer`);
      }),
    review: (p) => {
      closed.delete(p.id);
      if (last) draw(last);
    },
    notNow: (p) => {
      closed.add(p.id);
      if (last) draw(last);
    },
    turnOff: (p, b) =>
      busy(b, "Turning off…", async () => {
        show(await api.turnOffPack(id, p.id));
        toast(`${p.title} is off on this computer`);
      }),
  };

  const button = (p: PackView, kind: Button, off: boolean): HTMLButtonElement => {
    const b = h("button", { class: kind === "turnOn" ? "primary" : undefined, type: "button", "aria-label": `${LABELS[kind]}: ${p.title}` }, LABELS[kind]);
    if (off && NEEDS_INTERNET.includes(kind)) {
      b.disabled = true;
      b.title = "Claude Code needs the internet";
    }
    b.addEventListener("click", () => act[kind](p, b));
    return b;
  };

  const troubleBox = (p: PackView): HTMLElement | false => {
    const t = trouble.get(p.id);
    if (!t) return false;
    const copy = h("button", { type: "button" }, "Copy");
    const cmd = t.command;
    if (cmd) {
      copy.addEventListener("click", () => {
        navigator.clipboard.writeText(cmd).then(
          () => toast("Command copied"),
          () => toast("Could not copy; select the command and copy it", "error"),
        );
      });
    }
    return h(
      "div",
      { class: "notice", role: "status" },
      h(
        "div",
        { class: "notice-text" },
        h("strong", null, t.problem),
        t.command && [". Run this in a terminal instead:", h("pre", { class: "logs prompt" }, t.command)],
        !t.command && ". Check again still works.",
      ),
      t.command && copy,
    );
  };

  const draw = (o: PacksOverview) => {
    last = o;
    const ctx = (p: PackView) => ({ host: o.host, offline: offline(), os: o.os, nowMs: Date.now(), checking: checking.has(p.id) });
    const rows = o.packs.map((p) => {
      const r = row(p, ctx(p));
      const tag = h("span", { class: `tag${r.tagClass ? ` ${r.tagClass}` : ""}` }, r.tag);
      const dismissed = p.state === "ready" && closed.has(p.id);
      const buttons = dismissed ? (["review"] as Button[]) : r.buttons;
      const text = dismissed ? "The check passed. Turn it on when you want to." : r.text;
      if (p.state === "ready" && !dismissed && r.tag === "ready") return readyPanel(p, tag, text, buttons);
      return h(
        "li",
        { class: "row pack" },
        h("div", { class: "grow" }, h("div", { class: "row-title" }, p.title, tag), h("div", { class: "muted small" }, text), troubleBox(p)),
        buttons.length > 0 && h("div", { class: "actions" }, buttons.map((k) => button(p, k, offline()))),
      );
    });
    const problems =
      o.problems.length > 0
        ? [
            h("h3", null, "Skipped"),
            h("p", { class: "muted small" }, "These packs of the group are not valid, so they are not offered."),
            h(
              "ul",
              { class: "list flat", "aria-label": "Skipped packs" },
              o.problems.map((p) => h("li", { class: "row" }, h("div", { class: "grow small" }, p.id !== null && h("code", null, p.id), p.id !== null && ": ", p.message))),
            ),
          ]
        : [];
    replace(body, h("p", { class: "muted small" }, intro({ host: o.host, offline: offline() })), h("ul", { class: "list flat packs-list", "aria-label": "Optional packs" }, rows), problems);
    follow(o);
  };

  /** Turning on changes this computer only: the plugin, the rule lines, a job and what Claude reads. */
  const readyPanel = (p: PackView, tag: HTMLElement, text: string, buttons: Button[]): HTMLElement => {
    const job = p.jobs.length > 0;
    const lines = readyLines(p);
    return h(
      "li",
      { class: "pack-ready" },
      h("div", { class: "row-title" }, p.title, tag),
      h("p", { class: "small" }, text),
      h("ul", { class: "small ready-lines" }, lines.map((l) => h("li", null, l))),
      p.jobs.map((j) =>
        h("div", { class: "job pending" }, h("div", { class: "row-title" }, j.title, h("span", { class: "tag warn" }, "needs your OK")), jobDetails(j)),
      ),
      h(
        "div",
        { class: "actions" },
        buttons.map((k) => {
          const b = button(p, k, offline());
          if (k === "turnOn" && job) b.textContent = "Turn on and accept the job";
          return b;
        }),
      ),
    );
  };

  /** While a setup terminal is open or a check runs, look again every 2 s; start the check when Claude closed. */
  const follow = (o: PacksOverview) => {
    window.clearTimeout(timer);
    for (const p of o.packs) if (p.claudeClosed && !checking.has(p.id)) void runCheck(p);
    if (o.packs.some((p) => p.settingUp || p.claudeClosed || checking.has(p.id))) {
      timer = window.setTimeout(() => {
        if (card.isConnected) void load();
      }, 2000);
    }
  };

  const show = (o: PacksOverview) => {
    if (o.packs.length === 0 && o.problems.length === 0) {
      card.remove();
      return;
    }
    card.hidden = false;
    draw(o);
  };

  const load = () =>
    api.packs(id).then(show, (e) => replace(body, errorBox(errorText(e), () => void load())));
  void load();
  return card;
}
