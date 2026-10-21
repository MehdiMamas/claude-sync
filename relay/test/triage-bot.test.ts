import assert from "node:assert/strict";
import { generateKeyPairSync, verify } from "node:crypto";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, it } from "node:test";
import { appJwt, botToken, NOT_CONFIGURED, readKeyFile } from "../triage/bot.ts";
import { post, runToken } from "../triage/run.ts";
import { type FetchMock, mockFetch } from "./helpers.ts";

const { privateKey, publicKey } = generateKeyPairSync("rsa", {
  modulusLength: 2048,
  privateKeyEncoding: { type: "pkcs8", format: "pem" },
  publicKeyEncoding: { type: "spki", format: "pem" },
});

describe("triage bot", () => {
  let dir: string;
  let fm: FetchMock;
  let keyFile: string;
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "triage-bot-"));
    keyFile = join(dir, "app.pem");
    writeFileSync(keyFile, privateKey, { mode: 0o600 });
    chmodSync(keyFile, 0o600);
    fm = mockFetch();
  });
  afterEach(() => {
    fm.restore();
    rmSync(dir, { recursive: true, force: true });
  });
  const env = () => ({ TRIAGE_APP_ID: "12345", TRIAGE_APP_INSTALLATION_ID: "678", TRIAGE_APP_KEY_FILE: keyFile });

  it("builds an RS256 JWT with iat now-60s, exp now+9min and iss the app id", () => {
    const now = 1_800_000_000_000;
    const [h, b, s] = appJwt("12345", privateKey, now).split(".") as [string, string, string];
    assert.deepEqual(JSON.parse(Buffer.from(h, "base64url").toString()), { alg: "RS256", typ: "JWT" });
    assert.deepEqual(JSON.parse(Buffer.from(b, "base64url").toString()), { iat: 1_800_000_000 - 60, exp: 1_800_000_000 + 540, iss: "12345" });
    assert.equal(verify("RSA-SHA256", Buffer.from(`${h}.${b}`), publicKey, Buffer.from(s, "base64url")), true);
  });

  it("mints an installation token with the JWT", async () => {
    fm.on("POST", "/app/installations/678/access_tokens", Response.json({ token: "ghs_installation" }, { status: 201 }));
    assert.equal(await botToken(env(), fetch), "ghs_installation");
    const call = fm.calls[0];
    assert.ok(call?.headers.authorization?.startsWith("Bearer ey"));
    assert.equal(fm.calls.length, 1);
  });

  it("refuses without configuration, with a clear message", async () => {
    await assert.rejects(botToken({}), /triage bot not configured: set TRIAGE_APP_ID/);
    await assert.rejects(botToken({ TRIAGE_APP_ID: "1" }), new RegExp(NOT_CONFIGURED.slice(0, 30)));
    assert.equal(fm.calls.length, 0);
  });

  it("refuses a key file that group or others can access", { skip: process.platform === "win32" && "no POSIX modes on Windows" }, () => {
    chmodSync(keyFile, 0o640);
    assert.throws(() => readKeyFile(keyFile), /group or others/);
    chmodSync(keyFile, 0o604);
    assert.throws(() => readKeyFile(keyFile), /group or others/);
    chmodSync(keyFile, 0o600);
    assert.equal(readKeyFile(keyFile), privateKey);
  });

  it("never falls back to GH_TOKEN or gh for a real run", async () => {
    const withUserToken = { GH_TOKEN: "ghp_" + "u".repeat(36), GITHUB_TOKEN: "ghp_" + "v".repeat(36) };
    await assert.rejects(runToken(false, withUserToken), /triage bot not configured/);
    await assert.rejects(post({ repo: "o/r", token: "", log: () => {}, out: dir, sha: "abc" }), /triage bot not configured/);
    assert.equal(fm.calls.length, 0);
  });

  it("a dry run works without the bot and uses no token", async () => {
    assert.equal(await runToken(true, { GH_TOKEN: "ghp_" + "u".repeat(36) }), "");
    assert.equal(fm.calls.length, 0);
  });
});
