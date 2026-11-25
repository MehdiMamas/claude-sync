// `pnpm -C app/ui test`: the words of the Optional packs card, state by state, as the screens of
// the blueprint canvas "Opt-in packs" have them. Node runs packs-copy.ts directly.
import { test } from "node:test";
import assert from "node:assert/strict";
import { ago, computerName, description, intro, modNames, osList, readyLines, row, since } from "./packs-copy.ts";

const NOW = Date.UTC(2026, 10, 25, 12, 0, 0);
const MIN = 60_000;
const DAY = 86_400_000;

const pack = (over = {}) => ({
  id: "passbridge",
  title: "PassBridge sign-in",
  summary: "Claude signs in to GitHub, Google, Vercel and Apple through your PassBridge app.",
  needs: [{ name: "the PassBridge app" }],
  os: ["windows", "macos", "linux"],
  state: "available",
  by: null,
  since: null,
  lastCheck: null,
  checkTimeout: 60,
  plugins: ["passbridge-signin@claude-mods"],
  jobs: [],
  rulePreview: "then PassBridge for sign-ins",
  ruleLines: 1,
  hasFiles: false,
  settingUp: false,
  claudeClosed: false,
  changed: false,
  groupEnabled: false,
  ...over,
});
const ctx = (over = {}) => ({ host: false, offline: false, os: "windows", nowMs: NOW, checking: false, ...over });

test("screen 01: a pack that is not set up has its summary, what it needs and Set up with Claude", () => {
  const r = row(pack(), ctx());
  assert.deepEqual(r, {
    tag: "not set up",
    tagClass: "off",
    text: "Claude signs in to GitHub, Google, Vercel and Apple through your PassBridge app. Needs the PassBridge app.",
    buttons: ["setup"],
  });
  const scout = pack({ summary: "Mon, Wed and Fri: reviews your Claude setup.", needs: [], jobs: [{ id: "s" }] });
  assert.equal(description(scout), "Mon, Wed and Fri: reviews your Claude setup. Runs on a schedule.");
});

test("screens 02 and 04: the chat is open, then the check runs", () => {
  assert.deepEqual(row(pack({ state: "tried", settingUp: true }), ctx()), {
    tag: "setting up",
    tagClass: "",
    text: "Claude Code is open in a terminal. Finish there; claude-sync checks the pack when you close it.",
    buttons: [],
  });
  const closed = row(pack({ state: "tried", claudeClosed: true }), ctx());
  assert.equal(closed.tag, "checking");
  assert.equal(closed.text, "Claude Code closed. Running the pack's check (at most 60 s)…");
  assert.equal(row(pack(), ctx({ checking: true })).text, "Running the pack's check (at most 60 s)…");
});

test("screen 08: a failed check names the reason and offers Check again and Set up", () => {
  const p = pack({ state: "tried", lastCheck: { at: NOW - 2 * MIN, exit: 1, tail: "PassBridge agent not reachable", hash: "h" } });
  assert.deepEqual(row(p, ctx()), {
    tag: "not set up",
    tagClass: "off",
    text: "Last check failed 2 min ago: PassBridge agent not reachable (exit 1). Nothing was turned on.",
    buttons: ["check", "setup"],
  });
  const quiet = pack({ state: "tried", lastCheck: { at: NOW - 2 * MIN, exit: 3, tail: "", hash: "h" } });
  assert.equal(row(quiet, ctx()).text, "Last check failed 2 min ago (exit 3). Nothing was turned on.");
});

test("screen 05: a ready pack lists what turning it on changes here", () => {
  const p = pack({ state: "ready", lastCheck: { at: NOW - 10_000, exit: 0, tail: "", hash: "h" } });
  const r = row(p, ctx());
  assert.equal(r.tag, "ready");
  assert.equal(r.text, "The check passed just now. Turning it on changes this computer only:");
  assert.deepEqual(r.buttons, ["turnOn", "notNow"]);
  assert.deepEqual(readyLines(p), [
    "Turns on the mod passbridge-signin",
    'Adds one line to the rules Claude reads: "then PassBridge for sign-ins"',
    "No scheduled jobs",
    "From then on this computer gets the pack's updates; your other computers don't",
  ]);
  assert.deepEqual(modNames(p), ["passbridge-signin"]);
});

test("screen 09: a pack with a job says so", () => {
  const p = pack({ state: "ready", plugins: ["setup-scout@claude-mods"], ruleLines: 0, rulePreview: null, jobs: [{ id: "s" }] });
  assert.deepEqual(readyLines(p), [
    "Turns on the mod setup-scout",
    "Registers its scheduled job on this computer (below)",
    "From then on this computer gets the pack's updates; your other computers don't",
  ]);
  const many = readyLines(pack({ jobs: [{ id: "a" }, { id: "b" }], ruleLines: 3 }));
  assert.ok(many.includes("Adds 3 lines to the rules Claude reads, starting with \"then PassBridge for sign-ins\""));
  assert.ok(many.includes("Registers its 2 scheduled jobs on this computer (below)"));
});

test("screens 06, 10 and 12: on, with and without a job", () => {
  const on = pack({ state: "on", by: "setup", since: NOW - 5_000 });
  assert.deepEqual(row(on, ctx()), {
    tag: "on",
    tagClass: "",
    text: "On on this computer since just now. Gets the pack's updates here only.",
    buttons: ["turnOff"],
  });
  assert.equal(row({ ...on, since: NOW - DAY }, ctx()).text, "On on this computer since yesterday. Gets the pack's updates here only.");
  assert.equal(row({ ...on, jobs: [{ id: "s" }] }, ctx()).text, "On on this computer since just now, with its scheduled job.");
});

test("screen 07: off by the member says it will not ask again", () => {
  const r = row(pack({ state: "off", by: "setup" }), ctx());
  assert.equal(r.tag, "off");
  assert.equal(r.text, "You turned it off on this computer. It won't ask again and gets no updates here. Its settings were put back to the group's.");
  assert.deepEqual(r.buttons, ["turnOnAgain"]);
});

test("screen 11: a pack that changed and fails stays on with Fix with Claude", () => {
  const p = pack({ state: "failing", by: "setup", lastCheck: { at: NOW, exit: 1, tail: "PassBridge 2.0 or later is needed", hash: "h" } });
  assert.deepEqual(row(p, ctx()), {
    tag: "check fails",
    tagClass: "warn",
    text: "The pack changed and its check now fails: PassBridge 2.0 or later is needed (exit 1). It stays on until you fix it or turn it off.",
    buttons: ["turnOff", "fix"],
  });
});

test("screen 14: the host's packs stayed on, reflect stays off", () => {
  const since = Date.UTC(2026, 10, 24, 9);
  assert.deepEqual(row(pack({ state: "on", by: "carry-over", since }), ctx({ host: true })), {
    tag: "on",
    tagClass: "",
    text: "On since 2026-11-24 · kept on from before packs",
    buttons: ["turnOff"],
  });
  assert.equal(row(pack({ state: "on", by: "carry-over", since, jobs: [{ id: "s" }] }), ctx({ host: true })).text, "On since 2026-11-24 · kept on from before packs · job accepted");
  assert.deepEqual(row(pack({ state: "off", by: "carry-over", since }), ctx({ host: true })), {
    tag: "off",
    tagClass: "off",
    text: "It was off on this computer before packs, so it stays off.",
    buttons: ["setup"],
  });
});

test("screen 15: a pack that is not for this system has no button", () => {
  const busy = pack({ id: "busy-switch", state: "unavailable", os: ["windows", "linux"] });
  const r = row(busy, ctx({ os: "macos" }));
  assert.equal(r.tag, "not for macOS");
  assert.equal(r.text, "It works on Windows and Linux only, so it can't be set up on this Mac.");
  assert.deepEqual(r.buttons, []);
  assert.equal(computerName("windows"), "Windows PC");
  assert.equal(computerName("linux"), "Linux computer");
  assert.equal(osList(["windows"]), "Windows");
  assert.equal(osList(["windows", "macos", "linux"]), "Windows, macOS and Linux");
});

test("the sentence under the heading follows who you are and whether you are online", () => {
  assert.equal(
    intro({ host: false, offline: false }),
    "Extras for setups like the host's. Each one turns on for this computer only after Claude helps you set it up and its check passes. Nothing here asks you twice.",
  );
  assert.equal(intro({ host: true, offline: false }), "You host this group. Packs in use on this computer when packs arrived stayed on.");
  assert.equal(
    intro({ host: false, offline: true }),
    "Setting up needs the internet, because Claude Code does. Check again, Turn on and Turn off work offline.",
  );
});

test("times read as a person says them", () => {
  assert.equal(ago(NOW - 10_000, NOW), "just now");
  assert.equal(ago(NOW - 2 * MIN, NOW), "2 min ago");
  assert.equal(ago(NOW - 3 * 3600_000, NOW), "3 h ago");
  assert.equal(ago(Date.UTC(2026, 10, 1), NOW), "2026-11-01");
  assert.equal(since(NOW - 5_000, NOW), "just now");
  assert.equal(since(NOW - 3600_000, NOW), "today");
  assert.equal(since(NOW - DAY, NOW), "yesterday");
  assert.equal(since(NOW - 5 * DAY, NOW), "2026-11-20");
});
