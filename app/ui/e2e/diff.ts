import type { Page } from "@playwright/test";

/** Percent of pixels whose colour differs by more than 24/255 in any channel. Runs in the browser, no image library. */
export async function diffPercent(page: Page, a: Buffer, b: Buffer): Promise<number> {
  return page.evaluate(
    async ([x, y]) => {
      const load = async (s: string) => {
        const img = new Image();
        img.src = `data:image/png;base64,${s}`;
        await img.decode();
        const c = document.createElement("canvas");
        c.width = img.width;
        c.height = img.height;
        const g = c.getContext("2d");
        if (!g) throw new Error("no canvas");
        g.drawImage(img, 0, 0);
        return g.getImageData(0, 0, c.width, c.height).data;
      };
      const [p, q] = [await load(x as string), await load(y as string)];
      let bad = 0;
      for (let i = 0; i < p.length; i += 4)
        if (
          Math.abs((p[i] ?? 0) - (q[i] ?? 0)) > 24 ||
          Math.abs((p[i + 1] ?? 0) - (q[i + 1] ?? 0)) > 24 ||
          Math.abs((p[i + 2] ?? 0) - (q[i + 2] ?? 0)) > 24
        )
          bad++;
      return (bad / (p.length / 4)) * 100;
    },
    [a.toString("base64"), b.toString("base64")],
  );
}
