import { describe, expect, it } from "vitest";
import { ServerClock } from "./clock";

// Wall time for the samples: it moves with performance.now() unless a test says otherwise.
const WALL = 1_700_000_000_000;

describe("server clock", () => {
  it("keeps the offset from the fastest round trip", () => {
    const clock = new ServerClock();
    // Server is 5 s ahead. A slow sample (400 ms) is off by its asymmetry; a fast one wins.
    clock.add(1_000, 6_350, 1_400, WALL + 1_400);
    clock.add(2_000, 7_010, 2_020, WALL + 2_020);
    expect(clock.best?.rttMs).toBe(20);
    expect(clock.now(3_000)).toBe(8_000);
    expect(clock.toLocal(8_000)).toBe(3_000);
  });

  it("knows the latest round trip, not just the best", () => {
    const clock = new ServerClock();
    clock.add(1_000, 6_010, 1_020, WALL + 1_020);
    clock.add(2_000, 7_150, 2_300, WALL + 2_300);
    expect(clock.best?.rttMs).toBe(20);
    expect(clock.latest?.rttMs).toBe(300);
  });

  it("drops samples older than a few minutes", () => {
    const clock = new ServerClock();
    // A fast sample with a stale offset, then slower ones 6 minutes on.
    clock.add(1_000, 6_000, 1_010, WALL + 1_010);
    const later = 6 * 60_000;
    clock.add(later, later + 3_100, later + 200, WALL + later + 200);
    expect(clock.best?.rttMs).toBe(200);
    expect(clock.now(later + 1_000)).toBe(later + 1_000 + 3_000);
  });

  it("starts over after a sleep: wall time jumped, performance.now() didn't", () => {
    const clock = new ServerClock();
    clock.add(1_000, 6_000, 1_010, WALL + 1_010);
    expect(clock.synced).toBe(true);
    // Two seconds of performance time, but an hour of wall time: the laptop slept.
    expect(clock.add(3_000, 3_600_000, 3_010, WALL + 3_600_000)).toBeNull();
    expect(clock.synced).toBe(false);
    // The next sample counts again.
    expect(clock.add(4_000, 3_605_000, 4_020, WALL + 3_601_010)).not.toBeNull();
    expect(clock.best?.offsetMs).toBe(3_605_000 - 4_010);
  });

  it("notices a sleep when the tab comes back, and forgets everything on reset", () => {
    const clock = new ServerClock();
    clock.add(1_000, 6_000, 1_010, WALL + 1_010);
    expect(clock.checkSleep(1_500, WALL + 1_500)).toBe(false);
    expect(clock.synced).toBe(true);
    expect(clock.checkSleep(2_000, WALL + 600_000)).toBe(true);
    expect(clock.synced).toBe(false);

    clock.add(3_000, 8_000, 3_010, WALL + 600_000 + 1_010);
    clock.reset();
    expect(clock.synced).toBe(false);
    expect(clock.latest).toBeUndefined();
  });
});
