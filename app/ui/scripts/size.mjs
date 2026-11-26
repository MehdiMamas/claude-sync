// Fails when the UI JavaScript in dist/ is over 200 KB gzipped (v2 budget).
import { readdirSync, readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";

const LIMIT = 200 * 1024;
const dir = new URL("../dist/assets/", import.meta.url);
let total = 0;
for (const f of readdirSync(dir).filter((f) => f.endsWith(".js"))) {
  total += gzipSync(readFileSync(new URL(f, dir))).length;
}
console.log(`UI JS: ${(total / 1024).toFixed(1)} KB gzipped (limit ${LIMIT / 1024} KB)`);
process.exit(total > LIMIT ? 1 : 0);
