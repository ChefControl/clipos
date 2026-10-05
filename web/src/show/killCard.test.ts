import { describe, expect, it } from "vitest";
import type { ClipAnalysis } from "../clips/killfeed/analysis";
import { killCard } from "./killCard";

const analysis = (myKills: number, deathAt?: number): ClipAnalysis => ({
  status: "done",
  stats: {
    kills: myKills,
    myKills,
    myDeaths: deathAt == null ? 0 : 1,
    multiKill: null,
    weapons: {},
    modifiers: {},
  },
  kills: [
    { t: 3, owner: "myKill", weapon: "ak47", modifiers: [] },
    ...(deathAt == null
      ? []
      : [{ t: deathAt, owner: "myDeath" as const, weapon: "awp", modifiers: [] }]),
  ],
});

describe("killCard", () => {
  it("comes up when the uploader dies, for 5 s", () => {
    expect(killCard(analysis(2, 12), 40_000)).toEqual({
      kills: 2,
      fromMs: 12_000,
      untilMs: 17_000,
    });
  });

  it("without a death, takes the clip's last 5 s", () => {
    expect(killCard(analysis(4), 40_000)).toEqual({ kills: 4, fromMs: 35_000, untilMs: 40_000 });
  });

  it("never runs past the end or before the start, and counts to an ace at most", () => {
    expect(killCard(analysis(7, 38), 40_000)).toEqual({
      kills: 5,
      fromMs: 38_000,
      untilMs: 40_000,
    });
    expect(killCard(analysis(1), 3_000)).toEqual({ kills: 1, fromMs: 0, untilMs: 3_000 });
  });

  it("needs a killfeed and a length", () => {
    expect(killCard(null, 40_000)).toBeNull();
    expect(killCard({ status: "pending", stats: null, kills: [] }, 40_000)).toBeNull();
    expect(killCard(analysis(2), null)).toBeNull();
  });
});
