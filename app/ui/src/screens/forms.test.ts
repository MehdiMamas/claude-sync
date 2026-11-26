import { expect, test } from "vitest";
import { nameProblem } from "./GroupsForms";
import { initials } from "./Shell";

test("repo names follow GitHub's rules", () => {
  expect(nameProblem("claude-group")).toBeNull();
  expect(nameProblem("team.setup_2")).toBeNull();
  expect(nameProblem("has space")).not.toBeNull();
  expect(nameProblem("..")).not.toBeNull();
  expect(nameProblem("")).not.toBeNull();
});

test("initials", () => {
  expect(initials("alex-host")).toBe("AH");
  expect(initials("sam")).toBe("SA");
});
