import { useEffect, useRef, useState } from "react";
import { Kip } from "../kip/Kip";

/** The show dock's reactions (the clip page's six) and 🍌, the fail button, which only the
 *  show has (decision 30). */
export const DOCK = [
  ["🔥", "Fire"],
  ["😂", "Laugh"],
  ["💀", "Dead"],
  ["🐐", "GOAT"],
  ["😮", "Wow"],
  ["👏", "Clap"],
] as const;
export const BANANA = "🍌";

export interface Float {
  id: number;
  emoji: string;
  /** Across the overlay's lane, 0–1. */
  x: number;
}

/** Reactions floating up the right edge of the player, each gone after its animation. At
 *  most `max` at once, so a burst of taps can't pile up. */
export function useFloats(max = 30) {
  const [floats, setFloats] = useState<Float[]>([]);
  const next = useRef(0);
  const add = (emoji: string) => {
    const id = next.current++;
    setFloats((f) => [...f.slice(-(max - 1)), { id, emoji, x: Math.random() }]);
    setTimeout(() => setFloats((f) => f.filter((x) => x.id !== id)), 2600);
  };
  return { floats, add };
}

export function FloatingReactions({ floats }: { floats: Float[] }) {
  return (
    <div
      aria-hidden="true"
      className="pointer-events-none absolute top-0 right-4 bottom-16 w-24 overflow-hidden"
      data-testid="floats"
    >
      {floats.map((f) => (
        <span
          key={f.id}
          className="float-up absolute bottom-0 text-[30px] drop-shadow-[0_2px_3px_rgba(0,0,0,0.6)]"
          style={{ left: `${Math.round(f.x * 60)}%` }}
        >
          {f.emoji}
        </span>
      ))}
    </div>
  );
}

// React (canvas 2.2, 2.3): a round button that opens the dock under it, six reactions
// and 🍌. Every tap floats on everyone's screen. The dock stays open for more taps;
// React again, Escape or a click elsewhere closes it. `failHint`: the uploader died in
// this clip, so Kip asks "Fail?" beside it (decision 30), until someone presses 🍌.
export function ReactButton({
  onReact,
  failMarked,
  failHint,
  disabled,
}: {
  onReact: (emoji: string) => void;
  failMarked: boolean;
  failHint: boolean;
  disabled: boolean;
}) {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (e: PointerEvent) => {
      if (!box.current?.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("pointerdown", away);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("pointerdown", away);
      document.removeEventListener("keydown", esc);
    };
  }, [open]);

  return (
    <div ref={box} className="relative flex items-center gap-3">
      {failHint && !failMarked && !open && (
        <button
          type="button"
          onClick={() => onReact(BANANA)}
          disabled={disabled}
          className="glass flex items-center gap-1.5 rounded-full py-1 pr-3.5 pl-1 text-sm font-bold text-violet ring-1 ring-violet/50 hover:ring-violet"
        >
          <Kip pose="banana-slip" className="h-8 w-8" />
          Fail? {BANANA}
        </button>
      )}
      {open && (
        <fieldset className="glass absolute top-full right-0 z-10 m-0 mt-3 flex min-w-0 items-center gap-0.5 rounded-full px-1.5 py-[5px] shadow-[0_10px_30px_-10px_rgba(0,0,0,0.8)]">
          <legend className="sr-only">Reactions</legend>
          {DOCK.map(([emoji, name]) => (
            <button
              key={emoji}
              type="button"
              aria-label={`React ${name}`}
              disabled={disabled}
              onClick={() => onReact(emoji)}
              className="grid h-11 w-11 place-items-center rounded-full text-[26px] leading-none transition hover:scale-110 hover:bg-white/10 active:scale-95"
            >
              {emoji}
            </button>
          ))}
          <span aria-hidden="true" className="mx-1 h-7 w-px bg-white/15" />
          <button
            type="button"
            aria-label="Fail? Mark it for fail of the night"
            aria-pressed={failMarked}
            title={failMarked ? "Marked for fail of the night" : "Fail of the night?"}
            disabled={disabled}
            onClick={() => onReact(BANANA)}
            className={`grid h-11 w-11 place-items-center rounded-full text-[26px] leading-none transition hover:scale-110 active:scale-95 ${
              failMarked ? "bg-violet/25 ring-2 ring-violet" : "hover:bg-violet/15"
            }`}
          >
            {BANANA}
          </button>
        </fieldset>
      )}
      <RoundButton
        label="React"
        expanded={open}
        onClick={() => setOpen((o) => !o)}
        className={open ? "ring-2 ring-accent" : ""}
      >
        <svg
          viewBox="0 0 24 24"
          className="h-[22px] w-[22px]"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <circle cx="11" cy="12" r="8" />
          <path d="M8 14c.8 1.2 1.8 1.8 3 1.8s2.2-.6 3-1.8" />
          <circle cx="8.5" cy="10" r=".9" fill="currentColor" />
          <circle cx="13.5" cy="10" r=".9" fill="currentColor" />
          <path d="M19 3v4M17 5h4" />
        </svg>
      </RoundButton>
    </div>
  );
}

/** The show's round 52 px control (React, Pause for everyone, Next clip, …). */
export function RoundButton({
  label,
  onClick,
  disabled,
  expanded,
  className = "",
  children,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  expanded?: boolean;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-expanded={expanded}
      disabled={disabled}
      onClick={onClick}
      className={`frost grid h-[52px] w-[52px] shrink-0 place-items-center rounded-full text-text transition hover:bg-white/15 disabled:opacity-40 ${className}`}
    >
      {children}
    </button>
  );
}
