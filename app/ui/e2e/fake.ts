import type { Page } from "@playwright/test";

/** What the engine does by itself (a tick, a person approving on GitHub), played from the test. */
export const fake = (page: Page) => ({
  approve: () => page.evaluate(() => window.__fake.approve()),
  end: (k: "expired" | "denied" | "offline") => page.evaluate((x) => window.__fake.end(x), k),
  transferred: (l: string) => page.evaluate((x) => window.__fake.transferred(x), l),
  fixed: () => page.evaluate(() => window.__fake.fixed()),
  release: () => page.evaluate(() => window.__fake.release()),
  opened: () => page.evaluate(() => window.__opened ?? []),
});

declare global {
  interface Window {
    __fake: import("../src/fake").FakeControls;
    __opened?: string[];
  }
}
