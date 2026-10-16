#!/usr/bin/env node
// Sets or checks the app version in every file it lives in (docs/releasing.md).
//
//   node scripts/bump-version.mjs 0.2.0            bump every file to 0.2.0 (must be greater)
//   node scripts/bump-version.mjs --check 0.2.0    exit 1 unless every file says 0.2.0
//   node scripts/bump-version.mjs --print          print the current version
//
// The file list and patterns are the same as $VersionFiles in scripts/release.ps1: keep them
// in step. The app and the core take their version from [workspace.package]
// (version.workspace = true) and tauri.conf.json has none, so Tauri uses the Cargo one.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

const FILES = [
  { path: "Cargo.toml", re: /(^\[workspace\.package\][^[]*?^version = ")([^"]+)(")/m },
  { path: "Cargo.lock", re: /(^name = "claude-sync-app"\r?\nversion = ")([^"]+)(")/m },
  { path: "Cargo.lock", re: /(^name = "claude-sync-core"\r?\nversion = ")([^"]+)(")/m },
  { path: "app/package.json", re: /(^ {2}"version": ")([^"]+)(")/m },
  { path: "app/ui/package.json", re: /(^ {2}"version": ")([^"]+)(")/m },
];

const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?$/;

function die(msg) {
  console.error(`bump-version: ${msg}`);
  process.exit(1);
}

function parse(v) {
  const m = SEMVER.exec(v);
  return m && { nums: [m[1], m[2], m[3]].map(BigInt), pre: m[4] ?? null };
}

// Negative, zero or positive, like relay/lib/semver.ts (a prerelease sorts before its release).
function compare(a, b) {
  for (let i = 0; i < 3; i++) if (a.nums[i] !== b.nums[i]) return a.nums[i] < b.nums[i] ? -1 : 1;
  if (a.pre === b.pre) return 0;
  if (a.pre === null) return 1;
  if (b.pre === null) return -1;
  const pa = a.pre.split("."), pb = b.pre.split(".");
  for (let i = 0; i < Math.min(pa.length, pb.length); i++) {
    const na = /^\d+$/.test(pa[i]), nb = /^\d+$/.test(pb[i]);
    let c;
    if (na && nb) c = BigInt(pa[i]) < BigInt(pb[i]) ? -1 : BigInt(pa[i]) > BigInt(pb[i]) ? 1 : 0;
    else if (na) c = -1;
    else if (nb) c = 1;
    else c = pa[i] < pb[i] ? -1 : pa[i] > pb[i] ? 1 : 0;
    if (c !== 0) return c;
  }
  return Math.sign(pa.length - pb.length);
}

const read = (rel) => readFileSync(join(root, rel), "utf8");

function current() {
  return FILES.map((f) => {
    const m = f.re.exec(read(f.path));
    if (!m) die(`no version found in ${f.path} (pattern ${f.re})`);
    return { path: f.path, version: m[2] };
  });
}

function single() {
  const found = current();
  const distinct = [...new Set(found.map((f) => f.version))];
  if (distinct.length !== 1) die(`the version files disagree: ${found.map((f) => `${f.path}=${f.version}`).join(", ")}`);
  return distinct[0];
}

const args = process.argv.slice(2);
if (args[0] === "--print" && args.length === 1) {
  console.log(single());
} else if (args[0] === "--check" && args.length === 2) {
  const want = args[1].replace(/^v/, "");
  const have = single();
  if (have !== want) die(`the version files say ${have}, not ${want}`);
  console.log(`version ${have} in ${FILES.length} places`);
} else if (args.length === 1 && !args[0].startsWith("-")) {
  const next = args[0];
  const target = parse(next);
  if (!target) die(`version must be semver like 0.2.0 or 0.2.0-rc.1 (no leading v), got '${next}'`);
  const have = single();
  if (compare(target, parse(have)) <= 0) die(`${next} is not greater than the current version ${have}`);
  for (const path of new Set(FILES.map((f) => f.path))) {
    let text = read(path);
    for (const f of FILES.filter((f) => f.path === path)) text = text.replace(f.re, (_, a, _v, c) => a + next + c);
    writeFileSync(join(root, path), text);
  }
  if (single() !== next) die("bump missed a file");
  console.log(`version ${have} -> ${next}: ${[...new Set(FILES.map((f) => f.path))].join(", ")}`);
} else {
  die("usage: node scripts/bump-version.mjs <version> | --check <version> | --print");
}
