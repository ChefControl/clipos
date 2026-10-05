import { describe, expect, it } from "vitest";
import {
  formatBytes,
  formatDuration,
  shortDate,
  showName,
  timeAgo,
  titleFromFilename,
} from "./format";

describe("format", () => {
  it("formats sizes", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(500)).toBe("1 KB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
    expect(formatBytes(1.5 * 1024 ** 3)).toBe("1.50 GB");
  });

  it("formats durations", () => {
    expect(formatDuration(65_400)).toBe("1:05");
    expect(formatDuration(9_000)).toBe("0:09");
  });

  it("derives a title from the file name", () => {
    expect(titleFromFilename("my_ace_clip.mp4")).toBe("my ace clip");
    expect(titleFromFilename("MedalTVCounterStrike220250408185133.mp4")).toBe(
      "Counter-Strike 2 · 2025-04-08 18:51",
    );
    expect(titleFromFilename("MedalTVValorant20250408185133.mp4")).toBe(
      "Valorant · 2025-04-08 18:51",
    );
    expect(titleFromFilename("Counter-strike 2 2026.10.01 - 21.04.33.02.DVR.mp4")).toBe(
      "Counter-strike 2 · 2026-10-01 21:04",
    );
    expect(titleFromFilename("2026-10-01 21-04-33.mkv")).toBe("Clip · 2026-10-01 21:04");
  });

  it("writes short dates, with the year only when it isn't this one", () => {
    const now = new Date("2026-10-20T12:00:00");
    expect(shortDate("2026-10-16T21:00:00", now)).toBe("Oct 16");
    expect(shortDate("2025-04-08T18:51:00", now)).toBe("Apr 8, 2025");
  });

  it("describes how long ago", () => {
    const now = Date.parse("2026-10-01T12:00:00Z");
    expect(timeAgo("2026-10-01T11:59:30Z", now)).toBe("just now");
    expect(timeAgo("2026-10-01T11:45:00Z", now)).toBe("15m ago");
    expect(timeAgo("2026-10-01T09:00:00Z", now)).toBe("3h ago");
    expect(timeAgo("2026-09-28T12:00:00Z", now)).toBe("3d ago");
    // A week or more: the date. And a clock running behind isn't "in the future".
    expect(timeAgo("2026-09-01T12:00:00Z", now)).toBe(
      new Date("2026-09-01T12:00:00Z").toLocaleDateString(),
    );
    expect(timeAgo("2026-10-01T12:00:30Z", now)).toBe("just now");
  });
});

describe("showName", () => {
  it("names a show after its night", () => {
    expect(showName("2026-10-02T19:00:00")).toBe("Friday night show");
    expect(showName(null)).toBe("The show");
  });
});
