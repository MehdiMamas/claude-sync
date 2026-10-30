import { test } from "node:test";
import assert from "node:assert/strict";
import { limitText, syncError } from "./limit.ts";

test("the limit text names a time", () => {
  assert.match(limitText(1_800_000_000), /^GitHub limit reached, resumes \d{1,2}:\d{2}/);
});

test("a rate limit replaces the error, otherwise the error shows", () => {
  assert.match(syncError({ lastError: "x", rateLimitedUntil: 1_800_000_000 }), /^GitHub limit reached/);
  assert.equal(syncError({ lastError: "offline", rateLimitedUntil: null }), "offline");
  assert.equal(syncError({ lastError: null, rateLimitedUntil: null }), null);
});
