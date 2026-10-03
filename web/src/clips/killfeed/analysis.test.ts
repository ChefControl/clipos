import { describe, expect, it } from "vitest";
import { activeKill, isMine, jumpTime, type Kill, nextKill, previousKill } from "./analysis";

const kill = (t: number, owner: Kill["owner"] = "myKill"): Kill => ({
  t,
  owner,
  weapon: "ak47",
  modifiers: [],
});

// The "decent 4k" clip: kills at 10, 16, 18 (two), 19, 24, 29 and 34 s.
const kills = [10, 16, 18, 18, 19, 24, 29, 34].map((t) => kill(t));

describe("kill navigation", () => {
  it("jumps a little before the kill, never before the start", () => {
    expect(jumpTime(kill(10))).toBe(8);
    expect(jumpTime(kill(1))).toBe(0);
  });

  it("next kill is the first one ahead of the playhead", () => {
    expect(nextKill(kills, 0)?.t).toBe(10);
    // Just jumped to 16's lead-in (14 s): next is 18, not 16 again.
    expect(nextKill(kills, 14)?.t).toBe(18);
    expect(nextKill(kills, 33)).toBeUndefined();
  });

  it("previous kill skips the one being watched", () => {
    // Watching 24 (row appeared at 24, now 25): previous is 19.
    expect(previousKill(kills, 25)?.t).toBe(19);
    expect(previousKill(kills, 10.5)).toBeUndefined();
  });

  it("the active kill is the latest row on screen, for a few seconds", () => {
    expect(activeKill(kills, 5)).toBeUndefined();
    expect(activeKill(kills, 24.5)?.t).toBe(24);
    expect(activeKill(kills, 40.5)).toBeUndefined();
  });
});

describe("isMine", () => {
  it("is your kills and your deaths, not the others'", () => {
    expect(isMine(kill(1, "myKill"))).toBe(true);
    expect(isMine(kill(1, "myDeath"))).toBe(true);
    expect(isMine(kill(1, "other"))).toBe(false);
  });
});
