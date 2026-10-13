import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, it } from "node:test";
import { GET as download } from "../api/download/[id].ts";
import { GET as update } from "../api/update.ts";
import { resetAuthCache } from "../lib/auth.ts";
import { type FetchMock, mockFetch, mockUser, RELAY_TOKEN, req, setEnv } from "./helpers.ts";

const REPO = "/repos/MehdiMamas/claude-sync";
const SIG_WIN = "dW50cnVzdGVkIGNvbW1lbnQ6IHdpbmRvd3M=";
const SIG_MAC = "dW50cnVzdGVkIGNvbW1lbnQ6IG1hY29z";

function release(tag: string, names: string[]) {
  return {
    tag_name: tag,
    name: `claude-sync ${tag}`,
    body: "Fixes and polish.",
    draft: false,
    prerelease: false,
    published_at: "2026-10-12T10:00:00Z",
    assets: names.map((name, i) => ({ id: 100 + i, name, size: name.endsWith(".sig") ? 400 : 5_000_000 })),
  };
}

// Asset ids are 100 + index.
const WIN_EXE = "claude-sync_0.3.0_windows_x86_64-setup.exe";
const FULL = [
  WIN_EXE,
  `${WIN_EXE}.sig`,
  "claude-sync_0.3.0_darwin_universal.app.tar.gz",
  "claude-sync_0.3.0_darwin_universal.app.tar.gz.sig",
  "claude-sync_0.3.0_darwin_universal.dmg",
];

function updateReq(target: string, arch: string, current: string): Request {
  return req(`/api/update?target=${target}&arch=${arch}&current_version=${current}`);
}

let fm: FetchMock;

beforeEach(() => {
  setEnv();
  resetAuthCache();
  fm = mockFetch();
  mockUser(fm, "MehdiMamas");
  fm.on("GET", `${REPO}/releases/latest`, release("v0.3.0", FULL));
  fm.on("GET", `${REPO}/releases/assets/101`, new Response(SIG_WIN + "\n"));
  fm.on("GET", `${REPO}/releases/assets/103`, new Response(SIG_MAC));
});

afterEach(() => fm.restore());

describe("GET /api/update", () => {
  it("returns the Tauri updater JSON when the release is newer", async () => {
    const res = await update(updateReq("windows", "x86_64", "0.2.9"));
    assert.equal(res.status, 200);
    assert.deepEqual(await res.json(), {
      version: "0.3.0",
      notes: "Fixes and polish.",
      pub_date: "2026-10-12T10:00:00Z",
      url: "https://relay.example.test/api/download/100",
      signature: SIG_WIN,
    });
    const sig = fm.calls.find((c) => c.url.endsWith("/releases/assets/101"));
    assert.equal(sig?.headers.accept, "application/octet-stream");
    assert.equal(sig?.headers.authorization, `Bearer ${RELAY_TOKEN}`);
    const latest = fm.calls.find((c) => c.url.endsWith("/releases/latest"));
    assert.equal(latest?.headers.authorization, `Bearer ${RELAY_TOKEN}`);
  });

  it("serves the universal macOS bundle to both darwin arches", async () => {
    for (const arch of ["aarch64", "x86_64"]) {
      const res = await update(updateReq("darwin", arch, "0.2.0"));
      assert.equal(res.status, 200);
      const body = (await res.json()) as { url: string; signature: string };
      assert.equal(body.url, "https://relay.example.test/api/download/102");
      assert.equal(body.signature, SIG_MAC);
    }
  });

  it("uses RELAY_PUBLIC_URL for the download url", async () => {
    setEnv({ RELAY_PUBLIC_URL: "https://claude-sync-relay.vercel.app/" });
    const res = await update(updateReq("windows", "x86_64", "0.1.0"));
    assert.equal(((await res.json()) as { url: string }).url, "https://claude-sync-relay.vercel.app/api/download/100");
  });

  it("204 when the current version is the same", async () => {
    assert.equal((await update(updateReq("windows", "x86_64", "0.3.0"))).status, 204);
  });

  it("204 when the current version is newer", async () => {
    assert.equal((await update(updateReq("windows", "x86_64", "0.4.0-beta.1"))).status, 204);
  });

  it("treats a prerelease of the same version as older", async () => {
    assert.equal((await update(updateReq("windows", "x86_64", "0.3.0-rc.1"))).status, 200);
  });

  it("204 when the platform's bundle is missing (macOS build not uploaded yet)", async () => {
    fm.on("GET", `${REPO}/releases/latest`, release("v0.3.0", FULL.slice(0, 2)));
    assert.equal((await update(updateReq("darwin", "aarch64", "0.2.0"))).status, 204);
    assert.equal(fm.calls.filter((c) => c.url.includes("/releases/assets/")).length, 0);
  });

  it("204 when the bundle has no .sig", async () => {
    fm.on("GET", `${REPO}/releases/latest`, release("v0.3.0", [WIN_EXE]));
    assert.equal((await update(updateReq("windows", "x86_64", "0.2.0"))).status, 204);
  });

  it("204 for a Windows arch with no build", async () => {
    assert.equal((await update(updateReq("windows", "aarch64", "0.2.0"))).status, 204);
  });

  it("204 when there is no release yet", async () => {
    fm.on("GET", `${REPO}/releases/latest`, 404);
    assert.equal((await update(updateReq("windows", "x86_64", "0.2.0"))).status, 204);
  });

  it("400 for bad parameters", async () => {
    for (const q of [
      "target=linux&arch=x86_64&current_version=1.0.0",
      "target=windows&arch=arm&current_version=1.0.0",
      "target=windows&arch=x86_64&current_version=latest",
      "target=windows&arch=x86_64",
    ]) {
      assert.equal((await update(req(`/api/update?${q}`))).status, 400, q);
    }
  });

  it("401 without a token, before touching releases", async () => {
    const res = await update(req("/api/update?target=windows&arch=x86_64&current_version=0.1.0", { token: null }));
    assert.equal(res.status, 401);
    assert.equal(fm.calls.length, 0);
  });

  it("403 for a caller who is not allowed", async () => {
    mockUser(fm, "stranger");
    fm.on("GET", "/repos/MehdiMamas/claude-sync-data/collaborators/stranger", 404);
    assert.equal((await update(updateReq("windows", "x86_64", "0.1.0"))).status, 403);
    assert.equal(fm.calls.filter((c) => c.url.includes("/releases")).length, 0);
  });

  it("502 when GitHub fails", async () => {
    fm.on("GET", `${REPO}/releases/latest`, 500);
    assert.equal((await update(updateReq("windows", "x86_64", "0.1.0"))).status, 502);
  });
});

describe("GET /api/download/<id>", () => {
  const signed = "https://release-assets.githubusercontent.com/github-production-release-asset/1/abc?sig=xyz";

  it("302s to GitHub's signed URL without following it", async () => {
    fm.on("GET", `${REPO}/releases/assets/100`, new Response(null, { status: 302, headers: { location: signed } }));
    const res = await download(req("/api/download/100"));
    assert.equal(res.status, 302);
    assert.equal(res.headers.get("location"), signed);
    const call = fm.calls.at(-1);
    assert.equal(call?.redirect, "manual");
    assert.equal(call?.headers.accept, "application/octet-stream");
    assert.equal(call?.headers.authorization, `Bearer ${RELAY_TOKEN}`);
  });

  it("404 for an asset that is not in RELEASES_REPO", async () => {
    fm.on("GET", `${REPO}/releases/assets/999`, 404);
    assert.equal((await download(req("/api/download/999"))).status, 404);
  });

  it("400 for an id that is not a number", async () => {
    assert.equal((await download(req("/api/download/..%2F..%2Fuser"))).status, 400);
    assert.equal((await download(req("/api/download/abc"))).status, 400);
  });

  it("502 rather than proxying bytes when GitHub does not redirect", async () => {
    fm.on("GET", `${REPO}/releases/assets/100`, new Response("bytes", { status: 200 }));
    assert.equal((await download(req("/api/download/100"))).status, 502);
  });

  it("401 without a token", async () => {
    const res = await download(req("/api/download/100", { token: null }));
    assert.equal(res.status, 401);
    assert.equal(fm.calls.length, 0);
  });
});
