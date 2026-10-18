// `pnpm -C app/ui test`: Node runs activity.ts directly (type stripping, Node 23.6+).
import { test } from "node:test";
import assert from "node:assert/strict";
import { avatarUrl, beforeNowFor, changeParts, collapse, dayLabel, deviceName, groupByDay, highlight, hue, initials, parseCommit, timeLabel } from "./activity.ts";

const ctx = { me: "mehdimamas", mods: ["claude-mods", "session-manager", "model-router", "setup-scout"], skills: ["review"] };
const commit = (message, atUnix = 1_000_000, author = "MehdiMamas", sha = "abc1234def") => ({ sha, message, author, atUnix });
const parse = (message, c = ctx) => parseCommit(commit(message), c);

test("the login@host prefix becomes who and which device, never text", () => {
  const a = parse("MehdiMamas@DESKTOP-MEHDI: setup-scout: read X and LinkedIn through Chrome on scheduled runs too (read only); job passes --chrome");
  assert.equal(a.login, "MehdiMamas");
  assert.equal(a.isYou, true);
  assert.equal(a.device, "desktop-mehdi");
  assert.equal(a.kind, "mod");
  assert.equal(a.title, "Updated a mod");
  assert.deepEqual(a.chips, ["setup-scout"]);
  assert.equal(a.detail, "Read X and LinkedIn through Chrome on scheduled runs too (read only); job passes --chrome");
  assert.ok(!a.title.includes("@") && !a.detail.includes("DESKTOP"));
});

test("devices differing only in case are one device", () => {
  assert.equal(parse("MehdiMamas@desktop-mehdi: add member x").device, parse("MehdiMamas@DESKTOP-MEHDI: add member x").device);
  assert.equal(deviceName("Saras-MacBook.local"), "saras-macbook");
});

test("members added and removed", () => {
  const a = parse("MehdiMamas@desktop-mehdi: add member ibrahim-ahidar");
  assert.equal(a.kind, "memberAdded");
  assert.equal(a.title, "Added ibrahim-ahidar to the group");
  const r = parse("sara-dev@laptop: remove member bo");
  assert.equal(r.kind, "memberRemoved");
  assert.equal(r.title, "Removed bo from the group");
  assert.equal(r.isYou, false);
  assert.equal(parse("sara-dev@laptop: add member MehdiMamas").title, "Added you to the group");
});

test("group memory and group creation", () => {
  const m = parse("MehdiMamas@DESKTOP-MEHDI: group memory: personal vs group scope, group-memory skill, mods are made claude-sync wide");
  assert.equal(m.kind, "memory");
  assert.equal(m.title, "Updated group memory");
  assert.deepEqual(m.chips, []);
  assert.equal(m.detail, "Personal vs group scope, group-memory skill, mods are made claude-sync wide");
  const c = parse("MehdiMamas@DESKTOP-MEHDI: create group (start the repo)");
  assert.equal(c.kind, "created");
  assert.equal(c.title, "Created the group");
  assert.equal(c.detail, "Start the repo");
  assert.equal(parse("ana@mac: create group").detail, "");
});

test("old commits without the prefix use the commit author", () => {
  const a = parseCommit(commit("session-manager: 0.2.0", 1, "sara-dev"), ctx);
  assert.equal(a.login, "sara-dev");
  assert.equal(a.device, null);
  assert.equal(a.kind, "mod");
  assert.deepEqual(a.chips, ["session-manager"]);
  assert.equal(a.detail, "Version 0.2.0");
});

test("engine sync summaries", () => {
  assert.deepEqual(pick(parse("ana@mac: share skills/pdf-tools")), ["skill", "Shared a skill", ["pdf-tools"]]);
  assert.deepEqual(pick(parse("ana@mac: update skills/review")), ["skill", "Updated a skill", ["review"]]);
  assert.deepEqual(pick(parse("ana@mac: update skills/a, skills/b")), ["skill", "Updated 2 skills", ["a", "b"]]);
  assert.deepEqual(pick(parse("ana@mac: remove agents/old.md")), ["agent", "Removed an agent", ["old"]]);
  assert.deepEqual(pick(parse("ana@mac: update CLAUDE.md")), ["memory", "Updated the group's CLAUDE.md", []]);
  assert.deepEqual(pick(parse("ana@mac: share CLAUDE.md")), ["memory", "Shared the group's CLAUDE.md", []]);
  assert.deepEqual(pick(parse("ana@mac: update settings.shared.json")), ["settings", "Updated shared settings", []]);
  assert.deepEqual(pick(parse("ana@mac: update CLAUDE.md, skills/review")), ["other", "Updated 2 items", ["CLAUDE.md", "review"]]);
  assert.deepEqual(pick(parse("ana@mac: update skills/a; remove skills/b")), ["skill", "Updated a skill, removed a skill", ["a", "b"]]);
  assert.deepEqual(pick(parse("ana@mac: update 5 items")), ["other", "Updated 5 items", []]);
  assert.deepEqual(pick(parse("ana@mac: update README.md")), ["other", "Updated README.md", []]);
  assert.deepEqual(
    pick(parse("ana@mac: update plugins/claude-mods/mods/setup-scout/SKILL.md")),
    ["mod", "Updated a mod", ["setup-scout"]],
  );
  assert.deepEqual(
    pick(parse("ana@mac: update plugins/claude-mods/mods/model-router/router.js, plugins/claude-mods/mods/model-router/README.md")),
    ["mod", "Updated a mod", ["model-router"]],
  );
  assert.deepEqual(pick(parse("ana@mac: update commands/ship.md, output-styles/terse.md")), ["other", "Updated 2 items", ["ship", "terse"]]);
});

test("hand-written areas, known and unknown", () => {
  assert.deepEqual(pick(parse("ana@mac: skills/review: tighten checklist")), ["skill", "Updated a skill", ["review"]]);
  assert.deepEqual(pick(parse("ana@mac: review: tighten checklist")), ["skill", "Updated a skill", ["review"]]);
  assert.deepEqual(pick(parse("ana@mac: settings: allow pnpm")), ["settings", "Changed shared settings", []]);
  assert.deepEqual(pick(parse("ana@mac: jobs: run setup-scout weekly")), ["job", "Changed a scheduled job", []]);
  const u = parse("ana@mac: docs site: new layout");
  assert.equal(u.kind, "other");
  assert.equal(u.title, "Updated docs site");
  assert.equal(u.detail, "New layout");
  // Conventional types and free text are not areas.
  assert.equal(parse("ana@mac: fix: typo in readme").title, "Fix: typo in readme");
  assert.equal(parse("Initial commit").title, "Initial commit");
  assert.equal(parse("ana@mac: update the docs").title, "Update the docs");
});

test("the commit body goes into the detail", () => {
  const a = parse("ana@mac: setup-scout: faster\n\nSkips the Reddit pass when offline.\r\nSecond line.");
  assert.equal(a.detail, "Faster\nSkips the Reddit pass when offline.\nSecond line.");
});

test("runs of look-alike entries collapse with a count", () => {
  const items = [
    parseCommit(commit("ana@mac: update CLAUDE.md", 10_000, "ana", "s1"), ctx),
    parseCommit(commit("ana@mac: update CLAUDE.md", 9_700, "ana", "s2"), ctx),
    parseCommit(commit("ana@laptop: update CLAUDE.md", 9_200, "ana", "s3"), ctx),
    // 20 minutes later than the run: separate.
    parseCommit(commit("ana@mac: update CLAUDE.md", 7_900, "ana", "s4"), ctx),
    parseCommit(commit("bo@mac: update CLAUDE.md", 7_800, "bo", "s5"), ctx),
    parseCommit(commit("bo@mac: update skills/review", 7_700, "bo", "s6"), ctx),
  ];
  const out = collapse(items);
  assert.deepEqual(out.map((a) => a.shas), [["s1", "s2", "s3"], ["s4"], ["s5"], ["s6"]]);
  assert.equal(out[0].atUnix, 10_000);
  assert.equal(out[0].firstAtUnix, 9_200);
});

test("collapsed details keep each distinct line once", () => {
  const out = collapse([
    parseCommit(commit("ana@mac: setup-scout: b", 100, "ana", "1"), ctx),
    parseCommit(commit("ana@mac: setup-scout: a", 50, "ana", "2"), ctx),
    parseCommit(commit("ana@mac: setup-scout: b", 40, "ana", "3"), ctx),
  ]);
  assert.equal(out.length, 1);
  assert.equal(out[0].detail, "B\nA");
});

test("days and times", () => {
  const now = new Date(2026, 9, 14, 15, 0); // Wed 14 Oct 2026, 15:00 local
  const at = (d, h, m = 0) => Math.floor(new Date(2026, 9, d, h, m).getTime() / 1000);
  assert.equal(dayLabel(new Date(2026, 9, 14, 0, 1), now, "en-US"), "Today");
  assert.equal(dayLabel(new Date(2026, 9, 13, 23, 59), now, "en-US"), "Yesterday");
  assert.equal(dayLabel(new Date(2026, 9, 10, 12), now, "en-US"), "Saturday");
  assert.equal(dayLabel(new Date(2026, 9, 1, 12), now, "en-US"), "Thu, Oct 1");
  assert.equal(dayLabel(new Date(2025, 9, 1, 12), now, "en-US"), "Wed, Oct 1, 2025");
  const items = [at(14, 14), at(14, 9), at(13, 22), at(10, 8)].map((t, i) => parseCommit(commit(`a@b: x${i}: y`, t), ctx));
  assert.deepEqual(
    groupByDay(items, now, "en-US").map((d) => [d.label, d.items.length]),
    [["Today", 2], ["Yesterday", 1], ["Saturday", 1]],
  );
  assert.equal(timeLabel(at(14, 14, 59) + 30, now), "just now");
  assert.equal(timeLabel(at(14, 14, 45), now), "15 min ago");
  assert.equal(timeLabel(at(14, 12, 30), now), "2 h ago");
  assert.equal(timeLabel(at(13, 22, 5), now, "en-GB"), "22:05");
  assert.equal(timeLabel(at(13, 9, 5), now, "en-US"), "9:05 AM");
});

test("avatars", () => {
  assert.equal(initials("MehdiMamas"), "MM");
  assert.equal(initials("ibrahim-ahidar"), "IA");
  assert.equal(initials("yassine"), "YA");
  assert.equal(hue("MehdiMamas"), hue("mehdimamas"));
  assert.ok(hue("x") >= 0 && hue("x") < 360);
  assert.equal(avatarUrl("ibrahim-ahidar"), "https://avatars.githubusercontent.com/ibrahim-ahidar?s=64");
  // Anything that is not a plain login gets initials only, never a crafted URL.
  for (const bad of ["", "a/b", "../x", "x?s=1", "x y", "sara-dev (laptop)", "é"]) assert.equal(avatarUrl(bad), null, bad);
});

function pick(a) {
  return [a.kind, a.title, a.chips];
}

test("a folded entry keeps each commit as a member, newest first", () => {
  const msg = (n) => `mehdi@desktop: session-manager: Change ${n} (#${n})`;
  const items = [3, 2, 1].map((n) => parse(msg(n)));
  items.forEach((a, i) => (a.sha = `sha${3 - i}`));
  const [folded] = collapse(items.map((a, i) => ({ ...a, atUnix: 1_000_000 - i * 60, firstAtUnix: 1_000_000 - i * 60 })));
  assert.equal(folded.members.length, 3);
  assert.deepEqual(folded.members.map((m) => m.sha), ["sha3", "sha2", "sha1"]);
  assert.equal(parse("sara-dev@laptop: add member bo").members.length, 1);
});

test("changeParts: subject, issues, bullets", () => {
  const p = changeParts("mehdi@desktop: session-manager: leave finished workers warm (Fixes #182/#183)\n\n- Auto-stop is gentle\n* Second #182 mention");
  assert.equal(p.subject, "Leave finished workers warm");
  assert.deepEqual(p.issues, [182, 183]);
  assert.deepEqual(p.bullets, ["Auto-stop is gentle", "Second #182 mention"]);
  assert.equal(p.before, null);
  assert.equal(p.now, null);
});

test("changeParts: Before/Now lines are removed from the bullets", () => {
  const p = changeParts("x@y: Fix thing\n\nbefore: it stopped.\nNow: it stays.\n- other");
  assert.equal(p.before, "it stopped.");
  assert.equal(p.now, "it stays.");
  assert.deepEqual(p.bullets, ["other"]);
  const w = changeParts("x@y: Fix thing\n\nWas: a\nAfter: b");
  assert.deepEqual([w.before, w.now], ["a", "b"]);
});

test("changeParts: an arrow line gives Before/Now, and nothing is invented", () => {
  const p = changeParts("x@y: Tune\n\n- Timeout: 5 s → 10 s\n- keep");
  assert.equal(p.before, "5 s");
  assert.equal(p.now, "10 s");
  assert.deepEqual(p.bullets, ["keep"]);
  const a = changeParts("x@y: Tune\n\n- plain line");
  assert.deepEqual([a.before, a.now], [null, null]);
  assert.deepEqual(changeParts("x@y: update CLAUDE.md").issues, []);
});

test("highlight marks numbers with units, names and key words", () => {
  const segs = highlight("Stays warm for 10 min, `stop_worker` is never used; 90% now, 3 GB, 200k");
  const strong = segs.filter((s) => s.strong).map((s) => s.text);
  assert.deepEqual(strong, ["10 min", "never", "90%", "now", "3 GB", "200k"]);
  assert.deepEqual(segs.filter((s) => s.code).map((s) => s.text), ["stop_worker"]);
  assert.equal(segs.map((s) => s.text).join(""), "Stays warm for 10 min, stop_worker is never used; 90% now, 3 GB, 200k");
  assert.deepEqual(highlight("plain"), [{ text: "plain" }]);
});

test("beforeNowFor: own body first, then the notes entry, else nothing", () => {
  const notes = { abc: { before: "old", now: "new" } };
  assert.deepEqual(beforeNowFor({ before: "b", now: "n" }, "abc", notes), { before: "b", now: "n" });
  assert.deepEqual(beforeNowFor({ before: null, now: null }, "abc", notes), { before: "old", now: "new" });
  assert.deepEqual(beforeNowFor({ before: null, now: null }, "zzz", notes), { before: null, now: null });
  assert.deepEqual(beforeNowFor({ before: null, now: null }, "abc", { abc: { now: "only" } }), { before: null, now: "only" });
});
