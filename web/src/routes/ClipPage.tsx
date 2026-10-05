import { getRouteApi, Link } from "@tanstack/react-router";
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { isNotFound } from "../api/errors";
import { useApi } from "../auth/ApiProvider";
import { ClipCard } from "../clips/ClipCard";
import { EditClip } from "../clips/EditClip";
import {
  type Clip,
  downloadOriginal,
  useClip,
  useClips,
  useDeleteClip,
  useHoldClip,
  useRestoreClip,
  useRetryClip,
} from "../clips/hooks";
import { useClipAnalysis, useReanalyse } from "../clips/killfeed/analysis";
import { KillfeedPanel } from "../clips/killfeed/KillfeedPanel";
import { Player } from "../clips/Player";
import { Reactions } from "../clips/Reactions";
import { SharePanel } from "../clips/SharePanel";
import { Kip } from "../kip/Kip";
import { formatBytes, formatDuration, shortDate, timeAgo } from "../lib/format";
import { useTitle } from "../lib/useTitle";
import { Avatar } from "../ui/Avatar";
import { Backdrop } from "../ui/Backdrop";
import { Button, buttonClass } from "../ui/Button";
import { LoadError } from "../ui/LoadError";
import { Modal } from "../ui/Modal";
import { Panel } from "../ui/Panel";
import { useToast } from "../ui/Toast";
import { NotFound } from "./NotFound";
import { useMe } from "./useMe";

const route = getRouteApi("/clips/$clipId");

type Dialog = "edit" | "share" | "delete" | null;

// The clip page (canvas 3.3): the player beside the killfeed panel, the title and actions,
// reactions and tags, then more clips. Edit, Share and Delete open over it (3.4–3.6);
// processing and failed clips get their own screens in the player's place (3.2).
export function ClipPage() {
  const { clipId } = route.useParams();
  const { data: clip, error, isPending, refetch } = useClip(clipId);
  const ready = clip?.status === "ready";
  // Someone else's clip saved for the show: only what tonight's lineup shows of it.
  const teaser = !!clip?.teaser && !clip.isMine;
  const { data: analysis } = useClipAnalysis(clipId, ready && !teaser && clip?.gameId === "cs2");
  useTitle(clip?.title);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [theater, setTheater] = useState(false);
  const toggleTheater = useCallback(() => setTheater((t) => !t), []);
  const video = useRef<HTMLVideoElement>(null);

  if (isPending) {
    return <div className="squircle aspect-video animate-pulse rounded-[26px] bg-surface" />;
  }
  if (isNotFound(error)) {
    return <NotFound />;
  }
  if (error || !clip) {
    return <LoadError error={error ?? new Error("Clip not found.")} onRetry={refetch} />;
  }
  if (teaser) {
    return <Teaser clip={clip} />;
  }

  // The side panel only once there's something to show; while the worker is still reading
  // the killfeed, a one-line notice under the title instead of an empty column.
  const panel = ready && analysis?.status === "done" && !clip.deletedAt;
  const pending = ready && analysis?.status === "pending" && !clip.deletedAt;
  const kills = analysis?.status === "done" ? analysis.kills : [];
  // Wide screens: the player beside the killfeed panel, the panel as tall as the player
  // (its row is sized by the player alone), the player capped so the title and tags fit
  // on screen under it. Theater mode gives the player the whole width.
  const layout =
    panel && !theater
      ? "lg:grid-cols-[minmax(0,calc((100svh-14rem)*16/9))_22rem] lg:justify-center lg:gap-x-6"
      : "mx-auto max-w-[calc((100svh-12rem)*16/9)]";

  return (
    <article className={`grid gap-y-5 ${layout}`}>
      <Backdrop image={clip.posterUrl} />
      <div className="min-w-0">
        {clip.deletedAt && <TrashBanner clip={clip} />}
        {ready && clip.playbackUrl ? (
          <Player
            src={clip.playbackUrl}
            poster={clip.posterUrl}
            fps={clip.fps}
            durationS={clip.durationMs != null ? clip.durationMs / 1000 : null}
            kills={kills}
            videoRef={video}
            theater={theater}
            onToggleTheater={toggleTheater}
          />
        ) : clip.status === "failed" ? (
          <Failed clip={clip} onDelete={() => setDialog("delete")} />
        ) : ready ? (
          <Unplayable onRetry={refetch} />
        ) : (
          <Processing clip={clip} onDelete={() => setDialog("delete")} />
        )}
      </div>

      <div className="min-w-0 lg:col-start-1">
        <Details clip={clip} onOpen={setDialog} />
        {pending && analysis && (
          <div className="mt-4">
            <KillfeedPanel analysis={analysis} videoRef={video} />
          </div>
        )}
      </div>

      {panel && (
        <aside
          className={
            theater ? "min-w-0" : "min-w-0 lg:col-start-2 lg:row-start-1 lg:h-0 lg:min-h-full"
          }
        >
          <KillfeedPanel analysis={analysis} videoRef={video} />
        </aside>
      )}

      {ready && !clip.deletedAt && (
        <MoreClips clip={clip} className={panel && !theater ? "lg:col-span-2" : ""} />
      )}

      <Modal open={dialog === "edit"} onClose={() => setDialog(null)} title="Edit clip">
        <EditClip clip={clip} onDone={() => setDialog(null)} />
      </Modal>
      <Modal open={dialog === "share"} onClose={() => setDialog(null)} title="Share link">
        <SharePanel clip={clip} />
      </Modal>
      <Modal
        open={dialog === "delete"}
        onClose={() => setDialog(null)}
        title="Delete this clip?"
        width="max-w-md"
      >
        <DeleteClip clip={clip} onDone={() => setDialog(null)} />
      </Modal>
    </article>
  );
}

function Details({ clip, onOpen }: { clip: Clip; onOpen: (d: Dialog) => void }) {
  const live = clip.status === "ready" && !clip.deletedAt;
  return (
    <div className="flex flex-col gap-4">
      {/* Title block and actions stack on phones and sit side by side from `sm` up. */}
      <div className="flex flex-col gap-3 sm:flex-row sm:items-start">
        <div className="flex min-w-0 flex-1 items-center gap-3">
          <Link
            to="/u/$handle"
            params={{ handle: clip.uploader.handle }}
            aria-label={clip.uploader.displayName}
            className="shrink-0"
          >
            <Avatar name={clip.uploader.displayName} url={clip.uploader.avatarUrl} size={44} />
          </Link>
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            {/* File-name titles can be one long word; let them break anywhere. */}
            <h1
              dir="auto"
              className="text-[28px] leading-none font-extrabold tracking-tight [overflow-wrap:anywhere] sm:text-[34px]"
            >
              {clip.title}
            </h1>
            <p className="text-[15px] text-soft">
              <Link
                to="/u/$handle"
                params={{ handle: clip.uploader.handle }}
                dir="auto"
                className="hover:text-text"
              >
                {clip.uploader.displayName}
              </Link>
              {" · "}
              {clip.status === "ready" ? shortDate(clip.createdAt) : timeAgo(clip.createdAt)}
              {clip.map && (
                <>
                  {" · "}
                  <Link
                    to="/"
                    search={{ map: clip.map }}
                    className="text-text underline decoration-text/30 hover:decoration-text"
                  >
                    {clip.map}
                  </Link>
                </>
              )}
              {clip.durationMs != null && ` · ${formatDuration(clip.durationMs)}`}
              {clip.height != null && ` · ${clip.height}p${clip.fps ? Math.round(clip.fps) : ""}`}
              {clip.status !== "ready" && " · only you can see it"}
            </p>
          </div>
        </div>
        <Actions clip={clip} onOpen={onOpen} />
      </div>

      {clip.heldUntil && clip.isMine && !clip.deletedAt && <HeldNotice clip={clip} />}
      {live && (
        <div className="flex flex-wrap items-center gap-2">
          <Reactions clip={clip} />
          <Tags clip={clip} />
        </div>
      )}
      {clip.description && (
        <p className="max-w-[680px] text-base leading-relaxed whitespace-pre-line text-soft [overflow-wrap:anywhere]">
          {clip.description}
        </p>
      )}
    </div>
  );
}

/** Friends in the clip, then tags: the killfeed's own (amber, with a crosshair) first. */
function Tags({ clip }: { clip: Clip }) {
  if (clip.players.length + clip.tags.length + clip.autoTags.length === 0) return null;
  return (
    <>
      <span aria-hidden="true" className="mx-1.5 h-6 w-px bg-white/12" />
      {clip.autoTags.map((t) => (
        <Link
          key={t}
          to="/"
          search={{ tag: t }}
          title="Detected from the killfeed"
          className="flex h-[30px] items-center gap-1.5 rounded-full bg-accent/14 pr-3 pl-2.5 text-[13px] font-bold text-accent-strong shadow-[inset_0_0_0_1px_rgba(245,165,36,0.4)] hover:bg-accent/22"
        >
          <svg
            viewBox="0 0 24 24"
            width="13"
            height="13"
            fill="none"
            stroke="var(--color-accent)"
            strokeWidth="2.2"
            strokeLinecap="round"
            aria-hidden="true"
          >
            <circle cx="12" cy="12" r="7" />
            <path d="M12 2v5M12 17v5M2 12h5M17 12h5" />
          </svg>
          #{t}
          <span className="sr-only"> (detected from the killfeed)</span>
        </Link>
      ))}
      {clip.players.map((p) => (
        <Link
          key={p.id}
          to="/u/$handle"
          params={{ handle: p.handle }}
          className="flex h-[30px] items-center gap-1.5 rounded-full bg-white/8 pr-3 pl-[3px] text-[13px] font-semibold hover:bg-white/12"
        >
          <Avatar name={p.displayName} url={p.avatarUrl} size={24} />
          <bdi>{p.displayName}</bdi>
        </Link>
      ))}
      {clip.tags.map((t) => (
        <Link
          key={t}
          to="/"
          search={{ tag: t }}
          className="flex h-[30px] items-center rounded-full bg-white/5 px-3 text-[13px] text-soft shadow-[inset_0_0_0_1px_rgba(255,255,255,0.1)] hover:text-text"
        >
          #{t}
        </Link>
      ))}
    </>
  );
}

/** The uploader's other clips, newest first, topped up with everyone's latest. */
function MoreClips({ clip, className }: { clip: Clip; className: string }) {
  const theirs = useClips({ uploader: clip.uploader.handle });
  const recent = useClips({});
  const seen = new Set([clip.id]);
  const pick = (list: Clip[] | undefined, n: number) =>
    (list ?? [])
      .filter((c) => c.status === "ready" && !seen.has(c.id) && seen.add(c.id))
      .slice(0, n);
  const fromThem = pick(
    theirs.data?.pages[0]?.clips.filter((c) => c.uploader.handle === clip.uploader.handle),
    8,
  );
  const clips = [
    ...fromThem,
    ...pick(recent.data?.pages[0]?.clips, Math.max(0, 4 - fromThem.length)),
  ];
  if (clips.length === 0) return null;
  return (
    <section className={`flex min-w-0 flex-col gap-4 pt-6 ${className}`}>
      <div className="flex flex-wrap items-end gap-3">
        <h2 className="text-[30px] leading-none font-extrabold tracking-tight">
          {fromThem.length > 0 ? (
            <>
              More from <bdi>{clip.uploader.displayName}</bdi>
            </>
          ) : (
            "More clips"
          )}
        </h2>
        {fromThem.length > 0 && (
          <Link
            to="/"
            search={{ uploader: clip.uploader.handle }}
            className="ml-auto py-1.5 font-mono text-xs text-accent hover:text-accent-strong"
          >
            All of <bdi>{clip.uploader.displayName}</bdi>'s clips
          </Link>
        )}
      </div>
      <div className="grid gap-x-6 gap-y-8 sm:grid-cols-2 lg:grid-cols-4">
        {clips.map((c) => (
          <ClipCard key={c.id} clip={c} />
        ))}
      </div>
    </section>
  );
}

function ActionButton({
  icon,
  children,
  ...rest
}: { icon: string; children: ReactNode } & React.ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      type="button"
      className="frost flex h-11 items-center gap-2 rounded-full pr-4 pl-3.5 text-[15px] font-semibold text-text transition hover:bg-white/14 disabled:opacity-50"
      {...rest}
    >
      <Icon d={icon} />
      {children}
    </button>
  );
}

function Icon({ d, size = 17 }: { d: string; size?: number }) {
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth="2.2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d={d} />
    </svg>
  );
}

const ICONS = {
  share: "M14 5h5v5M19 5l-8 8M10 5H6a1 1 0 0 0-1 1v12a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-4",
  download: "M12 4v11M7 10l5 5 5-5M5 20h14",
  edit: "M4 20h4L19 9l-4-4L4 16z",
  trash: "M5 7h14M10 7V5h4v2M7 7l1 12h8l1-12",
  reanalyse: "M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5",
};

/** "Download": the original file, for any clip whose upload finished. */
function useDownload(clip: Clip) {
  const api = useApi();
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  return {
    busy,
    title: `The original: ${clip.originalFilename} · ${formatBytes(clip.originalBytes)}`,
    start: async () => {
      setBusy(true);
      try {
        await downloadOriginal(api, clip.id);
      } catch (err) {
        toast(`Couldn't download it: ${errorMessage(err)}`, "danger");
      } finally {
        setBusy(false);
      }
    },
  };
}

const errorMessage = (err: unknown) => (err instanceof Error ? err.message : String(err));

/** "Re-analyse kill feed": admins have the worker read a clip's killfeed again (after a fix
 *  to the analysis, say). Only CS2 clips from the uploader's view are read at all, so the
 *  others don't get it (undefined). */
function useReanalyseAction(clip: Clip) {
  const { data: me } = useMe();
  const reanalyse = useReanalyse(clip.id);
  const toast = useToast();
  if (me?.role !== "admin" || clip.gameId !== "cs2" || !clip.myPov) return undefined;
  return () =>
    reanalyse.mutate(undefined, {
      onSuccess: () => toast("Re-analysing — the kill feed updates in a minute or two."),
      onError: (err) => toast(`Couldn't re-analyse it: ${errorMessage(err)}`, "danger"),
    });
}

function Actions({ clip, onOpen }: { clip: Clip; onOpen: (d: Dialog) => void }) {
  const download = useDownload(clip);
  const reanalyse = useReanalyseAction(clip);
  const live = !clip.deletedAt;
  // Processing and failed clips have their own screen with its own way forward.
  if (clip.status !== "ready") return null;
  return (
    <div className="flex shrink-0 flex-wrap gap-2">
      {clip.canEdit && live && (
        <ActionButton icon={ICONS.share} onClick={() => onOpen("share")}>
          {clip.shareUrl ? "Shared" : "Share"}
        </ActionButton>
      )}
      <ActionButton
        icon={ICONS.download}
        disabled={download.busy}
        title={download.title}
        onClick={download.start}
      >
        Download
      </ActionButton>
      {clip.canEdit && live && (
        <>
          <ActionButton icon={ICONS.edit} onClick={() => onOpen("edit")}>
            Edit
          </ActionButton>
          <MoreMenu onDelete={() => onOpen("delete")} onReanalyse={reanalyse} />
        </>
      )}
    </div>
  );
}

/** "…": the rarer actions, out of the way: Delete, and for admins Re-analyse kill feed. A
 *  menu button: opening it focuses the first item, the arrow keys move between items, and
 *  Escape and Tab close it with focus back on "…" (where a dialog opened from it returns
 *  focus too). */
function MoreMenu({ onDelete, onReanalyse }: { onDelete: () => void; onReanalyse?: () => void }) {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  const items = () => [...(box.current?.querySelectorAll<HTMLElement>("[role=menuitem]") ?? [])];
  const close = () => {
    setOpen(false);
    button.current?.focus();
  };
  useEffect(() => {
    if (!open) return;
    box.current?.querySelector<HTMLElement>("[role=menuitem]")?.focus();
    // A click anywhere else closes it; focus goes where the click put it.
    const outside = (e: PointerEvent) => {
      if (!box.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);
  const onKey = (e: React.KeyboardEvent) => {
    const list = items();
    const at = list.indexOf(document.activeElement as HTMLElement);
    const go = (i: number) => list[(i + list.length) % list.length]?.focus();
    switch (e.key) {
      case "ArrowDown":
        go(at + 1);
        break;
      case "ArrowUp":
        go(at - 1);
        break;
      case "Home":
        go(0);
        break;
      case "End":
        go(list.length - 1);
        break;
      case "Escape":
      case "Tab":
        close();
        break;
      default:
        return;
    }
    e.preventDefault();
  };
  return (
    <div ref={box} className="relative">
      <button
        ref={button}
        type="button"
        aria-label="More actions"
        aria-expanded={open}
        aria-haspopup="menu"
        onClick={() => setOpen((o) => !o)}
        className="frost grid h-11 w-11 place-items-center rounded-full hover:bg-white/14"
      >
        <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor" aria-hidden="true">
          <circle cx="5" cy="12" r="1.6" />
          <circle cx="12" cy="12" r="1.6" />
          <circle cx="19" cy="12" r="1.6" />
        </svg>
      </button>
      {open && (
        <div
          role="menu"
          tabIndex={-1}
          onKeyDown={onKey}
          className="glass squircle absolute top-12 right-0 z-30 flex min-w-44 flex-col rounded-2xl p-1.5 shadow-[0_20px_40px_-20px_rgba(0,0,0,0.9)]"
        >
          {onReanalyse && (
            <button
              type="button"
              role="menuitem"
              tabIndex={-1}
              onClick={() => {
                close();
                onReanalyse();
              }}
              className="flex h-10 items-center gap-2.5 rounded-xl px-3 text-left text-[15px] font-semibold whitespace-nowrap text-text hover:bg-white/10"
            >
              <Icon d={ICONS.reanalyse} />
              Re-analyse kill feed
            </button>
          )}
          <button
            type="button"
            role="menuitem"
            tabIndex={-1}
            onClick={() => {
              close();
              onDelete();
            }}
            className="flex h-10 items-center gap-2.5 rounded-xl px-3 text-left text-[15px] font-semibold text-danger hover:bg-danger/10"
          >
            <Icon d={ICONS.trash} />
            Delete
          </button>
        </div>
      )}
    </div>
  );
}

/** The Delete dialog's body (canvas 3.6). */
function DeleteClip({ clip, onDone }: { clip: Clip; onDone: () => void }) {
  const remove = useDeleteClip(clip.id);
  const toast = useToast();
  return (
    <div className="flex flex-col gap-5">
      <p className="text-[15px] text-soft">
        You can restore it for 7 days from Trash on your profile. After that it's gone for good.
      </p>
      {remove.error && (
        <p role="alert" className="text-sm text-danger">
          {remove.error.message}
        </p>
      )}
      <div className="flex gap-2">
        <Button
          variant="danger"
          disabled={remove.isPending}
          onClick={() =>
            remove.mutate(undefined, {
              onSuccess: () => {
                toast("Moved to the trash. You can restore it for 7 days.");
                onDone();
              },
            })
          }
        >
          Delete
        </Button>
        <Button variant="ghost" onClick={onDone}>
          Keep it
        </Button>
      </div>
    </div>
  );
}

/** Your clip saved for the show (S3): who can see it, and "Post now". */
function HeldNotice({ clip }: { clip: Clip }) {
  const { release } = useHoldClip(clip.id);
  const toast = useToast();
  return (
    <div className="squircle flex flex-wrap items-center gap-3 rounded-2xl bg-accent/10 px-4 py-3 ring-1 ring-accent/35 ring-inset">
      <Kip className="h-9 w-9 shrink-0" />
      <div className="flex min-w-0 flex-1 flex-col">
        <span className="font-bold">Saved for the show</span>
        <span className="text-sm text-soft">
          Nobody else sees it until it plays in a show, or until{" "}
          {shortDate(clip.heldUntil ?? clip.createdAt)}.
        </span>
        {release.error && (
          <span role="alert" className="text-sm text-danger">
            {release.error.message}
          </span>
        )}
      </div>
      <Button
        size="sm"
        disabled={release.isPending}
        onClick={() =>
          release.mutate(undefined, {
            onSuccess: () => toast("Posted. Everyone can see it now."),
          })
        }
      >
        Post now
      </Button>
    </div>
  );
}

function TrashBanner({ clip }: { clip: Clip }) {
  const restore = useRestoreClip(clip.id);
  const toast = useToast();
  const purgeAt = new Date(new Date(clip.deletedAt ?? 0).getTime() + 7 * 86400_000);
  return (
    <div className="squircle mb-4 flex flex-wrap items-center justify-between gap-3 rounded-2xl bg-danger/10 px-4 py-3 text-sm ring-1 ring-danger/40 ring-inset">
      <span>
        In the trash. Only you can see it; it's deleted for good on{" "}
        {shortDate(purgeAt.toISOString())}.
      </span>
      {clip.canEdit && (
        <Button
          size="sm"
          onClick={() =>
            restore.mutate(undefined, {
              onError: (err) => toast(`Couldn't restore it: ${err.message}`, "danger"),
            })
          }
          disabled={restore.isPending}
        >
          Restore
        </Button>
      )}
    </div>
  );
}

const PANEL_PADDING = "p-6 sm:p-10";

/** Someone else's clip saved for the show (S3): what tonight's lineup shows of it (title,
 *  uploader, map, length and the poster blurred), and nothing to play, react to or
 *  download until it plays in a show. */
function Teaser({ clip }: { clip: Clip }) {
  return (
    <article className="mx-auto grid w-full max-w-[calc((100svh-12rem)*16/9)] gap-y-5">
      <Backdrop image={clip.posterUrl} />
      <div className="squircle relative -mx-4 aspect-video overflow-hidden bg-surface ring-1 ring-white/6 sm:mx-0 sm:rounded-[26px]">
        {clip.posterUrl && (
          <img
            src={clip.posterUrl}
            alt=""
            className="h-full w-full scale-110 object-cover blur-xl brightness-75"
          />
        )}
        <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 px-6 text-center">
          <Kip pose="cheer" className="h-20 w-20 sm:h-32 sm:w-32" />
          <span className="text-shadow text-2xl font-extrabold sm:text-3xl">
            Saved for the show
          </span>
          <span className="text-shadow max-w-md text-soft">
            Everyone sees it when it plays in a show.
          </span>
        </div>
        {clip.durationMs != null && (
          <span className="absolute right-3 bottom-3 rounded-md bg-[rgba(8,9,10,0.75)] px-2 py-0.5 font-mono text-xs">
            {formatDuration(clip.durationMs)}
          </span>
        )}
      </div>
      <div className="flex min-w-0 items-center gap-3">
        <Link
          to="/u/$handle"
          params={{ handle: clip.uploader.handle }}
          aria-label={clip.uploader.displayName}
          className="shrink-0"
        >
          <Avatar name={clip.uploader.displayName} url={clip.uploader.avatarUrl} size={44} />
        </Link>
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <h1
            dir="auto"
            className="text-[28px] leading-none font-extrabold tracking-tight [overflow-wrap:anywhere] sm:text-[34px]"
          >
            {clip.title}
          </h1>
          <p className="text-[15px] text-soft">
            <Link
              to="/u/$handle"
              params={{ handle: clip.uploader.handle }}
              dir="auto"
              className="hover:text-text"
            >
              {clip.uploader.displayName}
            </Link>
            {clip.map && ` · ${clip.map}`}
            {clip.durationMs != null && ` · ${formatDuration(clip.durationMs)}`}
          </p>
        </div>
      </div>
    </article>
  );
}

/** Ready, but no link to play it came with it: say so instead of waiting forever. */
function Unplayable({ onRetry }: { onRetry: () => void }) {
  return (
    <Panel
      padding={PANEL_PADDING}
      className="flex flex-col items-center gap-8 sm:flex-row sm:gap-12"
    >
      <Kip pose="knocked-out" className="h-32 w-32 shrink-0 sm:h-44 sm:w-44" />
      <div className="flex flex-col gap-3">
        <h2 className="text-3xl font-extrabold tracking-tight sm:text-4xl">
          It won't play right now.
        </h2>
        <p className="max-w-lg text-[17px] text-soft">The link to play it didn't come through.</p>
        <Button className="self-start" onClick={() => onRetry()}>
          Try again
        </Button>
      </div>
    </Panel>
  );
}

/** Processing (canvas 3.2): what Kip is doing with the upload. Updates by itself. */
function Processing({ clip, onDelete }: { clip: Clip; onDelete: () => void }) {
  const download = useDownload(clip);
  // Still `uploading`: the file isn't all there yet (the upload page is still sending it,
  // or it was left half-way).
  const uploading = clip.status === "uploading";
  const steps: { label: string; detail: string; state: "done" | "now" | "next" }[] = [
    {
      label: uploading ? "Uploading" : "Uploaded",
      detail: uploading
        ? "from the upload page"
        : [
            formatBytes(clip.originalBytes),
            clip.durationMs != null && formatDuration(clip.durationMs),
          ]
            .filter(Boolean)
            .join(" · "),
      state: uploading ? "now" : "done",
    },
    {
      label: "Making it play everywhere",
      detail: "usually under a minute",
      state: uploading ? "next" : "now",
    },
    ...(clip.gameId === "cs2"
      ? [
          {
            label: "Reading the killfeed",
            detail: "kill marks and stats show up after",
            state: "next" as const,
          },
        ]
      : []),
  ];
  return (
    <Panel
      padding={PANEL_PADDING}
      className="flex flex-col items-center gap-8 sm:flex-row sm:gap-12"
    >
      <Kip pose="cheer" className="h-32 w-32 shrink-0 sm:h-44 sm:w-44" />
      <div className="flex w-full flex-col gap-5">
        <h2 className="text-3xl font-extrabold tracking-tight sm:text-4xl">
          Kip's getting it ready
        </h2>
        <ol className="flex flex-col gap-3.5">
          {steps.map((s) => (
            <li key={s.label} className="flex items-center gap-3.5">
              <span
                aria-hidden="true"
                className={`grid h-8 w-8 shrink-0 place-items-center rounded-full ${
                  s.state === "done"
                    ? "bg-green/20 text-green"
                    : s.state === "now"
                      ? "bg-accent/16"
                      : "bg-white/6"
                }`}
              >
                {s.state === "done" ? (
                  <Icon d="M5 12l5 5 9-10" size={16} />
                ) : s.state === "now" ? (
                  <span className="h-4 w-4 animate-spin rounded-full border-2 border-accent/30 border-t-accent" />
                ) : (
                  <span className="h-2 w-2 rounded-full bg-muted" />
                )}
              </span>
              <span className="flex flex-col">
                <span className={`text-lg font-bold ${s.state === "next" ? "text-muted" : ""}`}>
                  {s.label}
                  <span className="sr-only">
                    {s.state === "done" ? " (done)" : s.state === "now" ? " (now)" : " (next)"}
                  </span>
                </span>
                <span className="text-[13px] text-muted">{s.detail}</span>
              </span>
            </li>
          ))}
        </ol>
        <p className="text-sm text-soft">
          This page updates by itself. You can leave it.
          {/* An upload left half-way never finishes on its own. */}
          {uploading && clip.canEdit && " If the upload stopped, delete this and upload it again."}
        </p>
        <div className="flex flex-wrap gap-2">
          {!uploading && (
            <Button onClick={download.start} disabled={download.busy} title={download.title}>
              Download original
            </Button>
          )}
          {clip.canEdit && !clip.deletedAt && <Button onClick={onDelete}>Delete</Button>}
        </div>
      </div>
    </Panel>
  );
}

/** Failed (canvas 3.2): why, in words, and the way forward. */
function Failed({ clip, onDelete }: { clip: Clip; onDelete: () => void }) {
  const retry = useRetryClip(clip.id);
  const length = clip.error?.match(/this one is (\d+:\d{2})/)?.[1];
  const [headline, body] = {
    tooLong: [
      "Too long to keep.",
      `Clips can be at most 5 minutes.${length ? ` This one is ${length}.` : ""} Trim it in your recorder and upload the shorter one.`,
    ],
    notAVideo: [
      "No video in that file.",
      "Pick the recording itself, not its audio or a project file.",
    ],
    unreadable: [
      "Kip can't read that file.",
      "It may be cut off or corrupt. Export it again from your recorder and upload that.",
    ],
    duplicate: [
      "It's here already.",
      "This exact file was uploaded already, as another clip. One copy is plenty, so this one can go.",
    ],
    server: ["Something broke on our side.", "Try again in a moment."],
  }[clip.failureReason ?? "server"];
  const own = clip.isMine;
  const toast = useToast();
  const download = useDownload(clip);
  return (
    <Panel
      padding={PANEL_PADDING}
      className="flex flex-col items-center gap-8 sm:flex-row sm:gap-12"
    >
      <Kip pose="knocked-out" className="h-32 w-32 shrink-0 sm:h-44 sm:w-44" />
      <div className="flex flex-col gap-3">
        <span className="font-mono text-xs text-danger">Processing failed</span>
        <h2 className="text-4xl font-extrabold tracking-tight">{headline}</h2>
        <p className="max-w-lg text-[17px] text-soft">{body}</p>
        <div className="mt-2 flex flex-wrap gap-2">
          {own &&
            (clip.failureReason === "duplicate" ? (
              clip.duplicateOf && (
                <Link
                  to="/clips/$clipId"
                  params={{ clipId: clip.duplicateOf }}
                  className={buttonClass("primary", "lg")}
                >
                  Open that one
                </Link>
              )
            ) : clip.failureReason && clip.failureReason !== "server" ? (
              <Link to="/upload" className={buttonClass("primary", "lg")}>
                {clip.failureReason === "tooLong" ? "Upload a shorter one" : "Upload another file"}
              </Link>
            ) : (
              <Button
                variant="primary"
                size="lg"
                onClick={() =>
                  retry.mutate(undefined, {
                    onError: (err) => toast(`Couldn't try again: ${err.message}`, "danger"),
                  })
                }
                disabled={retry.isPending}
              >
                Try again
              </Button>
            ))}
          <Button
            size="lg"
            onClick={download.start}
            disabled={download.busy}
            title={download.title}
          >
            Download original
          </Button>
          {clip.canEdit && !clip.deletedAt && (
            <Button size="lg" onClick={onDelete}>
              Delete
            </Button>
          )}
        </div>
      </div>
    </Panel>
  );
}
