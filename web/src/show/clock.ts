// The server's clock, as seen from this browser (decision 34): NTP-style pings over the
// show's live connection. Each sample gives an offset (server minus local, at the
// midpoint of the round trip) and a round trip; the fastest of the recent samples wins,
// because a slow trip means the midpoint guess is likely off.
//
// Samples go stale: they're dropped after a few minutes, on every new connection
// (`reset`), and when the machine has slept. A sleeping laptop's `performance.now()`
// stops (or not, depending on the browser) while `Date.now()` keeps going, so an offset
// measured before the sleep can be seconds off after it.

export interface ClockSample {
  offsetMs: number;
  rttMs: number;
  /** Local `performance.now()` time it was taken. */
  atMs: number;
}

const KEEP = 8;
const MAX_AGE_MS = 5 * 60_000;
/** Wall time and `performance.now()` moving apart by more than this means a sleep. */
const SLEEP_JUMP_MS = 2_000;

export class ServerClock {
  private samples: ClockSample[] = [];
  private seen: { perf: number; wall: number } | null = null;

  /** `sentAt` and `receivedAt` are local `performance.now()` times; `serverMs` is the
   *  server's time when it answered. Returns null when the sample was dropped because the
   *  machine slept since the last one (the clock is empty again then). */
  add(sentAt: number, serverMs: number, receivedAt: number, wallNow = Date.now()) {
    // The round trip may span the sleep: its offset can't be trusted either.
    if (this.checkSleep(receivedAt, wallNow)) return null;
    const rttMs = Math.max(0, receivedAt - sentAt);
    const sample = { offsetMs: serverMs - (sentAt + receivedAt) / 2, rttMs, atMs: receivedAt };
    const recent = this.samples.filter((s) => receivedAt - s.atMs <= MAX_AGE_MS);
    this.samples = [...recent, sample].slice(-KEEP);
    return sample;
  }

  /** Forgets every sample: a new connection measures afresh. */
  reset() {
    this.samples = [];
  }

  /** True (and the samples are gone) if wall time and `performance.now()` moved apart
   *  since the last check: the machine slept. Checked on every sample and when the tab
   *  comes back. */
  checkSleep(perfNow = performance.now(), wallNow = Date.now()): boolean {
    const last = this.seen;
    this.seen = { perf: perfNow, wall: wallNow };
    if (!last) return false;
    const jump = Math.abs(wallNow - last.wall - (perfNow - last.perf));
    if (jump <= SLEEP_JUMP_MS) return false;
    this.reset();
    return true;
  }

  /** The best sample so far, or undefined before the first pong. */
  get best(): ClockSample | undefined {
    return this.samples.reduce<ClockSample | undefined>(
      (best, s) => (!best || s.rttMs < best.rttMs ? s : best),
      undefined,
    );
  }

  /** The most recent sample: its round trip is what the server plans starts around. */
  get latest(): ClockSample | undefined {
    return this.samples.at(-1);
  }

  get synced(): boolean {
    return this.samples.length > 0;
  }

  /** Server time now (or at local time `local`). */
  now(local = performance.now()): number {
    return local + (this.best?.offsetMs ?? 0);
  }

  /** Local `performance.now()` time when the server's clock reads `serverMs`. */
  toLocal(serverMs: number): number {
    return serverMs - (this.best?.offsetMs ?? 0);
  }
}
