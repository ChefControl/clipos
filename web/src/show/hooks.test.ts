import { describe, expect, it } from "vitest";
import { moveTo } from "./hooks";

describe("moveTo", () => {
  it("moves a clip up or down the lineup", () => {
    expect(moveTo(["a", "b", "c"], "a", 2)).toEqual(["b", "c", "a"]);
    expect(moveTo(["a", "b", "c"], "c", 0)).toEqual(["c", "a", "b"]);
    expect(moveTo(["a", "b", "c"], "b", 1)).toEqual(["a", "b", "c"]);
  });

  it("stops at the ends", () => {
    expect(moveTo(["a", "b"], "a", 9)).toEqual(["b", "a"]);
    expect(moveTo(["a", "b"], "b", -3)).toEqual(["b", "a"]);
  });
});
