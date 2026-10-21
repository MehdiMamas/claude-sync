// `pnpm -C app/ui test`: Node runs copy.ts directly (type stripping, Node 23.6+).
import { test } from "node:test";
import assert from "node:assert/strict";
import { clashesOf, clashTitle, compose, keys, sections, warning } from "./copy.ts";

const md = `Intro line

# How we work

- Use tabs
- Run the tests

\`\`\`sh
pnpm test
\`\`\`

## Reviews
- Keep PRs small
`;

test("a CLAUDE.md is cut into sections of lines and code blocks", () => {
  assert.deepEqual(
    sections(md).map((s) => [s.heading, s.blocks.map((b) => b.lines.join("|"))]),
    [
      [null, ["Intro line"]],
      ["# How we work", ["- Use tabs", "- Run the tests", "```sh|pnpm test|```"]],
      ["## Reviews", ["- Keep PRs small"]],
    ],
  );
});

test("the picked lines come back under their headings", () => {
  const all = sections(md);
  assert.equal(compose(all, new Set(["s1.1", "s2.0"])), "# How we work\n\n- Run the tests\n\n## Reviews\n\n- Keep PRs small\n");
  assert.equal(compose(all, new Set(keys(all, "s0"))), "Intro line\n");
  assert.equal(compose(all, new Set()), "");
  assert.equal(
    compose(all, new Set(keys(all))),
    "Intro line\n\n# How we work\n\n- Use tabs\n- Run the tests\n```sh\npnpm test\n```\n\n## Reviews\n\n- Keep PRs small\n",
  );
});

const A = { owner: "ana", repo: "data" };
const B = { owner: "acme", repo: "team" };
const side = (source, name, text) => ({ source, name, text });
const clash = (kind, a, b, pointer = null) => ({ id: kind, kind, pointer, a, b });

test("clashes of a group put its side first", () => {
  const own = side({ kind: "own" }, "Your CLAUDE.md", "Use tabs");
  const ours = side({ kind: "group", id: A }, "Friends", "- use tabs");
  const theirs = side({ kind: "group", id: B }, "Acme", "Never use tabs");
  const all = [clash("duplicate", own, ours), clash("contradiction", theirs, own)];
  const mine = clashesOf(all, A);
  assert.deepEqual(
    mine.map((c) => [c.a.name, c.b.name]),
    [["Friends", "Your CLAUDE.md"]],
  );
  assert.equal(clashTitle(mine[0]), "Your CLAUDE.md says the same thing");
  assert.equal(clashTitle(clash("setting", ours, theirs, "/model")), "Friends and Acme set /model differently");
  assert.equal(warning(clash("contradiction", ours, theirs)), '"- use tabs" contradicts Acme: "Never use tabs"');
  assert.equal(warning(clash("duplicate", ours, own)), '"- use tabs" is already in Your CLAUDE.md: "Use tabs"');
});

import { coverageLines } from "./copy.ts";
test("coverageLines names uncovered repos in plain words", () => {
  assert.deepEqual(coverageLines({ missing: [{ owner: "a", repo: "b" }], noInstallation: false }), [
    "Not covered: a/b — the host must install the App there",
  ]);
  assert.deepEqual(coverageLines({ missing: [], noInstallation: false }), []);
  assert.equal(coverageLines({ missing: [], noInstallation: true }).length, 1);
});
