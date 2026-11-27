import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { expect, test } from "@playwright/test";
import { frames } from "./frames";

// Step 8 check: every v2 screen frame is played against its screen and every v2 scenario frame
// has a spec. A new frame file without an entry fails here.
const dir = resolve(import.meta.dirname, "../../../docs/blueprint/project");
const names = (prefix: string) =>
  readdirSync(dir)
    .filter((f) => f.startsWith(prefix) && f.endsWith(".dc.html"))
    .map((f) => f.slice(prefix.length, -".dc.html".length));

test("every v2-Screen frame is in frames.ts", () => {
  const have = new Set(frames.map((f) => f.frame));
  expect(names("v2-Screen").filter((n) => !have.has(n))).toEqual([]);
  const files = new Set(names("v2-Screen"));
  expect([...have].filter((n) => !files.has(n))).toEqual([]);
});

test("every v2-Scenario frame has a spec that names it", () => {
  const specs = readdirSync(import.meta.dirname)
    .filter((f) => f.startsWith("scenario-") && f.endsWith(".spec.ts"))
    .map((f) => readFileSync(resolve(import.meta.dirname, f), "utf8"))
    .join("\n");
  expect(names("v2-Scenario").filter((n) => !specs.includes(`v2-Scenario${n}`))).toEqual([]);
});
