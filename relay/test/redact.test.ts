import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { pickSprint } from "../lib/milestone.ts";
import { keepTail, redact } from "../lib/redact.ts";

describe("redact", () => {
  // Same cases as crates/core/src/secrets.rs, plus Bearer.
  const cases: Array<[string, string]> = [
    ["github_pat", "github_pat_" + "A1_".repeat(20)],
    ["sk-ant", "sk-ant-api03-" + "x".repeat(30)],
    ["ghp", "ghp_" + "a".repeat(36)],
    ["gh-token", "gho_" + "b".repeat(36)],
    ["gh-token", "ghu_" + "c".repeat(36)],
    ["gh-token", "ghs_" + "d".repeat(36)],
    ["gh-token", "ghr_" + "e".repeat(76)],
    ["aws-access-key", "AKIA" + "Q".repeat(16)],
    ["slack-token", "xoxb-1234567890-abcdef"],
  ];
  for (const [name, secret] of cases) {
    it(`redacts ${name}`, () => {
      const out = redact(`before ${secret} after`);
      assert.equal(out, `before [REDACTED:${name}] after`);
    });
  }

  it("leaves no tail of a longer token", () => {
    assert.equal(redact("ghp_" + "a".repeat(40) + "!"), "[REDACTED:ghp]!");
  });

  it("redacts a private key block and a lone header", () => {
    const block = "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAAB3NzaC1yc2E\nmore\n-----END OPENSSH PRIVATE KEY-----";
    assert.equal(redact(`k=${block}\nnext`), "k=[REDACTED:private-key]\nnext");
    assert.equal(redact("x -----BEGIN RSA PRIVATE KEY-----\nMIIEsecret"), "x [REDACTED:private-key]");
  });

  it("redacts access_token and refresh_token values but keeps the keys", () => {
    assert.equal(
      redact('{"access_token": "abc\\"def", "refresh_token":"r1", "scope":"repo"}'),
      '{"access_token": "[REDACTED:token-field]", "refresh_token":"[REDACTED:token-field]", "scope":"repo"}',
    );
  });

  it("redacts Bearer tokens, any case", () => {
    assert.equal(redact("Authorization: Bearer abc.def-ghi_123=="), "Authorization: Bearer [REDACTED:bearer]");
    assert.equal(redact("authorization: bearer xyz"), "authorization: bearer [REDACTED:bearer]");
  });

  it("does not touch ordinary text", () => {
    const text = "2026-10-13 INFO sync done in 12ms (ghp_ short, AKIA short)";
    assert.equal(redact(text), text);
  });
});

describe("keepTail", () => {
  it("keeps short text as is", () => {
    assert.equal(keepTail("abc", 10), "abc");
  });

  it("keeps the last bytes and notes the cut", () => {
    assert.equal(keepTail("0123456789", 4), "[… 6 earlier bytes cut]\n6789");
  });

  it("never splits a UTF-8 character", () => {
    // "é" is 2 bytes; a 3-byte tail of "aéé" starts mid-character and moves forward.
    assert.equal(keepTail("aéé", 3), "[… 3 earlier bytes cut]\né");
  });
});

describe("pickSprint", () => {
  const now = new Date("2026-10-13T15:00:00Z");
  const m = (number: number, title: string, due_on: string | null, state = "open") => ({ number, title, due_on, state });

  it("picks the earliest open sprint due today or later", () => {
    const got = pickSprint(
      [
        m(1, "Sprint 1", "2026-10-06T07:00:00Z"),
        m(3, "Sprint 3", "2026-10-27T07:00:00Z"),
        m(2, "Sprint 2", "2026-10-20T07:00:00Z"),
        m(9, "Backlog", "2026-10-14T07:00:00Z"),
        m(8, "Sprint 8", null),
      ],
      now,
    );
    assert.equal(got?.number, 2);
  });

  it("counts a sprint due today as upcoming", () => {
    const got = pickSprint([m(1, "Sprint 1", "2026-10-13T07:00:00Z"), m(2, "Sprint 2", "2026-10-20T07:00:00Z")], now);
    assert.equal(got?.number, 1);
  });

  it("falls back to the sprint with the latest due date when all are past", () => {
    const got = pickSprint([m(1, "Sprint 1", "2026-09-01T07:00:00Z"), m(2, "Sprint 2", "2026-09-15T07:00:00Z")], now);
    assert.equal(got?.number, 2);
  });

  it("returns null when there is no open sprint with a due date", () => {
    assert.equal(pickSprint([], now), null);
    assert.equal(pickSprint([m(1, "Sprint 1", null), m(2, "Release 2", "2026-11-01T00:00:00Z")], now), null);
    assert.equal(pickSprint([m(1, "Sprint 1", "2026-11-01T00:00:00Z", "closed")], now), null);
    assert.equal(pickSprint([m(1, "Sprint 1 (old)", "2026-11-01T00:00:00Z")], now), null);
  });
});
