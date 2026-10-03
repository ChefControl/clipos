import { describe, expect, it } from "vitest";
import { initials } from "./initials";

describe("initials", () => {
  it("uses first and last word", () => {
    expect(initials("Jane Doe")).toBe("JD");
    expect(initials("  mary jane  watson ")).toBe("MW");
  });

  it("splits handles on separators", () => {
    expect(initials("jane.doe")).toBe("JD");
    expect(initials("s1mple_fan")).toBe("SF");
  });

  it("handles single words and empty input", () => {
    expect(initials("zeus")).toBe("Z");
    expect(initials("")).toBe("?");
  });
});
