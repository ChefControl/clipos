import {
  MediaControlBar,
  MediaController,
  MediaFullscreenButton,
  MediaLoadingIndicator,
  MediaMuteButton,
  MediaPlayButton,
  MediaPlaybackRateButton,
  MediaPreviewTimeDisplay,
  MediaTimeDisplay,
  MediaTimeRange,
} from "media-chrome/react";
import {
  type ComponentRef,
  type CSSProperties,
  type ReactNode,
  type RefObject,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { formatDuration } from "../lib/format";
import { jumpTime, type Kill, nextKill, previousKill } from "./killfeed/analysis";
import { iconUrl, modifierName, weaponName } from "./killfeed/names";

export const SPEEDS = [0.25, 0.5, 1, 1.5, 2] as const;

/** Keyboard shortcuts, YouTube's where YouTube has one. */
export const SHORTCUTS: [string, string][] = [
  ["Space / K", "Play / pause"],
  ["J / L", "Back / forward 10 s"],
  ["← / →", "Back / forward 5 s"],
  [", / .", "Previous / next frame (pauses)"],
  ["< / >", "Slower / faster"],
  ["[ / ]", "Previous / next kill"],
  ["M", "Mute"],
  ["F", "Fullscreen"],
  ["T", "Theater mode"],
  ["0–9", "Jump to 0–90 %"],
  ["?", "Show these shortcuts"],
];

/** Next speed in `SPEEDS` from `current`, one step in `direction` (clamped). */
export function stepSpeed(current: number, direction: 1 | -1): number {
  const index = SPEEDS.findIndex((s) => s >= current);
  const at = index === -1 ? SPEEDS.length - 1 : index;
  return SPEEDS[Math.min(SPEEDS.length - 1, Math.max(0, at + direction))] ?? current;
}

/** Seeks to a kill (with a short lead-in) and plays from there. */
export function playKill(video: HTMLVideoElement | null, kill: Kill) {
  if (!video) return;
  video.currentTime = jumpTime(kill);
  void video.play().catch(() => {});
}

// Media Chrome's look, in the app's colours. The time range's own padding is zeroed so
// the kill marks can be placed by percentage of its width.
const THEME = {
  "--media-primary-color": "#ffffff",
  "--media-secondary-color": "transparent",
  "--media-control-background": "transparent",
  "--media-control-hover-background": "rgb(255 255 255 / 0.12)",
  "--media-background-color": "transparent",
  "--media-font-family": "var(--font-sans)",
  "--media-font-size": "13px",
  "--media-range-bar-color": "#eceae4",
  "--media-range-track-background": "rgb(255 255 255 / 0.28)",
  "--media-range-track-height": "5px",
  "--media-range-track-border-radius": "3px",
  "--media-range-thumb-background": "#ffffff",
  "--media-time-range-buffered-color": "rgb(255 255 255 / 0.45)",
  "--media-range-padding-left": "0px",
  "--media-range-padding-right": "0px",
  "--media-tooltip-background-color": "rgb(14 15 17 / 0.92)",
  // Quick to get out of the way once the pointer rests (see `autohide` below).
  "--media-control-transition-in": "opacity 0.15s",
  "--media-control-transition-out": "opacity 0.25s",
} as CSSProperties;

const icon = (path: ReactNode) => (
  <svg
    viewBox="0 0 24 24"
    width="22"
    height="22"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden="true"
  >
    {path}
  </svg>
);

function ControlButton({
  label,
  onClick,
  disabled,
  pressed,
  children,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  pressed?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={pressed}
      disabled={disabled}
      onClick={onClick}
      className="grid h-11 w-11 shrink-0 place-items-center text-white hover:bg-white/12 focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-35 disabled:hover:bg-transparent"
    >
      {children}
    </button>
  );
}

/** The clip player: Media Chrome's controls, the kills marked on its timeline, and the
 *  shortcuts in `SHORTCUTS`. */
export function Player({
  src,
  poster,
  fps,
  durationS,
  kills = [],
  videoRef,
  theater,
  onToggleTheater,
}: {
  src: string;
  poster?: string | null;
  fps?: number | null;
  /** Known before the video's metadata loads, so marks appear right away. */
  durationS?: number | null;
  kills?: Kill[];
  videoRef: RefObject<HTMLVideoElement | null>;
  theater: boolean;
  onToggleTheater: () => void;
}) {
  const controller = useRef<ComponentRef<typeof MediaController>>(null);
  const [duration, setDuration] = useState(durationS ?? 0);
  const [time, setTime] = useState(0);
  const [showHelp, setShowHelp] = useState(false);
  const [hoverKill, setHoverKill] = useState<Kill | null>(null);
  const frame = 1 / (fps && fps > 0 ? fps : 60);

  const stepFrame = useCallback(
    (direction: 1 | -1) => {
      const v = videoRef.current;
      if (!v) return;
      v.pause();
      v.currentTime = Math.min(
        v.duration || Infinity,
        Math.max(0, v.currentTime + direction * frame),
      );
    },
    [frame, videoRef],
  );

  const goToKill = useCallback(
    (direction: 1 | -1) => {
      const v = videoRef.current;
      if (!v) return;
      const kill =
        direction === 1 ? nextKill(kills, v.currentTime) : previousKill(kills, v.currentTime);
      if (kill) playKill(v, kill);
    },
    [kills, videoRef],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      // Not while typing, and not behind an open dialog (Edit, Share, Delete): its keys are
      // its own.
      if (
        e.metaKey ||
        e.ctrlKey ||
        e.altKey ||
        target?.closest("input, textarea, select, [contenteditable=true], dialog") ||
        document.querySelector("dialog[open]")
      ) {
        return;
      }
      // A focused button or slider handles Space, Enter and the arrows itself.
      const onControl = target?.closest("button, a, media-time-range, [role=slider]");
      if (onControl && [" ", "enter", "arrowleft", "arrowright"].includes(e.key.toLowerCase())) {
        return;
      }
      const v = videoRef.current;
      if (!v) return;
      const seek = (s: number) => {
        v.currentTime = Math.min(v.duration || Infinity, Math.max(0, v.currentTime + s));
      };
      switch (e.key === "<" || e.key === ">" || e.key === "?" ? e.key : e.key.toLowerCase()) {
        case " ":
        case "k":
          if (v.paused) void v.play().catch(() => {});
          else v.pause();
          break;
        case "j":
          seek(-10);
          break;
        case "l":
          seek(10);
          break;
        case "arrowleft":
          seek(-5);
          break;
        case "arrowright":
          seek(5);
          break;
        case ",":
          stepFrame(-1);
          break;
        case ".":
          stepFrame(1);
          break;
        case "<":
          v.playbackRate = stepSpeed(v.playbackRate, -1);
          break;
        case ">":
          v.playbackRate = stepSpeed(v.playbackRate, 1);
          break;
        case "[":
          goToKill(-1);
          break;
        case "]":
          goToKill(1);
          break;
        case "m":
          v.muted = !v.muted;
          break;
        case "f":
          if (document.fullscreenElement) void document.exitFullscreen();
          else void controller.current?.requestFullscreen?.();
          break;
        case "t":
          onToggleTheater();
          break;
        case "?":
          setShowHelp((h) => !h);
          break;
        default:
          if (/^[0-9]$/.test(e.key) && v.duration) {
            v.currentTime = (Number(e.key) / 10) * v.duration;
            break;
          }
          return;
      }
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [goToKill, onToggleTheater, stepFrame, videoRef]);

  return (
    <div>
      <div className="squircle relative -mx-4 aspect-video overflow-hidden bg-black shadow-[0_30px_70px_-30px_rgba(0,0,0,0.95),0_0_0_1px_rgba(255,255,255,0.06)] sm:mx-0 sm:rounded-[26px]">
        {/* Letterboxed clips (4:3, stretched) sit on a blurred copy of themselves. */}
        {poster && (
          <img
            src={poster}
            alt=""
            aria-hidden="true"
            className="absolute inset-0 h-full w-full scale-110 object-cover blur-2xl brightness-50"
          />
        )}
        <MediaController
          ref={controller}
          noHotkeys
          // Seconds of stillness before the controls hide while playing (Media Chrome's
          // default is 2, and it counts whole seconds).
          autohide="1"
          className="absolute inset-0 h-full w-full"
          style={THEME}
        >
          {/* biome-ignore lint/a11y/useMediaCaption: game clips have no captions. */}
          <video
            slot="media"
            ref={videoRef}
            key={src}
            src={src}
            poster={poster ?? undefined}
            preload="metadata"
            playsInline
            className="h-full w-full object-contain"
            onLoadedMetadata={(e) => setDuration(e.currentTarget.duration)}
            onTimeUpdate={(e) => setTime(e.currentTarget.currentTime)}
            onSeeked={(e) => setTime(e.currentTarget.currentTime)}
          />
          <MediaLoadingIndicator slot="centered-chrome" noAutohide />
          <Glimpse kills={kills} duration={duration} videoRef={videoRef} />
          <div className="w-full bg-gradient-to-t from-black/85 via-black/45 to-transparent pt-10">
            <MediaControlBar className="w-full px-3">
              {/* The marks are drawn over the time range but don't take the pointer: hovering
                  the range near one shows what it was, and clicking seeks as usual. */}
              <div
                className="relative w-full"
                onPointerMove={(e) => {
                  const r = e.currentTarget.getBoundingClientRect();
                  setHoverKill(nearestKill(kills, duration, r.width, e.clientX - r.left));
                }}
                onPointerLeave={() => setHoverKill(null)}
              >
                <MediaTimeRange className="w-full">
                  <MediaPreviewTimeDisplay slot="preview" />
                </MediaTimeRange>
                <KillMarks kills={kills} duration={duration} hover={hoverKill} />
              </div>
            </MediaControlBar>
            <MediaControlBar className="w-full px-1">
              <MediaPlayButton />
              <ControlButton
                label="Previous kill ([)"
                onClick={() => goToKill(-1)}
                disabled={!previousKill(kills, time)}
              >
                {icon(
                  <>
                    <path d="M6 5v14" />
                    <path d="M19 5.5v13L9 12z" />
                  </>,
                )}
              </ControlButton>
              <ControlButton
                label="Next kill (])"
                onClick={() => goToKill(1)}
                disabled={!nextKill(kills, time)}
              >
                {icon(
                  <>
                    <path d="M18 5v14" />
                    <path d="M5 5.5v13L15 12z" />
                  </>,
                )}
              </ControlButton>
              <MediaMuteButton />
              <MediaTimeDisplay showDuration />
              <span className="grow" />
              <MediaPlaybackRateButton rates={[...SPEEDS]} />
              <span className="hidden sm:contents">
                <ControlButton label="Previous frame (,)" onClick={() => stepFrame(-1)}>
                  {icon(<path d="M15 6l-6 6 6 6" />)}
                </ControlButton>
                <ControlButton label="Next frame (.)" onClick={() => stepFrame(1)}>
                  {icon(<path d="M9 6l6 6-6 6" />)}
                </ControlButton>
                <ControlButton label="Theater mode (T)" onClick={onToggleTheater} pressed={theater}>
                  {icon(<rect x="3" y="6" width="18" height="12" rx="2" />)}
                </ControlButton>
                <ControlButton
                  label="Keyboard shortcuts (?)"
                  onClick={() => setShowHelp((h) => !h)}
                >
                  {icon(
                    <>
                      <rect x="3" y="6" width="18" height="12" rx="2" />
                      <path d="M7 10h.01M11 10h.01M15 10h.01M7 14h10" />
                    </>,
                  )}
                </ControlButton>
              </span>
              <MediaFullscreenButton />
            </MediaControlBar>
          </div>
        </MediaController>
      </div>
      {showHelp && (
        <dl className="glass squircle mt-3 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 rounded-2xl p-4 text-sm sm:grid-cols-[auto_1fr_auto_1fr]">
          {SHORTCUTS.map(([keys, action]) => (
            <div key={keys} className="contents">
              <dt className="font-mono text-muted">{keys}</dt>
              <dd>{action}</dd>
            </div>
          ))}
        </dl>
      )}
    </div>
  );
}

/** The kill within 10 px of `x` on a time range `width` px wide (yours preferred), if any. */
export function nearestKill(
  kills: Kill[],
  duration: number,
  width: number,
  x: number,
): Kill | null {
  if (!duration || !width) return null;
  let best: Kill | null = null;
  let bestDistance = 10;
  for (const k of kills) {
    const d = Math.abs((k.t / duration) * width - x);
    if (d < bestDistance || (d === bestDistance && best?.owner === "other")) {
      best = k;
      bestDistance = d;
    }
  }
  return best;
}

/** The kills over the time range: yours amber dots, your deaths CS2's skull, others white dots.
 *  Drawn only; the kill list is the way to them by keyboard or screen reader. */
function KillMarks({
  kills,
  duration,
  hover,
}: {
  kills: Kill[];
  duration: number;
  hover: Kill | null;
}) {
  if (!duration || kills.length === 0) return null;
  const at = (t: number) => `${Math.min(100, (t / duration) * 100)}%`;
  // Others first, so yours are drawn on top where they share a moment. Keyed by place in
  // the analysis: two kills can share a second, an owner and a gun.
  const ordered = kills
    .map((k, i) => ({ k, i }))
    .sort((a, b) => Number(a.k.owner !== "other") - Number(b.k.owner !== "other"));
  const common = "absolute -translate-x-1/2 -translate-y-1/2";
  return (
    <div className="pointer-events-none absolute inset-x-0 top-1/2 h-0" aria-hidden="true">
      {ordered.map(({ k, i }) => (
        <span
          key={i}
          style={{ left: at(k.t) }}
          className={
            k.owner === "myKill"
              ? `${common} h-[9px] w-[9px] rounded-full bg-accent ring-2 ring-[#0e0f11]`
              : k.owner === "myDeath"
                ? `${common} grid h-4 w-4 place-items-center rounded-full bg-black/80`
                : `${common} h-[7px] w-[7px] rounded-full bg-text ring-2 ring-[#0e0f11]`
          }
        >
          {k.owner === "myDeath" && iconUrl("suicide") && (
            <img src={iconUrl("suicide")} alt="" className="h-3 w-3" />
          )}
        </span>
      ))}
      {hover && (
        <div
          className="absolute bottom-4 w-max max-w-64 -translate-x-1/2 rounded-[10px] bg-[rgba(14,15,17,0.92)] px-2.5 py-2 text-xs shadow-[0_8px_20px_-6px_rgba(0,0,0,0.8)]"
          style={{ left: `clamp(7rem, ${at(hover.t)}, calc(100% - 7rem))` }}
        >
          <div className="flex justify-between gap-4">
            <span className={hover.owner === "other" ? "text-muted" : "font-bold text-accent"}>
              {hover.owner === "myKill"
                ? "Your kill"
                : hover.owner === "myDeath"
                  ? "Your death"
                  : "Kill"}
            </span>
            <span className="tabular-nums text-muted">{formatDuration(hover.t * 1000)}</span>
          </div>
          <div className="mt-1.5 flex items-center gap-1.5">
            <KillIcons kill={hover} size={16} />
          </div>
          <div className="mt-1 text-muted">{killLabel(hover)}</div>
        </div>
      )}
    </div>
  );
}

/** While the controls are hidden: a thin progress line along the bottom edge, as YouTube
 *  keeps, with the current moment and your kills and deaths on it. */
function Glimpse({
  kills,
  duration,
  videoRef,
}: {
  kills: Kill[];
  duration: number;
  videoRef: RefObject<HTMLVideoElement | null>;
}) {
  const played = useRef<HTMLDivElement>(null);
  const dot = useRef<HTMLDivElement>(null);

  // Moved every frame while playing (timeupdate only comes a few times a second).
  useEffect(() => {
    const v = videoRef.current;
    if (!v || !duration) return;
    let frame = 0;
    const draw = () => {
      const at = `${Math.min(100, (v.currentTime / duration) * 100)}%`;
      if (played.current) played.current.style.width = at;
      if (dot.current) dot.current.style.left = at;
    };
    const tick = () => {
      draw();
      frame = requestAnimationFrame(tick);
    };
    const start = () => {
      cancelAnimationFrame(frame);
      tick();
    };
    const stop = () => {
      cancelAnimationFrame(frame);
      draw();
    };
    if (v.paused) draw();
    else start();
    v.addEventListener("play", start);
    v.addEventListener("pause", stop);
    v.addEventListener("seeked", draw);
    return () => {
      cancelAnimationFrame(frame);
      v.removeEventListener("play", start);
      v.removeEventListener("pause", stop);
      v.removeEventListener("seeked", draw);
    };
  }, [duration, videoRef]);

  if (!duration) return null;
  const mine = kills.map((k, i) => ({ k, i })).filter(({ k }) => k.owner !== "other");
  return (
    <div
      // Media Chrome hides its slotted children when idle; this one does the opposite.
      {...{ noautohide: "" }}
      aria-hidden="true"
      className="pointer-events-none absolute inset-x-3 bottom-2 h-[3px] rounded-full bg-white/25 opacity-0 transition-opacity duration-300 [media-controller[userinactive]:not([mediapaused])_&]:opacity-90"
    >
      <div ref={played} className="h-full w-0 rounded-full bg-text" />
      {mine.map(({ k, i }) => (
        <span
          key={i}
          style={{ left: `${Math.min(100, (k.t / duration) * 100)}%` }}
          className="absolute top-1/2 -translate-x-1/2 -translate-y-1/2"
        >
          {k.owner === "myKill" ? (
            <span className="block h-2 w-2 rounded-full bg-accent ring-1 ring-black/60" />
          ) : (
            iconUrl("suicide") && (
              <span className="grid h-3.5 w-3.5 place-items-center rounded-full bg-black/70">
                <img src={iconUrl("suicide")} alt="" className="h-2.5 w-2.5" />
              </span>
            )
          )}
        </span>
      ))}
      <div
        ref={dot}
        className="absolute top-1/2 left-0 h-2.5 w-2.5 -translate-x-1/2 -translate-y-1/2 rounded-full bg-white shadow-[0_0_0_1px_rgb(0_0_0/0.4)]"
      />
    </div>
  );
}

/** "AK-47 · through smoke". */
export function killLabel(kill: Kill): string {
  return [weaponName(kill.weapon), ...kill.modifiers.map(modifierName)].join(" · ");
}

/** The kill's weapon and modifier icons, as the killfeed draws them. */
export function KillIcons({ kill, size = 18 }: { kill: Kill; size?: number }) {
  const names = [kill.weapon ?? "", ...kill.modifiers].filter(Boolean);
  if (names.length === 0) {
    return <span className="text-xs text-muted">unknown weapon</span>;
  }
  return (
    <>
      {names.map((n) => {
        const url = iconUrl(n);
        const label = n === kill.weapon ? weaponName(n) : modifierName(n);
        return url ? (
          <img
            key={n}
            src={url}
            alt={label}
            title={label}
            style={{ height: size }}
            className="w-auto"
          />
        ) : (
          <span key={n} className="text-xs text-white">
            {label}
          </span>
        );
      })}
    </>
  );
}
