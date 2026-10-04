import { useState } from "react";
import { type Clip, useClips } from "../clips/hooks";
import { formatDuration } from "../lib/format";
import { Button } from "../ui/Button";
import { Modal } from "../ui/Modal";
import { Panel } from "../ui/Panel";
import { useToast } from "../ui/Toast";
import { type LineupEntry, useShowActions } from "./hooks";

// The show's side panel (canvas 2.2, 2.3): what's still to come, the next one counting
// down, and Add a clip, which anyone in the show can do (to the end of the queue). The
// host can put any of them on straight away. It folds away for a bigger player.
export function UpNext({
  showId,
  upcoming,
  lineupIds,
  remainingMs,
  onPlay,
  onHide,
}: {
  showId: string;
  upcoming: LineupEntry[];
  /** Every clip in the lineup, so Add a clip doesn't offer them again. */
  lineupIds: string[];
  /** Left of the clip that's on, for the next one's "in 0:16"; null when nothing's on. */
  remainingMs: number | null;
  /** The host's: put this one on now. */
  onPlay?: (clipId: string) => void;
  onHide: () => void;
}) {
  const [adding, setAdding] = useState(false);
  const total = upcoming.reduce((sum, l) => sum + (l.clip.durationMs ?? 0), 0);
  return (
    <Panel
      as="aside"
      padding="px-[18px] pt-[18px] pb-3.5"
      className="flex min-h-0 flex-col lg:sticky lg:top-6 lg:h-[calc(100dvh-124px)]"
      aria-label="Up next"
    >
      <div className="flex items-center gap-2 pb-2.5">
        <span className="font-mono text-[11px] whitespace-nowrap text-muted">
          Up next · {upcoming.length} {upcoming.length === 1 ? "clip" : "clips"} ·{" "}
          {formatDuration(total)}
        </span>
        <PanelButton label="Add a clip" onClick={() => setAdding(true)} className="ml-auto">
          <path d="M12 5v14M5 12h14" />
        </PanelButton>
        <PanelButton label="Hide the side panel" onClick={onHide}>
          <path d="M9 6l6 6-6 6" />
        </PanelButton>
      </div>
      <ol className="flex min-h-0 flex-col overflow-y-auto" data-testid="up-next">
        {upcoming.length === 0 && (
          <li className="py-2 text-[15px] text-muted">
            Nothing left. Add one, or it's the finale.
          </li>
        )}
        {upcoming.map((l, i) => {
          const row = (
            <>
              <span
                className="squircle aspect-video w-28 shrink-0 rounded-[13px] bg-surface bg-cover bg-center"
                style={{
                  backgroundImage: l.clip.posterUrl ? `url(${l.clip.posterUrl})` : undefined,
                }}
              />
              <span className="flex min-w-0 flex-col text-left">
                <span dir="auto" className="truncate text-[17px] font-bold">
                  {l.clip.title}
                </span>
                <bdi className="truncate text-[13px] text-muted">{l.clip.uploader.displayName}</bdi>
              </span>
              <span
                className={`ml-auto shrink-0 font-mono text-[11px] ${i === 0 && remainingMs != null ? "text-accent" : "text-muted"}`}
              >
                {i === 0 && remainingMs != null
                  ? `in ${formatDuration(remainingMs)}`
                  : l.clip.durationMs != null && formatDuration(l.clip.durationMs)}
              </span>
            </>
          );
          return (
            <li key={l.clip.id} className={i === 0 ? "" : "opacity-60"}>
              {onPlay ? (
                <button
                  type="button"
                  aria-label={`Play ${l.clip.title} now`}
                  title="Play it now"
                  onClick={() => onPlay(l.clip.id)}
                  className="flex w-full items-center gap-3.5 rounded-xl py-2 text-text hover:bg-white/6"
                >
                  {row}
                </button>
              ) : (
                <div className="flex items-center gap-3.5 py-2">{row}</div>
              )}
            </li>
          );
        })}
      </ol>
      <div className="mt-auto flex flex-col gap-1 border-t border-white/7 pt-3.5">
        <span className="font-mono text-[11px] text-muted">After the last clip</span>
        <span className="text-[15px] text-soft">
          Finale: everyone votes for clip and fail of the night.
        </span>
      </div>
      {/* Mounted only while open: its search asks the archive. */}
      {adding && (
        <AddClip open onClose={() => setAdding(false)} showId={showId} lineupIds={lineupIds} />
      )}
    </Panel>
  );
}

function PanelButton({
  label,
  onClick,
  className = "",
  children,
}: {
  label: string;
  onClick: () => void;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className={`squircle grid h-9 w-9 shrink-0 place-items-center rounded-xl bg-white/8 text-text hover:bg-white/15 ${className}`}
    >
      <svg
        viewBox="0 0 24 24"
        className="h-4 w-4"
        fill="none"
        stroke="currentColor"
        strokeWidth="2.4"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        {children}
      </svg>
    </button>
  );
}

/** Add a clip from the archive to the end of the show's queue. */
function AddClip({
  open,
  onClose,
  showId,
  lineupIds,
}: {
  open: boolean;
  onClose: () => void;
  showId: string;
  lineupIds: string[];
}) {
  const [q, setQ] = useState("");
  const clips = useClips({ q: q.trim() || undefined });
  const { addClip } = useShowActions(showId);
  const toast = useToast();
  const found: Clip[] = (clips.data?.pages.flatMap((p) => p.clips) ?? [])
    .filter((c) => c.status === "ready" && !lineupIds.includes(c.id))
    .slice(0, 12);
  return (
    <Modal open={open} onClose={onClose} title="Add a clip to the show">
      <label className="flex flex-col gap-1.5">
        <span className="text-sm text-soft">Search the archive</span>
        <input
          type="search"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder="Title, map, tag"
          className="input"
        />
      </label>
      <ul className="-mx-2 flex max-h-[50vh] flex-col overflow-y-auto">
        {clips.isSuccess && found.length === 0 && (
          <li className="px-2 py-3 text-soft">No clips to add{q ? " for that search" : ""}.</li>
        )}
        {found.map((c) => (
          <li key={c.id} className="flex items-center gap-3 rounded-xl px-2 py-2 hover:bg-white/5">
            <span
              className="squircle aspect-video w-24 shrink-0 rounded-[11px] bg-surface bg-cover bg-center"
              style={{ backgroundImage: c.posterUrl ? `url(${c.posterUrl})` : undefined }}
            />
            <span className="flex min-w-0 flex-col">
              <span dir="auto" className="truncate font-bold">
                {c.title}
              </span>
              <span className="truncate text-[13px] text-muted">
                <bdi>{c.uploader.displayName}</bdi>
                {c.durationMs != null && ` · ${formatDuration(c.durationMs)}`}
              </span>
            </span>
            <Button
              size="sm"
              className="ml-auto"
              disabled={addClip.isPending}
              aria-label={`Add ${c.title}`}
              onClick={() =>
                addClip.mutate(c.id, {
                  onSuccess: () => {
                    toast(`${c.title} is at the end of the queue.`);
                    onClose();
                  },
                  onError: (err) => toast(`Couldn't add it: ${err.message}`, "danger"),
                })
              }
            >
              Add
            </Button>
          </li>
        ))}
      </ul>
    </Modal>
  );
}
