// Runs the self-update path locally against staged release assets, no Vercel, no GitHub:
//
//   node dev/local.ts ../dist/release/0.1.1 [--current 0.1.0] [--serve]
//
// Starts a stub GitHub that lists the assets in the folder as the latest release, the
// relay on a local port, then does what the app does: GET /api/update, then the download
// through /api/download/<id>. It checks the signature and the downloaded bytes against
// the files and exits 0 when both match. --serve keeps the servers up afterwards.

import { createHash, randomBytes } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { resetAuthCache } from "../lib/auth.ts";
import { routeGitHubTo, startRelay, startStubGitHub } from "./harness.ts";

const args = process.argv.slice(2);
const dir = args.find((a, i) => !a.startsWith("--") && args[i - 1] !== "--current");
const at = args.indexOf("--current");
const current = at >= 0 ? (args[at + 1] ?? "") : "0.1.0";
const serve = args.includes("--serve");
if (!dir) {
  console.error("usage: node dev/local.ts <dist/release/<version>> [--current 0.1.0] [--serve]");
  process.exit(2);
}

const names = readdirSync(dir).filter((n) => n.startsWith("claude-sync_"));
const version = /^claude-sync_([^_]+)_/.exec(names[0] ?? "")?.[1];
if (!version) {
  console.error(`no claude-sync_<version>_* assets in ${dir}`);
  process.exit(2);
}
const exeName = `claude-sync_${version}_windows_x86_64-setup.exe`;
if (!names.includes(exeName) || !names.includes(`${exeName}.sig`)) {
  console.error(`${dir} has no ${exeName} with its .sig`);
  process.exit(2);
}

// Throwaway tokens: the stub knows them, nothing else does.
const relayToken = `github_pat_local_${randomBytes(16).toString("hex")}`;
const userToken = `gho_local_${randomBytes(16).toString("hex")}`;
Object.assign(process.env, {
  RELAY_GITHUB_TOKEN: relayToken,
  RELEASES_REPO: "MehdiMamas/claude-sync",
  ALLOWED_GITHUB_USERS: "MehdiMamas",
  GROUP_REPOS: "",
});
delete process.env.RELAY_PUBLIC_URL;
resetAuthCache();

const gh = await startStubGitHub({
  relayToken,
  users: { [userToken]: "MehdiMamas" },
  release: {
    repo: "MehdiMamas/claude-sync",
    tag: `v${version}`,
    body: `claude-sync ${version} (local)`,
    published_at: new Date().toISOString(),
    assets: names.map((name) => ({ name, bytes: readFileSync(join(dir, name)) })),
  },
});
const unroute = routeGitHubTo(gh.url);
const relay = await startRelay();
const auth = { authorization: `Bearer ${userToken}` };

let ok = true;
function check(what: string, pass: boolean, detail = ""): void {
  console.log(`${pass ? "PASS" : "FAIL"}  ${what}${detail ? `  ${detail}` : ""}`);
  ok &&= pass;
}

const updateUrl = `${relay.url}/api/update?target=windows&arch=x86_64&current_version=${current}`;
const res = await fetch(updateUrl, { headers: auth });
check(`GET /api/update?target=windows&arch=x86_64&current_version=${current}`, res.status === 200, `-> ${res.status}`);
if (res.status === 200) {
  const body = (await res.json()) as { version: string; url: string; signature: string; notes: string; pub_date: string };
  console.log(JSON.stringify(body, null, 2));
  check("version", body.version === version, body.version);
  check("signature equals the .sig asset", body.signature === readFileSync(join(dir, `${exeName}.sig`), "utf8").trim());
  check("url is a relay download link", /\/api\/download\/\d+$/.test(body.url) && body.url.startsWith(relay.url), body.url);

  const redirect = await fetch(body.url, { headers: auth, redirect: "manual" });
  check("GET /api/download/<id> is a 302 to the asset host", redirect.status === 302 && (redirect.headers.get("location") ?? "").startsWith(gh.objectsUrl), `-> ${redirect.status} ${redirect.headers.get("location")}`);

  const followed = await fetch(body.url, { headers: auth });
  const bytes = Buffer.from(await followed.arrayBuffer());
  const want = createHash("sha256").update(readFileSync(join(dir, exeName))).digest("hex");
  const got = createHash("sha256").update(bytes).digest("hex");
  check("downloaded installer matches the staged file", followed.status === 200 && got === want, `${bytes.length} bytes, sha256 ${got}`);
}
const same = await fetch(`${relay.url}/api/update?target=windows&arch=x86_64&current_version=${version}`, { headers: auth });
check(`GET /api/update on ${version} itself is 204`, same.status === 204, `-> ${same.status}`);

if (serve) {
  console.log(`\nrelay ${relay.url}  stub GitHub ${gh.url}  user token ${userToken}\nCtrl+C to stop.`);
} else {
  unroute();
  await relay.close();
  await gh.close();
  process.exit(ok ? 0 : 1);
}
