// `pnpm -C app/ui test`: Node runs signin-copy.ts directly (type stripping, Node 23.6+).
import { test } from "node:test";
import assert from "node:assert/strict";
import { signInCopy } from "./signin-copy.ts";

test("an expired sign-in asks to sign in again, naming the login", () => {
  const c = signInCopy({ signedIn: false, login: null, expiredLogin: "octocat", clientIdMissing: false });
  assert.equal(c.heading, "Your GitHub sign-in expired");
  assert.equal(c.button, "Sign in again");
  assert.match(c.text, /octocat/);
  assert.match(c.text, /groups and files on this computer are kept/);
});

test("a first sign-in is not called expired", () => {
  for (const auth of [null, { signedIn: false, login: null, expiredLogin: null, clientIdMissing: false }]) {
    const c = signInCopy(auth);
    assert.equal(c.heading, "Sign in with GitHub");
    assert.equal(c.button, "Sign in with GitHub");
    assert.doesNotMatch(c.text, /expired/);
  }
});
