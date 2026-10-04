import { Link } from "@tanstack/react-router";
import { Kip, Logo } from "../kip/Kip";
import { formatDuration, shortDate } from "../lib/format";
import { Avatar } from "../ui/Avatar";
import type { Clip } from "./hooks";

// A clip in the archive and on profiles (canvas 4.1): squircle poster with the multi-kill
// and length on it, then who and when, then the reactions it got. Someone else's clip
// saved for the show is a teaser: blurred, and not a link until it plays.
export function ClipCard({ clip }: { clip: Clip }) {
  if (clip.teaser) return <TeaserCard clip={clip} />;
  return (
    <Link
      to="/clips/$clipId"
      params={{ clipId: clip.id }}
      className="group flex min-w-0 flex-col gap-2.5 text-text hover:text-white"
    >
      <div
        className={`squircle relative aspect-video overflow-hidden rounded-[18px] bg-surface transition ${
          clip.clipOfTheNight
            ? "ring-[1.5px] ring-accent/80 group-hover:ring-accent"
            : "ring-1 ring-white/6 group-hover:ring-white/25"
        }`}
      >
        {clip.posterUrl ? (
          <img
            src={clip.posterUrl}
            alt=""
            loading="lazy"
            className="h-full w-full object-cover transition duration-300 group-hover:scale-[1.03]"
          />
        ) : clip.status === "ready" ? (
          // Ready, but the worker made no poster: a quiet Kip, nothing to wait for.
          <div className="grid h-full place-items-center">
            <Logo className="h-12 w-12 opacity-25" />
          </div>
        ) : (
          <div className="grid h-full place-items-center text-sm text-muted">
            {clip.status === "failed" ? "Processing failed" : "Processing…"}
          </div>
        )}
        <div className="absolute top-2.5 left-2.5 flex gap-1.5">
          {clip.clipOfTheNight && (
            <span className="flex items-center gap-1 rounded-full bg-gradient-to-b from-accent-strong to-accent py-0.5 pr-2.5 pl-2 text-xs font-extrabold text-on-accent">
              <TrophyIcon />
              Clip of the night
            </span>
          )}
          {clip.failOfTheNight && (
            <span className="flex items-center gap-1 rounded-full bg-[rgba(14,15,17,0.8)] py-0.5 pr-2.5 pl-0.5 text-xs font-bold text-violet">
              <Kip pose="banana-slip" className="h-5 w-5" />
              Fail of the night
            </span>
          )}
          {clip.heldUntil && (
            <span className="rounded-full bg-[rgba(14,15,17,0.8)] px-2.5 py-0.5 text-xs font-bold text-accent-strong">
              Saved for the show
            </span>
          )}
          {clip.status === "ready" && clip.multiKill && (
            <span
              className="rounded-full bg-accent px-2.5 py-0.5 text-xs font-extrabold text-[#0e0f11] uppercase"
              title={`${clip.multiKill} from the killfeed`}
            >
              {clip.multiKill}
            </span>
          )}
          {clip.status !== "ready" && (
            <span
              className={`rounded-full px-2.5 py-0.5 text-xs font-bold ${
                clip.status === "failed"
                  ? "bg-danger-strong text-white"
                  : "bg-accent text-on-accent"
              }`}
            >
              {clip.status === "failed" ? "Failed" : "Processing"}
            </span>
          )}
        </div>
        {clip.durationMs != null && (
          <span className="absolute right-2.5 bottom-2.5 rounded-md bg-[rgba(8,9,10,0.75)] px-2 py-0.5 font-mono text-[11px] text-text">
            {formatDuration(clip.durationMs)}
          </span>
        )}
      </div>
      <div className="flex items-center gap-2.5 px-0.5">
        <Avatar name={clip.uploader.displayName} url={clip.uploader.avatarUrl} size={28} />
        <div className="flex min-w-0 flex-col">
          {/* Titles and names can be right-to-left (Hebrew): they truncate at their own end. */}
          <p dir="auto" className="truncate text-lg leading-tight font-bold">
            {clip.title}
          </p>
          <p className="truncate text-[13px] text-muted">
            <bdi>{clip.uploader.displayName}</bdi>
            {clip.map ? ` · ${clip.map}` : ""} · {shortDate(clip.createdAt)}
          </p>
        </div>
      </div>
      {clip.reactions.length > 0 && (
        <div className="flex flex-wrap gap-1.5 px-0.5">
          {clip.reactions.map((r) => (
            <span
              key={r.emoji}
              className="frost flex items-center gap-1 rounded-full py-0.5 pr-2.5 pl-2 text-[13px] font-semibold text-soft"
            >
              <span>{r.emoji}</span>
              {r.count}
            </span>
          ))}
        </div>
      )}
    </Link>
  );
}

function TeaserCard({ clip }: { clip: Clip }) {
  return (
    <div className="flex min-w-0 flex-col gap-2.5">
      <div className="squircle relative aspect-video overflow-hidden rounded-[18px] bg-surface ring-1 ring-white/6">
        {clip.posterUrl && (
          <img
            src={clip.posterUrl}
            alt=""
            loading="lazy"
            className="h-full w-full scale-110 object-cover"
          />
        )}
        <span className="absolute top-2.5 left-2.5 rounded-full bg-[rgba(14,15,17,0.8)] px-2.5 py-0.5 text-xs font-bold text-accent-strong">
          Saved for the show
        </span>
        {clip.durationMs != null && (
          <span className="absolute right-2.5 bottom-2.5 rounded-md bg-[rgba(8,9,10,0.75)] px-2 py-0.5 font-mono text-[11px] text-text">
            {formatDuration(clip.durationMs)}
          </span>
        )}
      </div>
      <div className="flex items-center gap-2.5 px-0.5">
        <Avatar name={clip.uploader.displayName} url={clip.uploader.avatarUrl} size={28} />
        <div className="flex min-w-0 flex-col">
          <p dir="auto" className="truncate text-lg leading-tight font-bold">
            {clip.title}
          </p>
          <p className="truncate text-[13px] text-muted">
            <bdi>{clip.uploader.displayName}</bdi>
            {clip.map ? ` · ${clip.map}` : ""}
          </p>
        </div>
      </div>
    </div>
  );
}

/** The trophy of clip of the night. */
export function TrophyIcon({ className = "h-3.5 w-3.5" }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      className={className}
      fill="none"
      stroke="currentColor"
      strokeWidth="2.2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M8 4h8v5a4 4 0 0 1-8 0z" />
      <path d="M8 6H5a3 3 0 0 0 3 4M16 6h3a3 3 0 0 1-3 4M12 13v4M8.5 20h7" />
    </svg>
  );
}
