// `pnpm -C app/ui test`: Node runs activity-consent.ts directly (type stripping, Node 23.6+).
import { test } from "node:test";
import assert from "node:assert/strict";
import { ACTIVITY_COPY, SHARE_DEFAULT, decide, statusText } from "./activity-consent.ts";

test("the consent copy says what is read and what is not, and where it goes", () => {
  const text = ACTIVITY_COPY.paragraphs.join(" ");
  assert.match(text, /seconds since the last mouse or keyboard input/);
  assert.match(text, /never reads which keys you press/);
  assert.match(text, /never to your groups, to other people, or to us/);
  assert.match(text, /deletes the value on this machine at once/);
  assert.equal(ACTIVITY_COPY.turnOn, "Turn on");
  assert.equal(ACTIVITY_COPY.notNow, "Not now");
});

test("sharing is off by default", () => {
  assert.equal(SHARE_DEFAULT, false);
  assert.equal(statusText(SHARE_DEFAULT), "Off");
});

test("it turns on only after consent", () => {
  assert.equal(decide(false, "notNow"), false);
  assert.equal(decide(false, "turnOn"), true);
  assert.equal(statusText(decide(false, "turnOn")), "On");
});

test("turning it off is one click and needs no consent", () => {
  assert.equal(decide(true, "turnOff"), false);
  assert.equal(decide(true, "notNow"), true);
});
