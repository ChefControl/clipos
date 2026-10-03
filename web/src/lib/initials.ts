/** Up to two uppercase initials for an avatar placeholder. */
export function initials(name: string): string {
  const words = name
    .trim()
    .split(/[\s._-]+/)
    .filter(Boolean);
  const picked = words.length > 1 ? [words[0], words.at(-1)] : words;
  return picked.map((w) => w?.[0]?.toUpperCase() ?? "").join("") || "?";
}
