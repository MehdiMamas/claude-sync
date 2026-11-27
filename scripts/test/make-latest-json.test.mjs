import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { latest } from "../make-latest-json.mjs";

test("latest.json has the four platforms with each .sig, and no linux-aarch64", () => {
  const d = mkdtempSync(join(tmpdir(), "latest-"));
  const v = "2.0.0";
  for (const n of [
    `claude-sync_${v}_windows_x86_64-setup.exe`,
    `claude-sync_${v}_darwin_universal.app.tar.gz`,
    `claude-sync_${v}_linux_x86_64.AppImage`,
  ])
    writeFileSync(join(d, `${n}.sig`), `sig-of-${n}\n`);
  const j = latest(d, v, "o/r");
  assert.deepEqual(Object.keys(j.platforms), [
    "windows-x86_64",
    "darwin-aarch64",
    "darwin-x86_64",
    "linux-x86_64",
  ]);
  assert.equal(j.version, v);
  assert.equal(j.platforms["darwin-x86_64"].url, `https://github.com/o/r/releases/download/v2.0.0/claude-sync_2.0.0_darwin_universal.app.tar.gz`);
  assert.equal(j.platforms["windows-x86_64"].signature, `sig-of-claude-sync_2.0.0_windows_x86_64-setup.exe`);
});
