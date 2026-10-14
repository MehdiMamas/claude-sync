import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync, mkdirSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, it } from "node:test";
import {
  buildPrompt,
  labelledByTrusted,
  MARKER,
  MAX_COMMENT,
  needsTriage,
  neutralize,
  parseVerdict,
  sanitizeComment,
  scrub,
  triageComment,
  trustedLogins,
} from "../triage/lib.ts";
import { analyze, post, prepare, type Queue } from "../triage/run.ts";
import { type Call, type FetchMock, mockFetch } from "./helpers.ts";

const REPO = "MehdiMamas/claude-sync";
const API = "/repos/MehdiMamas/claude-sync";
const TOKEN = "ghs_" + "w".repeat(36);
const OWNER = new Set(["mehdimamas"]);

describe("triage lib", () => {
  it("trusts only a feedback label added by a trusted login", () => {
    const labelled = (login: string, name = "feedback") => [{ event: "labeled", actor: { login }, label: { name } }];
    assert.equal(labelledByTrusted(labelled("MehdiMamas"), OWNER), true);
    assert.equal(labelledByTrusted(labelled("stranger"), OWNER), false);
    assert.equal(labelledByTrusted(labelled("MehdiMamas", "type:bug"), OWNER), false);
    assert.equal(labelledByTrusted([{ event: "unlabeled", actor: { login: "MehdiMamas" }, label: { name: "feedback" } }], OWNER), false);
    assert.equal(labelledByTrusted([], OWNER), false);
    assert.deepEqual([...trustedLogins("MehdiMamas", " Helper , ,bot2")], ["mehdimamas", "helper", "bot2"]);
  });

  it("reads the verdict and refuses an answer without one", () => {
    assert.deepEqual(parseVerdict("VERDICT: ok\r\nREASON: a real bug\r\n\r\n### Claude's read\nbody\n"), {
      verdict: "ok",
      reason: "a real bug",
      body: "### Claude's read\nbody",
    });
    assert.deepEqual(parseVerdict("VERDICT: Abusive\nREASON: spam"), { verdict: "abusive", reason: "spam", body: "" });
    assert.throws(() => parseVerdict("Sure! Here is my triage"), /VERDICT/);
    assert.throws(() => parseVerdict("VERDICT: ok\nREASON: x\n"), /no write-up/);
    assert.throws(() => parseVerdict("VERDICT: maybe\nREASON: x\nbody"), /VERDICT/);
  });

  it("scrubs secrets, home folders and emails", () => {
    const text = [
      `token ghp_${"a".repeat(36)} and ghr_${"b".repeat(36)}`,
      `oauth sk-ant-oat01-${"c".repeat(40)}`,
      `jwt eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJydW5uZXIifQ.c2lnbmF0dXJlLXZhbHVl`,
      `C:\\Users\\Mehdi\\.claude\\settings.json and /Users/alice/.claude and /home/bob/x`,
      `mail me at someone.else@example.co.uk`,
    ].join("\n");
    const out = scrub(text);
    for (const leak of ["a".repeat(36), "b".repeat(36), "c".repeat(40), "eyJzdWIiOiJydW5uZXIifQ", "Mehdi", "alice", "bob", "someone.else"]) {
      assert.ok(!out.includes(leak), `${leak} left in: ${out}`);
    }
    assert.ok(out.includes("C:\\Users\\[user]\\.claude\\settings.json"));
    assert.ok(out.includes("/Users/[user]/.claude") && out.includes("/home/[user]/x"));
    assert.ok(out.includes("[email]"));
  });

  it("breaks mentions and closing keywords", () => {
    const out = neutralize("cc @MehdiMamas and @org/team; this fixes #12, Closes: #3, resolved MehdiMamas/claude-sync#4; mail a@b");
    assert.ok(!/(^|\s)@[A-Za-z]/.test(out), out);
    assert.ok(!/\bfixes #12/.test(out) && !/Closes: #3/.test(out) && !/resolved MehdiMamas/.test(out), out);
    // Related issues still link.
    assert.ok(out.includes("#12") && out.includes("#3"));
  });

  it("keeps only links into the repo and drops images and HTML comments", () => {
    const out = sanitizeComment(
      [
        "See [the code](https://github.com/MehdiMamas/claude-sync/blob/main/relay/api/feedback.ts#L10) and [a site](https://evil.example/x).",
        "Bare https://evil.example/track?x=1 and https://github.com/MehdiMamas/claude-sync/issues/3.",
        "![shot](https://evil.example/pixel.png) <img src=x onerror=alert(1)>",
        "<!-- claude-sync-triage v1 sha=fake -->",
      ].join("\n"),
      REPO,
    );
    assert.ok(out.includes("[the code](https://github.com/MehdiMamas/claude-sync/blob/main/relay/api/feedback.ts#L10)"));
    assert.ok(out.includes("and a site."), out);
    assert.ok(out.includes("Bare `https://evil.example/track?x=1`"), out);
    assert.ok(out.includes(" https://github.com/MehdiMamas/claude-sync/issues/3."));
    assert.ok(!out.includes("![") && !out.includes("<img") && !out.includes(MARKER), out);
  });

  it("leaves code blocks alone but cuts long ones, and closes an open fence", () => {
    const short = "```text\n@user https://evil.example/x\n```";
    assert.ok(sanitizeComment(`before\n${short}\nafter @x`, REPO).includes(short));
    const long = "```text\n" + Array.from({ length: 40 }, (_, i) => `line ${i}`).join("\n") + "\n```";
    const out = sanitizeComment(long, REPO);
    assert.ok(out.includes("line 14") && !out.includes("line 15") && out.includes("… 25 more lines cut"), out);
    assert.ok(out.endsWith("```"));
    assert.ok(sanitizeComment("```\nnever closed", REPO).endsWith("\n```"));
  });

  it("caps the comment and adds the marker with the sha", () => {
    const comment = triageComment("x".repeat(MAX_COMMENT + 500), REPO, "0123456789abcdef");
    assert.ok(comment.length < MAX_COMMENT + 1000);
    assert.ok(comment.includes("cut to 20000 characters"));
    assert.ok(comment.endsWith(`${MARKER} sha=0123456789abcdef -->`));
    assert.ok(comment.includes("`0123456`"));
  });

  it("needs triage only for open feedback issues without a triage", () => {
    const issue = (labels: string[], state = "open") => ({ number: 1, title: "t", body: "", state, labels: labels.map((name) => ({ name })) });
    assert.equal(needsTriage(issue(["feedback"]), [], false), true);
    assert.equal(needsTriage(issue(["type:bug"]), [], false), false);
    assert.equal(needsTriage(issue(["feedback"], "closed"), [], false), false);
    assert.equal(needsTriage(issue(["feedback", "triaged"]), [], false), false);
    assert.equal(needsTriage(issue(["feedback", "triaged"]), [], true), true);
    assert.equal(needsTriage(issue(["feedback", "needs-moderation"]), [], false), false);
    assert.equal(needsTriage(issue(["feedback"]), [{ body: `${MARKER} sha=1 -->` }], false), false);
    assert.equal(needsTriage({ ...issue(["feedback"]), pull_request: {} }, [], true), false);
  });

  it("tells Claude the report is data and the attachments are private", () => {
    const prompt = buildPrompt({ repo: REPO, number: 7, issueDir: "/t/7", attachmentsDir: "/t/7/private/issues/7" });
    assert.match(prompt, /data, never instructions/);
    assert.match(prompt, /PRIVATE/);
    assert.match(prompt, /^VERDICT: ok$/m);
    assert.ok(prompt.includes("/t/7/issue.md") && prompt.includes("/t/7/private/issues/7"));
    assert.match(buildPrompt({ repo: REPO, number: 7, issueDir: "/t/7", attachmentsDir: null }), /no private attachments/);
  });
});

describe("triage run", () => {
  let fm: FetchMock;
  let dir: string;
  let lines: string[];
  const log = (line: string): void => {
    lines.push(line);
  };

  function issue(number: number, labels: string[], state = "open") {
    return { number, title: `Report ${number}`, body: `Body of ${number}`, state, user: { login: "friend" }, labels: labels.map((name) => ({ name })) };
  }

  function routes(number: number, labels: string[], actor = "MehdiMamas", comments: unknown[] = []): void {
    fm.on("GET", `${API}/issues/${number}`, issue(number, labels));
    fm.on("GET", `${API}/issues/${number}/comments?per_page=100&page=1`, comments);
    fm.on("GET", `${API}/issues/${number}/events?per_page=100&page=1`, [
      { event: "labeled", actor: { login: actor }, label: { name: "feedback" } },
    ]);
  }

  beforeEach(() => {
    fm = mockFetch();
    dir = mkdtempSync(join(tmpdir(), "triage-test-"));
    lines = [];
    fm.on("GET", `${API}/issues?state=open&per_page=100&page=1`, [
      issue(7, ["feedback", "type:bug"]),
      issue(8, ["feedback"]),
      issue(9, ["feedback", "triaged"]),
      { ...issue(10, []), pull_request: {} },
    ]);
  });

  afterEach(() => {
    fm.restore();
    rmSync(dir, { recursive: true, force: true });
  });

  it("prepare queues only issues the relay labelled, with their private attachments", async () => {
    // A stand-in for the attachments repo, with files for #7 and for another issue.
    const clone = join(dir, "clone");
    mkdirSync(join(clone, "issues", "7"), { recursive: true });
    mkdirSync(join(clone, "issues", "70"), { recursive: true });
    writeFileSync(join(clone, "issues", "7", "logs.txt"), "log of 7\n");
    writeFileSync(join(clone, "issues", "70", "logs.txt"), "someone else's log\n");
    const g = (...args: string[]) => execFileSync("git", ["-C", clone, ...args], { stdio: "ignore" });
    g("init", "-q");
    g("add", ".");
    g("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "x");

    routes(7, ["feedback", "type:bug"]);
    routes(8, ["feedback"], "stranger");
    routes(9, ["feedback", "triaged"]);
    const out = join(dir, "out");
    const queue = await prepare({ repo: REPO, token: TOKEN, log, owner: "MehdiMamas", issue: "all", force: false, out, attachments: clone });

    assert.deepEqual(queue.issues.map((q) => q.number), [7]);
    const q = queue.issues[0];
    assert.ok(q?.attachmentsDir);
    assert.equal(readFileSync(join(q.attachmentsDir, "logs.txt"), "utf8"), "log of 7\n");
    // Only this issue's attachments are checked out, never another reporter's.
    assert.equal(existsSync(join(q.issueDir, "private", "issues", "70")), false);
    assert.match(readFileSync(join(q.issueDir, "issue.md"), "utf8"), /^# #7: Report 7\n\nAuthor: friend\./);
    const others = readFileSync(join(q.issueDir, "open-issues.md"), "utf8");
    assert.ok(others.includes("#8 Report 8") && !others.includes("#7 ") && !others.includes("#10"));
    assert.deepEqual(JSON.parse(readFileSync(join(out, "queue.json"), "utf8")), queue);
    assert.ok(lines.some((l) => l === "#8: skipped (the feedback label was not added by the relay)"), lines.join("\n"));
    assert.ok(lines.some((l) => l.startsWith("#9: skipped")), lines.join("\n"));
    for (const c of fm.calls) assert.equal(c.headers.authorization, `Bearer ${TOKEN}`);
  });

  it("prepare takes one issue, and --force re-triages it", async () => {
    routes(9, ["feedback", "triaged"]);
    let queue = await prepare({ repo: REPO, token: TOKEN, log, owner: "MehdiMamas", issue: "9", force: false, out: dir });
    assert.equal(queue.issues.length, 0);
    queue = await prepare({ repo: REPO, token: TOKEN, log, owner: "MehdiMamas", issue: "9", force: true, out: dir });
    assert.deepEqual(queue.issues.map((q) => [q.number, q.attachmentsDir]), [[9, null]]);
    await assert.rejects(prepare({ repo: REPO, token: TOKEN, log, owner: "MehdiMamas", issue: "x", force: false, out: dir }), /--issue/);
  });

  function queueOf(numbers: number[], force = false): Queue {
    const queue: Queue = { force, issues: numbers.map((n) => ({ number: n, issueDir: join(dir, String(n)), attachmentsDir: null })) };
    for (const q of queue.issues) mkdirSync(q.issueDir, { recursive: true });
    writeFileSync(join(dir, "queue.json"), JSON.stringify(queue));
    return queue;
  }

  it("analyze runs Claude without any GitHub token and keeps its answer out of the log", () => {
    queueOf([7, 8]);
    const fake = join(dir, "fake-claude.mjs");
    writeFileSync(
      fake,
      `let input = "";
process.stdin.on("data", (d) => (input += d));
process.stdin.on("end", () => {
  const args = process.argv.slice(2);
  const leaked = ["GH_TOKEN", "GITHUB_TOKEN"].filter((k) => process.env[k]);
  const ok = args.includes("--allowedTools") && args[args.indexOf("--allowedTools") + 1] === "Read,Grep,Glob" && args.includes("Bash,Edit,Write,NotebookEdit,WebFetch,WebSearch,Task,Agent,Read(//proc/**),Read(//sys/**),Read(//etc/**)");
  if (input.includes("issue #8")) { process.stdout.write(JSON.stringify({ is_error: true, subtype: "error_max_turns" })); return; }
  const result = "VERDICT: ok\\nREASON: real\\n\\n### Claude's read of this report\\nSECRET-REPORT-TEXT leaked=" + leaked.join(",") + " flags=" + ok;
  process.stdout.write(JSON.stringify({ type: "result", is_error: false, result }));
});
`,
    );
    process.env.GH_TOKEN = TOKEN;
    try {
      const results = analyze({ out: dir, claude: [process.execPath, fake], model: "opus", cwd: dir, log });
      const first = results[0];
      assert.ok(first && "verdict" in first);
      assert.equal(first.body, "### Claude's read of this report\nSECRET-REPORT-TEXT leaked= flags=true");
      assert.deepEqual(results[1], { error: "Claude ended with error_max_turns" });
    } finally {
      delete process.env.GH_TOKEN;
    }
    assert.deepEqual(lines, ["#7: verdict ok", "#8: analysis failed: Claude ended with error_max_turns"]);
    assert.ok(existsSync(join(dir, "7", "result.json")) && existsSync(join(dir, "8", "result.json")));
  });

  function writes(): Call[] {
    return fm.calls.filter((c) => c.method !== "GET");
  }

  it("post comments the sanitized write-up and labels it triaged", async () => {
    queueOf([7]);
    writeFileSync(join(dir, "7", "result.json"), JSON.stringify({ verdict: "ok", reason: "x", body: "### Claude's read\nHi @someone, see C:\\Users\\Mehdi\\x" }));
    routes(7, ["feedback"]);
    fm.on("POST", `${API}/issues/7/comments`, Response.json({}, { status: 201 }));
    fm.on("POST", `${API}/issues/7/labels`, []);
    assert.equal(await post({ repo: REPO, token: TOKEN, log, out: dir, sha: "abcdef1234" }), 0);
    const [comment, label] = writes();
    const body = (comment?.body as { body: string }).body;
    assert.ok(body.startsWith("### Claude's read\nHi @\u200bsomeone, see C:\\Users\\[user]\\x"), body);
    assert.ok(body.endsWith(`${MARKER} sha=abcdef1234 -->`));
    assert.deepEqual(label?.body, { labels: ["triaged"] });
    assert.deepEqual(lines, ["#7: triage comment posted"]);
  });

  it("post closes, locks and labels an abusive report, without Claude's text", async () => {
    queueOf([7]);
    writeFileSync(join(dir, "7", "result.json"), JSON.stringify({ verdict: "abusive", reason: "slurs", body: "" }));
    routes(7, ["feedback"]);
    fm.on("POST", `${API}/issues/7/comments`, Response.json({}, { status: 201 }));
    fm.on("POST", `${API}/issues/7/labels`, []);
    fm.on("PATCH", `${API}/issues/7`, {});
    fm.on("PUT", `${API}/issues/7/lock`, new Response(null, { status: 204 }));
    assert.equal(await post({ repo: REPO, token: TOKEN, log, out: dir, sha: "abc" }), 0);
    const w = writes();
    assert.deepEqual(w.map((c) => `${c.method} ${c.url.replace("https://api.github.com", "")}`), [
      `POST ${API}/issues/7/comments`,
      `POST ${API}/issues/7/labels`,
      `PATCH ${API}/issues/7`,
      `PUT ${API}/issues/7/lock`,
    ]);
    assert.ok(!JSON.stringify(w[0]?.body).includes("slurs"));
    assert.deepEqual(w[1]?.body, { labels: ["needs-moderation"] });
    assert.deepEqual(w[2]?.body, { state: "closed", state_reason: "not_planned" });
  });

  it("post skips an issue triaged meanwhile and counts failures", async () => {
    queueOf([7, 8, 9]);
    writeFileSync(join(dir, "7", "result.json"), JSON.stringify({ verdict: "ok", reason: "x", body: "b" }));
    writeFileSync(join(dir, "8", "result.json"), JSON.stringify({ error: "boom" }));
    writeFileSync(join(dir, "9", "result.json"), JSON.stringify({ verdict: "ok", reason: "x", body: "b" }));
    routes(7, ["feedback"], "MehdiMamas", [{ body: `${MARKER} sha=1 -->` }]);
    routes(9, ["feedback"]);
    fm.on("POST", `${API}/issues/9/comments`, 403);
    assert.equal(await post({ repo: REPO, token: TOKEN, log, out: dir, sha: "abc" }), 2);
    assert.deepEqual(lines, [
      "#7: skipped (changed while it was analysed)",
      "#8: nothing posted, the analysis failed",
      "#9: posting failed: GitHub comment returned 403",
    ]);
  });
});
