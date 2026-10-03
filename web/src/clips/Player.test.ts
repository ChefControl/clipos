import { describe, expect, it } from "vitest";
import type { Kill } from "./killfeed/analysis";
import { killLabel, nearestKill, stepSpeed } from "./Player";

describe("stepSpeed", () => {
  it("steps through the speeds and clamps at the ends", () => {
    expect(stepSpeed(1, 1)).toBe(1.5);
    expect(stepSpeed(1, -1)).toBe(0.5);
    expect(stepSpeed(2, 1)).toBe(2);
    expect(stepSpeed(0.25, -1)).toBe(0.25);
  });

  it("snaps an off-list speed to the next step", () => {
    expect(stepSpeed(1.25, 1)).toBe(2);
    expect(stepSpeed(3, -1)).toBe(1.5);
  });
});

const kill = (t: number, owner: Kill["owner"], weapon: string | null = "ak47"): Kill => ({
  t,
  owner,
  weapon,
  modifiers: [],
});

describe("nearestKill", () => {
  // 100 s on a 1000 px time range: 10 px a second.
  const kills = [kill(10, "other"), kill(30, "other"), kill(30, "myKill"), kill(32, "myDeath")];

  it("finds the kill within 10 px of the pointer", () => {
    expect(nearestKill(kills, 100, 1000, 105)).toBe(kills[0]);
    expect(nearestKill(kills, 100, 1000, 92)).toBe(kills[0]);
    expect(nearestKill(kills, 100, 1000, 115)).toBeNull();
  });

  it("prefers yours where two share a spot, else the closer one", () => {
    expect(nearestKill(kills, 100, 1000, 300)).toBe(kills[2]);
    expect(nearestKill(kills, 100, 1000, 312)).toBe(kills[3]);
    expect(nearestKill(kills, 100, 1000, 309)).toBe(kills[2]);
  });

  it("is nothing before the video's length or the range's width is known", () => {
    expect(nearestKill(kills, 0, 1000, 100)).toBeNull();
    expect(nearestKill(kills, 100, 0, 0)).toBeNull();
    expect(nearestKill([], 100, 1000, 100)).toBeNull();
  });
});

describe("killLabel", () => {
  it("names the gun and how", () => {
    expect(killLabel({ ...kill(1, "myKill"), modifiers: ["headshot", "through_smoke"] })).toBe(
      "AK-47 · headshot · through smoke",
    );
    expect(killLabel(kill(1, "other", "knife_new_one"))).toBe("Knife");
    expect(killLabel({ ...kill(1, "other", null), modifiers: ["some_new_thing"] })).toBe(
      "Unknown weapon · some new thing",
    );
  });
});
