import { describe, expect, it } from "vitest";
import { FULL, readLevel, toggled } from "./Volume";

const storing = (value: string | null) => ({ getItem: () => value });

describe("readLevel", () => {
  it("is full volume with nothing kept, or storage out of reach", () => {
    expect(readLevel(storing(null))).toEqual(FULL);
    expect(readLevel(null)).toEqual(FULL);
    expect(
      readLevel({
        getItem: () => {
          throw new Error("blocked");
        },
      }),
    ).toEqual(FULL);
  });

  it("is what was kept, kept between 0 and 1", () => {
    expect(readLevel(storing('{"volume":0.3,"muted":true}'))).toEqual({
      volume: 0.3,
      muted: true,
    });
    expect(readLevel(storing('{"volume":7}'))).toEqual({ volume: 1, muted: false });
    expect(readLevel(storing('{"volume":-1,"muted":"yes"}'))).toEqual({
      volume: 0,
      muted: false,
    });
  });

  it("ignores anything else", () => {
    expect(readLevel(storing("not json"))).toEqual(FULL);
    expect(readLevel(storing('{"volume":"loud"}'))).toEqual(FULL);
  });
});

describe("toggled", () => {
  it("mutes, and unmutes at the same volume", () => {
    expect(toggled({ volume: 0.4, muted: false })).toEqual({ volume: 0.4, muted: true });
    expect(toggled({ volume: 0.4, muted: true })).toEqual({ volume: 0.4, muted: false });
  });

  it("brings a silent slider back at half", () => {
    expect(toggled({ volume: 0, muted: false })).toEqual({ volume: 0.5, muted: false });
  });
});
