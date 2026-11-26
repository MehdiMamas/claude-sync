import { writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { expect, test } from "@playwright/test";
import { diffPercent } from "./diff";
import { frames } from "./frames";

// Each screen at 880x640 against its blueprint frame rendered in the same browser.
const dir = resolve(import.meta.dirname, "../../../docs/blueprint/project");
const MAX = 5; // percent of differing pixels; the aim is close, not perfect

for (const f of frames) {
  test(`screen ${f.frame} matches its frame`, async ({ page, browser }) => {
    await page.goto(f.url);
    await f.steps?.(page);
    await page.waitForTimeout(100);
    const mine = await page.screenshot();

    const other = await browser.newPage({ viewport: { width: 880, height: 640 } });
    await other.goto(`file://${dir}/v2-Screen${f.frame}.dc.html`);
    await other.waitForTimeout(150);
    const frame = await other.screenshot();
    const pct = await diffPercent(other, mine, frame);
    await other.close();
    console.log(`PIXELDIFF ${f.frame} ${pct.toFixed(2)}%`);
    if (process.env.SHOTS) writeFileSync(`${process.env.SHOTS}/${f.frame}-app.png`, mine);
    if (process.env.SHOTS) writeFileSync(`${process.env.SHOTS}/${f.frame}-frame.png`, frame);
    expect(pct).toBeLessThan(MAX);
  });
}
