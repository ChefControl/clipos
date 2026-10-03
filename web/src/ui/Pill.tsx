import type { ReactNode } from "react";

export type PillTone = "neutral" | "accent" | "violet" | "danger" | "green";

const TONES: Record<PillTone, string> = {
  neutral: "bg-white/10 text-soft",
  accent: "bg-accent/15 text-accent-strong",
  violet: "bg-violet/15 text-violet",
  danger: "bg-danger/15 text-danger",
  green: "bg-green/15 text-green",
};

// A small read-only label: "Last show", "4K", "Synced", "Pending".
export function Pill({
  tone = "neutral",
  className = "",
  children,
}: {
  tone?: PillTone;
  className?: string;
  children: ReactNode;
}) {
  return (
    <span
      className={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-xs font-bold whitespace-nowrap ${TONES[tone]} ${className}`}
    >
      {children}
    </span>
  );
}
