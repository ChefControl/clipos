import { type RefObject, useEffect, useState } from "react";

/** Your volume in the show. It's yours alone (the room shares where the clip is, not how
 *  loud), kept in this browser for the next show. */
export interface Level {
  /** 0–1. */
  volume: number;
  muted: boolean;
}

const KEY = "clipos.show.volume";
export const FULL: Level = { volume: 1, muted: false };

/** The level kept from last time, or full volume: storage may be empty, blocked or hold
 *  something else. */
export function readLevel(storage: Pick<Storage, "getItem"> | null = safeStorage()): Level {
  try {
    const raw = JSON.parse(storage?.getItem(KEY) ?? "null") as Partial<Level> | null;
    const volume = Number(raw?.volume);
    if (!raw || !Number.isFinite(volume)) return FULL;
    return { volume: Math.min(1, Math.max(0, volume)), muted: raw.muted === true };
  } catch {
    return FULL;
  }
}

function keep(level: Level) {
  try {
    safeStorage()?.setItem(KEY, JSON.stringify(level));
  } catch {
    // Private windows and blocked storage: it just isn't remembered.
  }
}

function safeStorage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

/** The mute button's next level: unmuting at 0 comes back at half, or it stays silent. */
export function toggled(level: Level): Level {
  if (level.muted || level.volume === 0) {
    return { volume: level.volume === 0 ? 0.5 : level.volume, muted: false };
  }
  return { ...level, muted: true };
}

// The show's volume (decision 57): a round button in the player's bottom-left corner,
// across from Full screen. Hovering it (or tabbing to it) slides the volume out;
// leaving it slides it back, and the button stays. Clicking the button mutes.
export function Volume({ video }: { video: RefObject<HTMLVideoElement | null> }) {
  const [level, setLevel] = useState(readLevel);
  // The same <video> plays every clip, and keeps its volume across them; set it once
  // here, then on each change.
  useEffect(() => {
    const v = video.current;
    if (!v) return;
    v.volume = level.volume;
    v.muted = level.muted;
    keep(level);
  }, [level, video]);
  const silent = level.muted || level.volume === 0;
  const percent = Math.round((silent ? 0 : level.volume) * 100);

  return (
    <div
      className="group/volume frost absolute bottom-5 left-[18px] flex h-[52px] items-center rounded-full"
      data-testid="volume"
    >
      <button
        type="button"
        aria-label={silent ? "Unmute" : "Mute"}
        title={silent ? "Unmute" : "Mute"}
        onClick={() => setLevel(toggled(level))}
        className="grid h-[52px] w-[52px] shrink-0 place-items-center rounded-full text-text hover:bg-white/15"
      >
        <SpeakerIcon level={silent ? 0 : level.volume} />
      </button>
      <span className="flex w-0 items-center overflow-hidden opacity-0 transition-[width,opacity] duration-200 ease-out group-has-[:focus-visible]/volume:w-[124px] group-has-[:focus-visible]/volume:opacity-100 group-hover/volume:w-[124px] group-hover/volume:opacity-100 motion-reduce:transition-none">
        <input
          type="range"
          min={0}
          max={100}
          step={1}
          value={percent}
          aria-label="Volume"
          aria-valuetext={`${percent}%`}
          onChange={(e) => {
            const volume = Number(e.target.value) / 100;
            setLevel({ volume, muted: volume === 0 ? level.muted : false });
          }}
          className="mr-5 ml-1 h-6 w-[100px] shrink-0 cursor-pointer accent-accent"
        />
      </span>
    </div>
  );
}

export function SpeakerIcon({ level }: { level: number }) {
  return (
    <svg
      viewBox="0 0 24 24"
      className="h-5 w-5"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M4 9.5h3.5L12 5v14l-4.5-4.5H4z" fill="currentColor" />
      {level === 0 ? (
        <path d="M16 9.5l5 5M21 9.5l-5 5" />
      ) : (
        <>
          <path d="M15.5 9a4 4 0 0 1 0 6" />
          {level > 0.5 && <path d="M18.5 6.5a7.5 7.5 0 0 1 0 11" />}
        </>
      )}
    </svg>
  );
}
