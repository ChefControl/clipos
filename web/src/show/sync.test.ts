import { describe, expect, it } from "vitest";
import {
  correction,
  isSafari,
  type LiveState,
  type Player,
  SYNC,
  SyncEngine,
  targetMs,
} from "./sync";

const state = (over: Partial<LiveState> = {}): LiveState => ({
  seq: 1,
  clipId: "c",
  playing: true,
  positionMs: 0,
  atServerMs: 10_000,
  rate: 1,
  durationMs: 60_000,
  ...over,
});

class FakePlayer implements Player {
  currentTime = 0;
  playbackRate = 1;
  paused = true;
  ended = false;
  duration = Number.NaN;
  readyState = 4;
  /** What play() fails with, if anything. */
  refuse: string | null = null;
  plays = 0;
  async play() {
    this.plays += 1;
    // play() on an ended video starts it again from 0, as a browser does.
    if (this.ended) {
      this.currentTime = 0;
      this.ended = false;
    }
    if (this.refuse) throw new DOMException("refused", this.refuse);
    this.paused = false;
  }
  pause() {
    this.paused = true;
  }
  /** Plays for `ms` of wall time at its own rate. */
  advance(ms: number) {
    if (!this.paused) this.currentTime += (ms / 1000) * this.playbackRate;
  }
}

describe("where the clip should be", () => {
  it("holds before a scheduled start, runs after, stops at the end", () => {
    expect(targetMs(state(), 9_000)).toBe(0);
    expect(targetMs(state(), 12_500)).toBe(2_500);
    expect(targetMs(state(), 100_000)).toBe(60_000);
    expect(targetMs(state({ playing: false, positionMs: 700 }), 50_000)).toBe(700);
  });
});

describe("corrections", () => {
  const at = 10_000;
  it("leaves small drift alone, nudges the rate, jumps when far off", () => {
    const opts = { correcting: false, safari: false };
    expect(correction(250, at, opts)).toEqual({ kind: "none" });
    // 500 ms ahead: 5% slower.
    expect(correction(500, at, opts)).toEqual({ kind: "rate", rate: 0.95 });
    // 800 ms behind: 8% faster.
    const behind = correction(-800, at, opts);
    expect(behind.kind).toBe("rate");
    expect(behind.kind === "rate" && behind.rate).toBeCloseTo(1.08);
    expect(correction(1_500, at, opts)).toEqual({ kind: "seek", toMs: at + SYNC.seekLeadMs });
  });
  it("keeps correcting until back within the exit threshold", () => {
    expect(correction(200, 0, { correcting: true, safari: false }).kind).toBe("rate");
    expect(correction(80, 0, { correcting: true, safari: false }).kind).toBe("none");
  });
  it("only jumps on Safari, with a wider dead zone", () => {
    expect(correction(450, 0, { correcting: false, safari: true }).kind).toBe("none");
    expect(correction(700, 0, { correcting: false, safari: true }).kind).toBe("seek");
    expect(isSafari("Mozilla/5.0 (Macintosh) AppleWebKit/605 Version/18.0 Safari/605.1.15")).toBe(
      true,
    );
    expect(isSafari("Mozilla/5.0 AppleWebKit/537.36 Chrome/140.0 Safari/537.36")).toBe(false);
  });
});

describe("the engine", () => {
  it("waits for the scheduled start, then plays in step", () => {
    const p = new FakePlayer();
    const e = new SyncEngine(p);
    expect(e.step(state(), 9_800, 0)).toBe("starting");
    expect(p.paused).toBe(true);
    expect(e.step(state(), 10_000, 0)).toBe("catchingUp");
    expect(p.paused).toBe(false);
    // Let it settle, play along.
    p.advance(2_000);
    expect(e.step(state(), 12_000, 2_000)).toBe("synced");
    expect(Math.abs(e.driftMs)).toBeLessThan(1);
  });

  it("catches up after falling behind, then goes back to normal speed", () => {
    const p = new FakePlayer();
    const e = new SyncEngine(p);
    e.step(state(), 10_000, 0);
    p.advance(2_000);
    e.step(state(), 12_000, 2_000);
    // A stall: 600 ms behind.
    p.currentTime -= 0.6;
    expect(e.step(state(), 12_000, 2_000)).toBe("catchingUp");
    expect(p.playbackRate).toBeGreaterThan(1);
    let t = 2_000;
    // From 600 ms behind: about 12 s at 3–6 % faster.
    for (let i = 0; i < 80 && e.status !== "synced"; i++) {
      p.advance(250);
      t += 250;
      e.step(state(), 10_000 + t, t);
    }
    expect(e.status).toBe("synced");
    expect(p.playbackRate).toBe(1);
    expect(Math.abs(e.driftMs)).toBeLessThan(SYNC.exitMs);
  });

  it("jumps when far off and lets the seek settle", () => {
    const p = new FakePlayer();
    const e = new SyncEngine(p);
    e.step(state(), 10_000, 0);
    p.advance(2_000);
    e.step(state(), 12_000, 2_000);
    p.currentTime += 3;
    expect(e.step(state(), 12_000, 2_000)).toBe("catchingUp");
    expect(p.currentTime).toBeCloseTo(2 + SYNC.seekLeadMs / 1000);
    // Settling: no second jump for a moment, while it's no further off than before.
    p.currentTime = 4.1;
    expect(e.step(state(), 12_100, 2_100)).toBe("catchingUp");
    expect(p.currentTime).toBe(4.1);
    // Further off than before the first jump: jump again now.
    p.currentTime = 20;
    e.step(state(), 12_200, 2_200);
    expect(p.currentTime).toBeCloseTo(2.2 + SYNC.seekLeadMs / 1000);
  });

  it("leaves a buffering player alone until it plays on", () => {
    const p = new FakePlayer();
    const e = new SyncEngine(p);
    e.step(state(), 10_000, 0);
    p.advance(2_000);
    e.step(state(), 12_000, 2_000);
    // Stalled 2 s behind, out of data: no jump, no nudge.
    p.readyState = 2;
    expect(e.step(state(), 14_000, 4_000)).toBe("catchingUp");
    expect(p.currentTime).toBe(2);
    expect(p.playbackRate).toBe(1);
    // Data again, but between 'waiting' and 'playing': still hands off.
    p.readyState = 4;
    e.setWaiting(true);
    e.step(state(), 14_000, 4_000);
    expect(p.currentTime).toBe(2);
    // Playing: now it jumps.
    e.setWaiting(false);
    e.step(state(), 14_000, 4_000);
    expect(p.currentTime).toBeCloseTo(4 + SYNC.seekLeadMs / 1000);
  });

  it("holds a paused show on its spot and stops at the end", () => {
    const p = new FakePlayer();
    const e = new SyncEngine(p);
    p.paused = false;
    expect(e.step(state({ playing: false, positionMs: 4_000 }), 50_000, 0)).toBe("synced");
    expect(p.paused).toBe(true);
    expect(p.currentTime).toBe(4);
    p.paused = false;
    expect(e.step(state(), 100_000, 0)).toBe("synced");
    expect(p.paused).toBe(true);
  });

  it("with no clip loaded, stops the player and waits for the host", () => {
    const p = new FakePlayer();
    const e = new SyncEngine(p);
    p.paused = false;
    expect(e.step(null, 50_000, 0)).toBe("idle");
    expect(p.paused).toBe(true);
    p.paused = false;
    expect(e.step(state({ clipId: null }), 50_000, 0)).toBe("idle");
    expect(p.paused).toBe(true);
    // Already stopped: nothing to do.
    expect(e.step(null, 60_000, 0)).toBe("idle");
    expect(e.status).toBe("idle");
    expect(p.plays).toBe(0);
  });

  it("reports a refused play once, until a click unblocks it", async () => {
    const p = new FakePlayer();
    p.refuse = "NotAllowedError";
    const e = new SyncEngine(p);
    e.step(state(), 10_000, 0);
    await Promise.resolve();
    expect(e.status).toBe("blocked");
    expect(e.step(state(), 10_500, 500)).toBe("blocked");
    p.refuse = null;
    e.unblock();
    e.step(state(), 11_000, 1_000);
    await Promise.resolve();
    expect(p.paused).toBe(false);
  });

  it("doesn't take an interrupted play (AbortError) for a refusal", async () => {
    const p = new FakePlayer();
    p.refuse = "AbortError";
    const e = new SyncEngine(p);
    e.step(state(), 10_000, 0);
    await Promise.resolve();
    expect(e.status).toBe("catchingUp");
    // The next step simply tries again.
    p.refuse = null;
    e.step(state(), 10_250, 250);
    await Promise.resolve();
    expect(p.plays).toBe(2);
    expect(p.paused).toBe(false);
  });
});

describe("the end of a clip", () => {
  // The media is a little shorter than the server thinks (40 s vs 40.02 s).
  const end = state({ durationMs: 40_020 });

  it("stays on the last frame and never plays an ended video", () => {
    const p = new FakePlayer();
    p.duration = 40;
    const e = new SyncEngine(p);
    e.step(end, 10_000, 0);
    // The video ran out by itself: ended and paused, 20 ms before the server's end.
    p.currentTime = 40;
    p.ended = true;
    p.paused = true;
    expect(e.step(end, 50_005, 40_005)).toBe("synced");
    expect(p.plays).toBe(1);
    expect(p.currentTime).toBe(40);
    // Long after: still not restarted.
    expect(e.step(end, 90_000, 80_000)).toBe("synced");
    expect(p.plays).toBe(1);
  });

  it("clamps jumps to just before the end of the media", () => {
    const p = new FakePlayer();
    p.duration = 40;
    const e = new SyncEngine(p);
    e.step(end, 10_000, 0);
    p.advance(2_000);
    e.step(end, 12_000, 2_000);
    // Way behind with 100 ms to go: the jump (target + lead) would land past the end.
    p.currentTime = 30;
    e.step(end, 49_900, 39_900);
    expect(p.currentTime).toBeCloseTo(40 - SYNC.endMarginMs / 1000);
    // A paused show past the media's end holds just before it, too.
    p.currentTime = 0;
    e.step(state({ playing: false, positionMs: 40_010 }), 60_000, 50_000);
    expect(p.currentTime).toBeCloseTo(40 - SYNC.endMarginMs / 1000);
  });

  it("after the end, going back seeks off the end before playing", async () => {
    const p = new FakePlayer();
    p.duration = 40;
    p.currentTime = 40;
    p.ended = true;
    const e = new SyncEngine(p);
    const back = state({ positionMs: 10_000, atServerMs: 100_000 });
    expect(e.step(back, 100_000, 0)).toBe("catchingUp");
    expect(p.currentTime).toBe(10);
    expect(p.plays).toBe(0);
    // The seek has taken it off the end (a browser clears `ended` then).
    p.ended = false;
    e.step(back, 100_250, 250);
    await Promise.resolve();
    expect(p.plays).toBe(1);
    expect(p.currentTime).toBeCloseTo(10.25);
  });
});
