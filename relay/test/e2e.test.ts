// The self-update path over real HTTP (#27): the app's request to /api/update, then its
// download through /api/download/<id> to the asset host, against a stub GitHub.

import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";
import { FIRST_ASSET_ID, type Running, routeGitHubTo, startRelay, startStubGitHub, type StubGitHub } from "../dev/harness.ts";
import { resetAuthCache } from "../lib/auth.ts";
import { RELAY_TOKEN, setEnv, USER_TOKEN } from "./helpers.ts";

const VERSION = "0.3.1";
const EXE = `claude-sync_${VERSION}_windows_x86_64-setup.exe`;
const INSTALLER = new TextEncoder().encode("MZ fake installer bytes");
const SIG = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXk=";

let gh: StubGitHub;
let relay: Running;
let unroute: () => void;

before(async () => {
  setEnv();
  resetAuthCache();
  gh = await startStubGitHub({
    relayToken: RELAY_TOKEN,
    users: { [USER_TOKEN]: "MehdiMamas" },
    release: {
      repo: "MehdiMamas/claude-sync",
      tag: `v${VERSION}`,
      body: "Local release.",
      published_at: "2026-10-13T12:00:00Z",
      assets: [
        { name: EXE, bytes: INSTALLER },
        { name: `${EXE}.sig`, bytes: new TextEncoder().encode(`${SIG}\n`) },
      ],
    },
  });
  unroute = routeGitHubTo(gh.url);
  relay = await startRelay();
});

after(async () => {
  unroute();
  await relay.close();
  await gh.close();
});

const auth = { authorization: `Bearer ${USER_TOKEN}` };

describe("self-update over HTTP", () => {
  it("answers the app's update check with the updater JSON", async () => {
    const res = await fetch(`${relay.url}/api/update?target=windows&arch=x86_64&current_version=0.1.0`, { headers: auth });
    assert.equal(res.status, 200);
    assert.deepEqual(await res.json(), {
      version: VERSION,
      notes: "Local release.",
      pub_date: "2026-10-13T12:00:00Z",
      url: `${relay.url}/api/download/${FIRST_ASSET_ID}`,
      signature: SIG,
    });
    // The .sig came through GitHub's redirect to the asset host, without the relay token.
    assert.ok(gh.log.includes(`objects GET /assets/${FIRST_ASSET_ID + 1}?sig=stub`));
  });

  it("302s the download to the asset host, which serves the installer without auth", async () => {
    const res = await fetch(`${relay.url}/api/download/${FIRST_ASSET_ID}`, { headers: auth, redirect: "manual" });
    assert.equal(res.status, 302);
    const location = res.headers.get("location") ?? "";
    assert.equal(location, `${gh.objectsUrl}/assets/${FIRST_ASSET_ID}?sig=stub`);

    // What the updater does next: follow the redirect. fetch drops the token across origins.
    const followed = await fetch(`${relay.url}/api/download/${FIRST_ASSET_ID}`, { headers: auth });
    assert.equal(followed.status, 200);
    assert.deepEqual(new Uint8Array(await followed.arrayBuffer()), INSTALLER);
  });

  it("204 when the app is already on the release", async () => {
    const res = await fetch(`${relay.url}/api/update?target=windows&arch=x86_64&current_version=${VERSION}`, { headers: auth });
    assert.equal(res.status, 204);
  });

  it("204 for macOS until its bundle is uploaded", async () => {
    const res = await fetch(`${relay.url}/api/update?target=darwin&arch=aarch64&current_version=0.1.0`, { headers: auth });
    assert.equal(res.status, 204);
  });

  it("401 without the user's token", async () => {
    const res = await fetch(`${relay.url}/api/update?target=windows&arch=x86_64&current_version=0.1.0`);
    assert.equal(res.status, 401);
  });
});
