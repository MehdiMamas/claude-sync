#!/usr/bin/env node
// Fails when an installer is over the v2 budget of 15 MB (v2-Budgets; 15 * 1024 * 1024 bytes).
//   node scripts/check-installer-size.mjs <installer> [<installer> ...]
import { statSync } from "node:fs";

const LIMIT = 15 * 1024 * 1024;
const files = process.argv.slice(2);
if (files.length === 0) {
  console.error("usage: check-installer-size.mjs <installer>...");
  process.exit(2);
}
let over = false;
for (const f of files) {
  const size = statSync(f).size;
  console.log(`${f}: ${(size / 1024 / 1024).toFixed(2)} MB (limit ${LIMIT / 1024 / 1024} MB)`);
  if (size > LIMIT) over = true;
}
process.exit(over ? 1 : 0);
