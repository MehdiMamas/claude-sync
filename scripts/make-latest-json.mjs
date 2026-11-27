// Writes latest.json, the static file tauri-plugin-updater reads from
// https://github.com/<repo>/releases/latest/download/latest.json (docs/releasing.md).
//
//   node scripts/make-latest-json.mjs <assets-dir> <version> <repo> [notes]
//
// Platforms: windows-x86_64 (setup.exe), darwin-aarch64 and darwin-x86_64 (both the universal
// .app.tar.gz), linux-x86_64 (AppImage). Each signature is the content of the asset's .sig.
// There is no linux-aarch64: the command line there reads only `version`. Names are the ones in
// relay/lib/assets.ts.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export function latest(dir, version, repo, notes = "") {
  const base = `https://github.com/${repo}/releases/download/v${version}`;
  const entry = (name) => ({
    url: `${base}/${name}`,
    signature: readFileSync(join(dir, `${name}.sig`), "utf8").trim(),
  });
  const mac = entry(`claude-sync_${version}_darwin_universal.app.tar.gz`);
  return {
    version,
    notes,
    pub_date: new Date().toISOString(),
    platforms: {
      "windows-x86_64": entry(`claude-sync_${version}_windows_x86_64-setup.exe`),
      "darwin-aarch64": mac,
      "darwin-x86_64": mac,
      "linux-x86_64": entry(`claude-sync_${version}_linux_x86_64.AppImage`),
    },
  };
}

if (process.argv[1]?.endsWith("make-latest-json.mjs")) {
  const [dir, version, repo, notes] = process.argv.slice(2);
  if (!dir || !version || !repo) {
    console.error("usage: make-latest-json.mjs <assets-dir> <version> <repo> [notes]");
    process.exit(2);
  }
  writeFileSync(join(dir, "latest.json"), `${JSON.stringify(latest(dir, version, repo, notes), null, 2)}\n`);
}
