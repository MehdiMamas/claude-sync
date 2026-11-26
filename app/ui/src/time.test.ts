import { expect, test } from "vitest";
import { ago } from "./time";

test("ago", () => {
  expect(ago(100, 130)).toBe("just now");
  expect(ago(0, 120)).toBe("2 min ago");
  expect(ago(0, 7200)).toBe("2 h ago");
  expect(ago(0, 86400)).toBe("1 day ago");
});
