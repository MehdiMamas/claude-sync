import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { compareSemver, formatSemver, parseSemver } from "../lib/semver.ts";

function cmp(a: string, b: string): number {
  const x = parseSemver(a);
  const y = parseSemver(b);
  assert.ok(x && y, `${a} / ${b} parse`);
  return compareSemver(x, y);
}

describe("semver", () => {
  it("parses with or without a leading v and drops build metadata", () => {
    assert.deepEqual(parseSemver("v1.2.3"), { major: 1, minor: 2, patch: 3, prerelease: [] });
    const v = parseSemver("1.2.3-beta.1+sha.abc");
    assert.ok(v);
    assert.equal(formatSemver(v), "1.2.3-beta.1");
  });

  it("rejects what is not semver", () => {
    for (const bad of ["", "1.2", "1.2.3.4", "01.2.3", "1.2.3-", "latest", "1.2.x"]) {
      assert.equal(parseSemver(bad), null, bad);
    }
  });

  it("orders versions per semver 2.0.0", () => {
    const ordered = [
      "1.0.0-alpha",
      "1.0.0-alpha.1",
      "1.0.0-alpha.beta",
      "1.0.0-beta",
      "1.0.0-beta.2",
      "1.0.0-beta.11",
      "1.0.0-rc.1",
      "1.0.0",
      "1.0.1",
      "1.2.0",
      "1.10.0",
      "2.0.0",
    ];
    for (let i = 0; i < ordered.length - 1; i++) {
      const a = ordered[i] as string;
      const b = ordered[i + 1] as string;
      assert.ok(cmp(a, b) < 0, `${a} < ${b}`);
      assert.ok(cmp(b, a) > 0, `${b} > ${a}`);
    }
    assert.equal(cmp("1.0.0+a", "v1.0.0+b"), 0);
  });
});
