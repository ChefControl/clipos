export function formatBytes(bytes: number): string {
  if (bytes === 0) {
    return "0 B";
  }
  if (bytes < 1024 * 1024) {
    return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  }
  if (bytes < 1024 * 1024 * 1024) {
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export function formatDuration(ms: number): string {
  const total = Math.round(ms / 1000);
  return `${Math.floor(total / 60)}:${(total % 60).toString().padStart(2, "0")}`;
}

/** CamelCase game names from Medal ("CounterStrike2") → "Counter Strike 2". */
function splitWords(s: string): string {
  return s
    .replace(/([a-z])([A-Z0-9])/g, "$1 $2")
    .replace(/([0-9])([A-Za-z])/g, "$1 $2")
    .trim();
}

/** A readable starting title from a recorder's file name. */
export function titleFromFilename(name: string): string {
  const base = name.replace(/\.[^.]+$/, "").trim();

  // Medal: MedalTVCounterStrike220250408185133 → Counter Strike 2 · 2025-04-08 18:51
  const medal = base.match(/^MedalTV(.*?)(\d{4})(\d{2})(\d{2})(\d{2})(\d{2})\d{2}$/);
  if (medal) {
    const [, game, y, mo, d, h, mi] = medal;
    const label = splitWords(game ?? "")
      .replace(/^Counter Strike 2$/, "Counter-Strike 2")
      .replace(/^Counter Strike2$/, "Counter-Strike 2");
    return `${label || "Clip"} · ${y}-${mo}-${d} ${h}:${mi}`;
  }

  // ShadowPlay: Counter-strike 2 2026.10.01 - 21.04.33.02.DVR → Counter-strike 2 · 2026-10-01 21:04
  const shadowplay = base.match(
    /^(.*?)\s*(\d{4})\.(\d{2})\.(\d{2}) - (\d{2})\.(\d{2})\.\d{2}(?:\.\d+)?(?:\.DVR)?$/,
  );
  if (shadowplay) {
    const [, game, y, mo, d, h, mi] = shadowplay;
    return `${game?.trim() || "Clip"} · ${y}-${mo}-${d} ${h}:${mi}`;
  }

  // OBS: 2026-10-01 21-04-33 → Clip · 2026-10-01 21:04
  const obs = base.match(/^(\d{4}-\d{2}-\d{2})[ _](\d{2})-(\d{2})-\d{2}$/);
  if (obs) {
    return `Clip · ${obs[1]} ${obs[2]}:${obs[3]}`;
  }

  return base.replace(/_+/g, " ").trim().slice(0, 100);
}

export function timeAgo(iso: string, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - new Date(iso).getTime()) / 1000));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 7 * 86400) return `${Math.floor(s / 86400)}d ago`;
  return new Date(iso).toLocaleDateString();
}

/** "Oct 16" this year, "Oct 16, 2025" before. */
export function shortDate(iso: string, now = new Date()): string {
  const d = new Date(iso);
  return d.toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    ...(d.getFullYear() === now.getFullYear() ? {} : { year: "numeric" }),
  });
}

/** "Friday night show" for a show that started at `iso`. */
export function showName(iso: string | null | undefined): string {
  if (!iso) return "The show";
  return `${new Date(iso).toLocaleDateString("en-US", { weekday: "long" })} night show`;
}
