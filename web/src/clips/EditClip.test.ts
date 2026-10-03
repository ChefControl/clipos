import { describe, expect, it } from "vitest";
import { parseTags } from "./EditClip";

describe("parseTags", () => {
  it("splits on commas and drops blanks", () => {
    expect(parseTags("ace, 1v3 clutch ,, smoke")).toEqual(["ace", "1v3 clutch", "smoke"]);
    expect(parseTags("")).toEqual([]);
  });
});
