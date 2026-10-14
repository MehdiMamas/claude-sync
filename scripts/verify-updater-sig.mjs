// Verifies a Tauri updater signature (.sig) against the updater public key in
// app/src-tauri/tauri.conf.json, the way the app's updater does (minisign, §13).
//
//   node scripts/verify-updater-sig.mjs <file> [<file>.sig] [--pubkey <base64>]
//
// Exit 0 and one "OK ..." line when the signature and its trusted comment check out,
// else exit 1 with the reason. Only Node built-ins (Ed25519, BLAKE2b-512).

import { createHash, createPublicKey, verify } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

function fail(msg) {
  console.error(`verify-updater-sig: ${msg}`);
  process.exit(1);
}

/** The second line of a minisign text block, base64-decoded. */
function keyLine(text, what) {
  const lines = text.split(/\r?\n/).filter((l) => l.length > 0);
  if (lines.length < 2 || !lines[0].startsWith("untrusted comment:")) fail(`${what} is not a minisign block`);
  return { lines, bytes: Buffer.from(lines[1], "base64") };
}

/** minisign shows key ids as the little-endian u64 in upper-case hex. */
function keyIdHex(bytes) {
  return Buffer.from(bytes).reverse().toString("hex").toUpperCase();
}

const args = process.argv.slice(2);
let pubkeyB64;
const files = [];
for (let i = 0; i < args.length; i++) {
  if (args[i] === "--pubkey") pubkeyB64 = args[++i];
  else files.push(args[i]);
}
const [file, sigFile = `${file}.sig`] = files;
if (!file) fail("usage: verify-updater-sig.mjs <file> [<file>.sig] [--pubkey <base64>]");

if (!pubkeyB64) {
  const conf = join(dirname(fileURLToPath(import.meta.url)), "..", "app", "src-tauri", "tauri.conf.json");
  pubkeyB64 = JSON.parse(readFileSync(conf, "utf8")).plugins?.updater?.pubkey;
  if (!pubkeyB64) fail(`no plugins.updater.pubkey in ${conf}`);
}

// Public key: "Ed" + key id (8) + Ed25519 key (32).
const pk = keyLine(Buffer.from(pubkeyB64, "base64").toString("utf8"), "public key").bytes;
if (pk.length !== 42 || pk.toString("latin1", 0, 2) !== "Ed") fail("public key has an unknown format");
const pkId = keyIdHex(pk.subarray(2, 10));
const key = createPublicKey({ key: { kty: "OKP", crv: "Ed25519", x: pk.subarray(10).toString("base64url") }, format: "jwk" });

// Tauri's .sig is the base64 of a minisign signature file.
const sigText = Buffer.from(readFileSync(sigFile, "utf8").trim(), "base64").toString("utf8");
const { lines, bytes: sig } = keyLine(sigText, "signature");
if (sig.length !== 74) fail("signature has an unknown format");
const alg = sig.toString("latin1", 0, 2);
const sigId = keyIdHex(sig.subarray(2, 10));
if (sigId !== pkId) fail(`signed with key ${sigId}, but tauri.conf.json has key ${pkId}`);

const data = readFileSync(file);
// "ED" signs the BLAKE2b-512 of the file (what Tauri writes), "Ed" the file itself.
const message = alg === "ED" ? createHash("blake2b512").update(data).digest() : alg === "Ed" ? data : null;
if (!message) fail(`unknown signature algorithm ${alg}`);
if (!verify(null, message, key, sig.subarray(10))) fail(`${file}: signature does not match the file`);

const trusted = lines[2] ?? "";
if (!trusted.startsWith("trusted comment: ") || !lines[3]) fail("signature has no trusted comment");
const comment = trusted.slice("trusted comment: ".length);
const global = Buffer.from(lines[3], "base64");
if (!verify(null, Buffer.concat([sig.subarray(10), Buffer.from(comment, "utf8")]), key, global)) {
  fail("trusted comment signature does not match");
}
console.log(`OK ${file} (key ${pkId}, ${alg === "ED" ? "prehashed" : "plain"}, ${comment})`);
