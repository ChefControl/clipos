import type { ClipAnalysis } from "../clips/killfeed/analysis";

/** How long Kip's kill card is up, and its outro at the end of that. */
export const CARD_MS = 5_000;
export const CARD_OUTRO_MS = 600;

/** When Kip's kill card is up in a clip, and how many kills it shows: from the uploader's
 *  death, or the clip's last 5 s if they don't die (S8). Nothing without a killfeed. */
export function killCard(
  analysis: ClipAnalysis | null,
  durationMs: number | null,
): { kills: number; fromMs: number; untilMs: number } | null {
  if (!analysis?.stats || durationMs == null) return null;
  const death = analysis.kills.find((k) => k.owner === "myDeath");
  const fromMs = Math.max(0, death ? death.t * 1000 : durationMs - CARD_MS);
  return {
    kills: Math.min(5, analysis.stats.myKills),
    fromMs,
    untilMs: Math.min(durationMs, fromMs + CARD_MS),
  };
}
