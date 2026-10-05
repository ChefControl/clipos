import { useQueryClient } from "@tanstack/react-query";
import { Link, useBlocker, useNavigate } from "@tanstack/react-router";
import {
  type FormEvent,
  type MutableRefObject,
  type ReactNode,
  useEffect,
  useReducer,
  useRef,
  useState,
} from "react";
import { call } from "../api/errors";
import { useApi } from "../auth/ApiProvider";
import { type Clip, useMembers, useRestoreClip } from "../clips/hooks";
import { Kip } from "../kip/Kip";
import { findCopy } from "../lib/fingerprint";
import { formatBytes, formatDuration, timeAgo, titleFromFilename } from "../lib/format";
import { CS2_MAPS } from "../lib/maps";
import { type UploadProgress, uploadToBlob } from "../lib/upload";
import { useTitle } from "../lib/useTitle";
import { Avatar } from "../ui/Avatar";
import { Button, buttonClass } from "../ui/Button";
import { Chip } from "../ui/Chip";
import { Modal } from "../ui/Modal";
import { Switch } from "../ui/Switch";
import { useToast } from "../ui/Toast";
import { useMe } from "./useMe";

const MAX_BYTES = 2 * 1024 * 1024 * 1024;
const EXTENSIONS = [".mp4", ".mkv", ".mov"];
const LIMITS =
  ".mp4, .mkv or .mov, up to 2 GB and 5 minutes. Straight from ShadowPlay, Medal or OBS.";

type Phase =
  /** Making sure it isn't here already; `hashed` (0 to 1) once it's compared in full. */
  | { kind: "checking"; hashed: number | null }
  /** It's here already, as `clip` when you may open it: not uploaded. */
  | { kind: "duplicate"; clip: Clip | null }
  /** It's one of your clips in the trash: restore it, or upload it as new. */
  | { kind: "inTrash"; clip: Clip }
  /** Restored from the trash instead of uploading it. */
  | { kind: "restored"; clip: Clip }
  | { kind: "starting" }
  | { kind: "uploading"; progress: UploadProgress; startedAt: number }
  | { kind: "finishing" }
  | { kind: "done" }
  | { kind: "error"; message: string };

type Rejection = { headline: string; body: string };
type Rejected = Rejection & { file: File };

/** Why `file` can't be uploaded, or null when it can. */
function rejection(file: File): Rejection | null {
  const lower = file.name.toLowerCase();
  if (!EXTENSIONS.some((ext) => lower.endsWith(ext))) {
    return {
      headline: "That's not a video file.",
      body: "Pick an .mp4, .mkv or .mov file, straight from your recorder.",
    };
  }
  if (file.size === 0) {
    return {
      headline: "That file is empty.",
      body: "It's 0 bytes, so there's nothing in it to upload. If your recorder is still saving it, give it a moment and pick it again.",
    };
  }
  if (file.size > MAX_BYTES) {
    return {
      headline: `That one's ${formatBytes(file.size)}.`,
      body: "Clips can be at most 2 GB. Trim it in your recorder first, or pick another file.",
    };
  }
  return null;
}

type Mode = { kind: "pick" } | { kind: "one"; file: File } | { kind: "batch"; files: File[] };

/** Asks whether `file` is here already (decision 55): by its samples, then, if they match
 *  something, by all of it. */
function useFindCopy() {
  const api = useApi();
  return (file: File, onHashing: (fraction: number) => void, signal: AbortSignal) =>
    findCopy(
      file,
      (body) => call(api.POST("/api/clips/check", { body, signal })),
      onHashing,
      signal,
    );
}

// Upload (canvas 3.1). Picking a file starts the upload straight away; the details are
// filled in beside it while it goes, and saved to the clip with "Save details". Picking
// several queues them up instead, one after another, with their details left for later.
export function Upload() {
  const [mode, setMode] = useState<Mode>({ kind: "pick" });
  const [rejected, setRejected] = useState<Rejected | null>(null);
  const [dragging, setDragging] = useState(false);
  const busy = useRef(false);
  /** Set while the queue is showing: what's picked then joins it. */
  const addToBatch = useRef<((files: File[]) => void) | null>(null);
  useTitle("Upload");

  const pick = (picked: File[]) => {
    if (picked.length === 0 || busy.current) return;
    if (addToBatch.current) {
      addToBatch.current(picked);
      return;
    }
    setRejected(null);
    if (picked.length > 1) {
      setMode({ kind: "batch", files: picked });
      return;
    }
    const file = picked[0];
    if (!file) return;
    const no = rejection(file);
    if (no) setRejected({ file, ...no });
    else setMode({ kind: "one", file });
  };
  const pickRef = useRef(pick);
  pickRef.current = pick;

  // Drag and drop anywhere on the page. A depth counter tracks enter/leave across child
  // elements; every drop is swallowed so a near miss never opens the video in the tab.
  useEffect(() => {
    let depth = 0;
    const hasFiles = (e: DragEvent) => e.dataTransfer?.types.includes("Files") ?? false;
    const enter = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth += 1;
      setDragging(true);
    };
    const over = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
    };
    const leave = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      depth = Math.max(0, depth - 1);
      if (depth === 0) setDragging(false);
    };
    const drop = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth = 0;
      setDragging(false);
      pickRef.current([...(e.dataTransfer?.files ?? [])]);
    };
    window.addEventListener("dragenter", enter);
    window.addEventListener("dragover", over);
    window.addEventListener("dragleave", leave);
    window.addEventListener("drop", drop);
    return () => {
      window.removeEventListener("dragenter", enter);
      window.removeEventListener("dragover", over);
      window.removeEventListener("dragleave", leave);
      window.removeEventListener("drop", drop);
    };
  }, []);

  return (
    <section className="flex flex-col gap-6">
      <div className="flex flex-col gap-1.5">
        <h1 className="text-[40px] leading-[0.95] font-extrabold tracking-tight sm:text-[52px]">
          {mode.kind === "batch" ? "Upload clips" : "Upload a clip"}
        </h1>
        <p className="text-base text-soft">{LIMITS}</p>
      </div>
      {mode.kind === "one" ? (
        <Uploading
          key={`${mode.file.name}-${mode.file.size}-${mode.file.lastModified}`}
          file={mode.file}
          onBusy={(b) => {
            busy.current = b;
          }}
          onRestart={() => {
            busy.current = false;
            setMode({ kind: "pick" });
          }}
        />
      ) : mode.kind === "batch" ? (
        <Batch files={mode.files} add={addToBatch} />
      ) : (
        <DropZone rejected={rejected} dragging={dragging} onPick={pick} />
      )}
    </section>
  );
}

function DropZone({
  rejected,
  dragging,
  onPick,
}: {
  rejected: Rejected | null;
  dragging: boolean;
  onPick: (files: File[]) => void;
}) {
  return (
    <div
      className={`squircle flex flex-col items-center justify-center gap-4 rounded-[34px] px-6 py-10 text-center outline-2 -outline-offset-2 outline-dashed sm:px-10 sm:py-12 ${
        dragging
          ? "bg-accent/10 outline-accent"
          : rejected
            ? "bg-danger/6 outline-danger"
            : "bg-white/3 outline-text/28"
      }`}
    >
      <Kip
        pose={rejected ? "knocked-out" : dragging ? "cheer" : "idle"}
        className="h-32 w-32 drop-shadow-[0_10px_14px_rgba(0,0,0,0.45)] sm:h-44 sm:w-44"
      />
      {rejected ? (
        <>
          <h2 className="text-[28px] leading-tight font-extrabold sm:text-[34px]">
            {rejected.headline}
          </h2>
          <p className="max-w-[460px] text-[17px] text-soft">{rejected.body}</p>
          <p className="font-mono text-[11px] [overflow-wrap:anywhere] text-[#f08a8d]">
            {rejected.file.name} · {formatBytes(rejected.file.size)}
          </p>
        </>
      ) : (
        <>
          <h2 className="text-[28px] leading-tight font-extrabold sm:text-[34px]">
            {dragging ? "Drop it." : "Drop a clip anywhere on this page"}
          </h2>
          <p className="max-w-[460px] text-[17px] text-soft">
            {LIMITS} Several at once go up one after another.
          </p>
        </>
      )}
      <FilePicker onPick={onPick}>{rejected ? "Choose another file" : "Choose a video"}</FilePicker>
    </div>
  );
}

/** The upload button and its hidden file input; picks one file or several. */
function FilePicker({
  onPick,
  children,
  variant = "primary",
  size = "lg",
  className = "mt-1.5",
}: {
  onPick: (files: File[]) => void;
  children: string;
  variant?: "primary" | "secondary";
  size?: "md" | "lg";
  className?: string;
}) {
  const input = useRef<HTMLInputElement>(null);
  return (
    <>
      <input
        ref={input}
        type="file"
        multiple
        accept={EXTENSIONS.join(",")}
        className="sr-only"
        tabIndex={-1}
        aria-hidden="true"
        onChange={(e) => {
          onPick([...(e.target.files ?? [])]);
          e.target.value = "";
        }}
      />
      <Button
        variant={variant}
        size={size}
        className={className}
        onClick={() => input.current?.click()}
      >
        <svg
          viewBox="0 0 24 24"
          className="h-[18px] w-[18px]"
          fill="none"
          stroke="currentColor"
          strokeWidth="2.4"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <path d="M12 16V4M7 9l5-5 5 5M5 20h14" />
        </svg>
        {children}
      </Button>
    </>
  );
}

function Uploading({
  file,
  onBusy,
  onRestart,
}: {
  file: File;
  onBusy: (busy: boolean) => void;
  onRestart: () => void;
}) {
  const api = useApi();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const members = useMembers();
  const { data: me } = useMe();
  const toast = useToast();
  // "Save it for the show" (S3): offered once the show is open to you; on by default
  // once it's open to everyone (decision 29). Applies to the clip as soon as it changes.
  const [hold, setHold] = useState(() => !!me?.showsForEveryone);
  const holdTouched = useRef(false);
  const holdAtUpload = useRef(hold);
  const [serverHold, setServerHold] = useState(false);
  const holdSending = useRef(false);
  // /api/me came in after the page opened: its default, unless you've chosen already.
  const holdDefault = !!me?.showsForEveryone;
  useEffect(() => {
    if (!holdTouched.current) setHold(holdDefault);
  }, [holdDefault]);
  // Friends in the clip: everyone but you.
  const friends = members.data?.filter((m) => m.id !== me?.id) ?? [];
  const findCopy = useFindCopy();
  const [phase, setPhase] = useState<Phase>({ kind: "checking", hashed: null });
  const [clipId, setClipId] = useState<string | null>(null);
  const abort = useRef<AbortController | null>(null);

  const [title, setTitle] = useState(() => titleFromFilename(file.name));
  const [map, setMap] = useState("");
  const [myPov, setMyPov] = useState(true);
  const [players, setPlayers] = useState<Set<string>>(new Set());
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  // What's filled in now, for saving it after you've left the page.
  const latest = useRef({ title, map, myPov, players, hold, serverHold, saved });
  latest.current = { title, map, myPov, players, hold, serverHold, saved };
  const shows = useRef(false);
  shows.current = !!me?.shows;

  /** Saves the details, and a hold switch that didn't go through, to the clip. Reads the
   *  latest values, so it also works from the upload that goes on after you've left. */
  const persist = async (id: string) => {
    const d = latest.current;
    await call(
      api.PATCH("/api/clips/{id}", {
        params: { path: { id } },
        body: { title: d.title, map: d.map, myPov: d.myPov, players: [...d.players] },
      }),
    );
    // The switch has normally gone through already; this catches one that failed.
    if (shows.current && d.hold !== d.serverHold) {
      const path = d.hold ? "/api/clips/{id}/hold" : "/api/clips/{id}/release";
      await call(api.POST(path, { params: { path: { id } } }));
      setServerHold(d.hold);
    }
  };

  const uploading =
    phase.kind === "checking" ||
    phase.kind === "starting" ||
    phase.kind === "uploading" ||
    phase.kind === "finishing";
  /** Not uploaded, as it's here already: there are no details to fill in. */
  const found = phase.kind === "duplicate" || phase.kind === "inTrash";
  useEffect(() => {
    onBusy(uploading);
    return () => onBusy(false);
  }, [uploading, onBusy]);

  // Leaving by an in-app link mid-upload: ask first (the dialog below). Leaving anyway
  // lets the upload go on in the background while the tab is open, and saves what's been
  // filled in to the clip (once it exists).
  const blocker = useBlocker({
    shouldBlockFn: () => true,
    disabled: !uploading,
    withResolver: true,
  });
  const left = useRef(false);
  const saveAfterLeaving = (id: string) =>
    persist(id).catch((err) => toast(`Couldn't save the details: ${errorText(err)}`, "danger"));
  const leave = () => {
    left.current = true;
    if (clipId && !saved) void saveAfterLeaving(clipId);
    blocker.proceed?.();
  };

  // Closing the tab mid-upload loses it; the browser asks first.
  useEffect(() => {
    if (!uploading) return;
    const warn = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [uploading]);

  // A local preview of the file: its first frame and length, before the server has it.
  const [preview, setPreview] = useState<string | null>(null);
  const [durationMs, setDurationMs] = useState<number | null>(null);
  useEffect(() => {
    const url = URL.createObjectURL(file);
    setPreview(url);
    return () => URL.revokeObjectURL(url);
  }, [file]);

  /** Uploads the file, once it's made sure it isn't here already, unless `check` is false
   *  (it's in your trash, and you're uploading it as new anyway). Only Cancel stops it;
   *  moving to another page lets it finish, as before. A cancelled upload's clip goes to
   *  the trash, so no half-uploaded clip is left behind. */
  const start = (check: boolean) => {
    const controller = new AbortController();
    const { signal } = controller;
    abort.current = controller;
    let id: string | null = null;
    (async () => {
      try {
        if (check) {
          const copy = await findCopy(
            file,
            (hashed) => setPhase({ kind: "checking", hashed }),
            signal,
          );
          if (copy.result === "duplicate") {
            setPhase({ kind: "duplicate", clip: copy.clip ?? null });
            if (left.current) toast(`${file.name} is here already, so it wasn't uploaded.`);
            return;
          }
          if (copy.result === "inTrash" && copy.clip) {
            setPhase({ kind: "inTrash", clip: copy.clip });
            if (left.current) {
              toast(`${file.name} is in your trash. Restore it from Trash on your profile.`);
            }
            return;
          }
        }
        setPhase({ kind: "starting" });
        const created = await call(
          api.POST("/api/clips", {
            body: {
              title: titleFromFilename(file.name),
              myPov: true,
              filename: file.name,
              bytes: file.size,
              hold: holdAtUpload.current,
            },
            signal,
          }),
        );
        id = created.clip.id;
        signal.throwIfAborted();
        setClipId(id);
        if (left.current && !latest.current.saved) void saveAfterLeaving(id);
        setServerHold(!!created.clip.heldUntil);
        const startedAt = Date.now();
        setPhase({ kind: "uploading", progress: { loaded: 0, total: file.size }, startedAt });
        await uploadToBlob(
          file,
          created.uploadUrl,
          (progress) => setPhase({ kind: "uploading", progress, startedAt }),
          signal,
        );
        setPhase({ kind: "finishing" });
        await call(api.POST("/api/clips/{id}/complete", { params: { path: { id } }, signal }));
        await queryClient.invalidateQueries({ queryKey: ["clips"] });
        setPhase({ kind: "done" });
        if (left.current) toast(`${latest.current.title} is up. Kip's getting it ready.`);
      } catch (err) {
        if (signal.aborted) {
          if (id) {
            const params = { params: { path: { id } } };
            await call(api.DELETE("/api/clips/{id}", params)).catch(() => {});
            queryClient.invalidateQueries({ queryKey: ["trash"] });
          }
          return;
        }
        setPhase({ kind: "error", message: errorText(err) });
        if (left.current) toast(`Couldn't upload ${file.name}: ${errorText(err)}`, "danger");
      }
    })();
  };

  // Start as soon as the file is picked: once per file (the component is keyed by it),
  // even when React runs the effect twice in development.
  const started = useRef(false);
  // biome-ignore lint/correctness/useExhaustiveDependencies: see above.
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    start(true);
  }, []);

  // The hold switch goes to the clip as soon as it changes (and once the clip exists).
  // One request at a time; when it lands, this runs again in case the switch moved since.
  useEffect(() => {
    if (!clipId || !me?.shows || hold === serverHold || holdSending.current) return;
    holdSending.current = true;
    const path = hold ? "/api/clips/{id}/hold" : "/api/clips/{id}/release";
    call(api.POST(path, { params: { path: { id: clipId } } }))
      .then((clip) => setServerHold(!!clip.heldUntil))
      .catch((err) =>
        toast(`Couldn't ${hold ? "save it for the show" : "post it"}: ${errorText(err)}`, "danger"),
      )
      .finally(() => {
        holdSending.current = false;
      });
  }, [api, clipId, me?.shows, hold, serverHold, toast]);

  // Details saved while uploading: open the clip once it's up.
  useEffect(() => {
    if (phase.kind === "done" && saved && clipId) {
      navigate({ to: "/clips/$clipId", params: { clipId } });
    }
  }, [phase.kind, saved, clipId, navigate]);

  const save = async (e: FormEvent) => {
    e.preventDefault();
    if (!clipId) return;
    setSaving(true);
    setSaveError(null);
    try {
      await persist(clipId);
      setSaved(true);
    } catch (err) {
      setSaveError(errorText(err));
    } finally {
      setSaving(false);
    }
  };

  const togglePlayer = (id: string) =>
    setPlayers((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <div className="grid grid-cols-1 items-start gap-6 lg:grid-cols-[560px_minmax(0,1fr)]">
      <div className="flex flex-col gap-4">
        <div className="glass squircle flex flex-col gap-4 rounded-[30px] p-4">
          <div className="squircle relative aspect-video overflow-hidden rounded-[14px] bg-black">
            {preview && (
              <video
                src={preview}
                muted
                playsInline
                preload="metadata"
                className="h-full w-full object-cover"
                onLoadedMetadata={(e) => setDurationMs(e.currentTarget.duration * 1000)}
              />
            )}
            {durationMs != null && Number.isFinite(durationMs) && (
              <span className="absolute right-2.5 bottom-2.5 rounded-md bg-[rgba(8,9,10,0.75)] px-2 py-0.5 font-mono text-[11px]">
                {formatDuration(durationMs)}
              </span>
            )}
          </div>
          <div className="flex flex-col px-1">
            <span dir="auto" className="text-[17px] font-bold [overflow-wrap:anywhere]">
              {file.name}
            </span>
            <span className="font-mono text-[11px] text-muted">
              {formatBytes(file.size)}
              {durationMs != null &&
                Number.isFinite(durationMs) &&
                ` · ${formatDuration(durationMs)}`}
            </span>
          </div>
          {/* Found here already: the panel beside it says so. */}
          {!found && (
            <div className="px-1">
              <UploadState phase={phase} total={file.size} />
            </div>
          )}
          <div className="flex items-center gap-2.5 px-1">
            {uploading ? (
              <>
                <span className="text-sm text-soft">Keep this tab open until it's up.</span>
                <Button
                  className="ml-auto"
                  onClick={() => {
                    abort.current?.abort();
                    onRestart();
                  }}
                >
                  Cancel
                </Button>
              </>
            ) : phase.kind === "error" || found ? (
              <Button className="ml-auto" onClick={onRestart}>
                Choose another file
              </Button>
            ) : (
              <span className="text-sm text-soft">
                {saved ? "Opening it…" : "Save the details to open it."}
              </span>
            )}
          </div>
        </div>
        {!found && (
          <div className="squircle flex items-center gap-3.5 rounded-[22px] bg-white/4 py-3.5 pr-4.5 pl-2.5">
            <Kip className="h-14 w-14 shrink-0" />
            <p className="text-sm leading-relaxed text-soft">
              Next, Kip gets it playing everywhere and reads the killfeed. 4Ks, aces and headshots
              get tagged on their own, usually within a minute or two.
            </p>
          </div>
        )}
      </div>

      {phase.kind === "duplicate" ? (
        <HereAlready clip={phase.clip} />
      ) : phase.kind === "inTrash" ? (
        <InYourTrash clip={phase.clip} onUploadAnyway={() => start(false)} />
      ) : (
        <form
          onSubmit={save}
          className="glass squircle flex flex-col gap-5.5 rounded-[30px] px-5 pt-6 pb-5 sm:px-7"
        >
          <label className="flex flex-col gap-2">
            <span className="text-[13px] font-semibold text-soft">Title</span>
            <input
              required
              maxLength={100}
              value={title}
              onChange={(e) => {
                setTitle(e.target.value);
                setSaved(false);
              }}
              className="squircle h-12 rounded-[14px] bg-white/6 px-4 text-[17px] font-semibold ring-1 ring-white/10 ring-inset focus:ring-accent focus:outline-none"
            />
          </label>
          <fieldset className="flex flex-col gap-2">
            <legend className="mb-2 text-[13px] font-semibold text-soft">
              Map <span className="font-medium text-muted">· optional</span>
            </legend>
            <div className="flex flex-wrap gap-1.5">
              {CS2_MAPS.map((m) => (
                <Chip
                  key={m}
                  pressed={map === m}
                  onClick={() => {
                    setMap(map === m ? "" : m);
                    setSaved(false);
                  }}
                >
                  {m}
                </Chip>
              ))}
            </div>
          </fieldset>
          {friends.length > 0 && (
            <fieldset className="flex flex-col gap-2">
              <legend className="mb-2 text-[13px] font-semibold text-soft">Who's in it</legend>
              <div className="flex flex-wrap gap-1.5">
                {friends.map((m) => {
                  const on = players.has(m.id);
                  return (
                    <button
                      key={m.id}
                      type="button"
                      aria-pressed={on}
                      onClick={() => {
                        togglePlayer(m.id);
                        setSaved(false);
                      }}
                      className={`flex h-9 items-center gap-1.5 rounded-full pr-3.5 pl-1 text-sm font-semibold ${
                        on
                          ? "bg-white/14 text-text shadow-[inset_0_0_0_1.5px_var(--color-text)]"
                          : "bg-white/6 text-soft hover:bg-white/10"
                      }`}
                    >
                      <Avatar name={m.displayName} url={m.avatarUrl} size={28} />
                      <bdi>{m.displayName}</bdi>
                    </button>
                  );
                })}
              </div>
            </fieldset>
          )}
          <div className="flex flex-col gap-2 border-t border-white/7 pt-3">
            {me?.shows && (
              <Switch
                checked={hold}
                onChange={(v) => {
                  holdTouched.current = true;
                  setHold(v);
                }}
                label="Save it for the show"
                hint={
                  hold
                    ? "Joins tonight's lineup. Nobody sees it until it plays."
                    : "Post now: everyone sees it as soon as it's ready."
                }
              />
            )}
            <Switch
              checked={myPov}
              onChange={(v) => {
                setMyPov(v);
                setSaved(false);
              }}
              label="Recorded from my point of view"
              hint="Kip only counts kills as yours when it's your screen."
            />
          </div>
          {saveError && (
            <p role="alert" className="text-sm text-danger">
              {saveError}
            </p>
          )}
          <div className="flex flex-wrap items-center gap-3 pt-1">
            <span className="text-[13px] text-muted">
              Description, tags and the rest can be added later from the clip page.
            </span>
            <Button
              type="submit"
              variant="primary"
              size="lg"
              className="ml-auto"
              disabled={!clipId || saving || phase.kind === "error"}
            >
              {saved && uploading ? "Saved ✓" : "Save details"}
            </Button>
          </div>
        </form>
      )}

      <Modal
        open={blocker.status === "blocked"}
        onClose={() => blocker.reset?.()}
        title="Leave while it uploads?"
        width="max-w-md"
      >
        <p className="text-[15px] text-soft">
          It keeps uploading in the background while this tab stays open, and what you've filled in
          is saved to it.
        </p>
        <div className="flex flex-wrap gap-2">
          <Button variant="primary" onClick={leave}>
            Leave and keep uploading
          </Button>
          <Button variant="ghost" onClick={() => blocker.reset?.()}>
            Stay
          </Button>
        </div>
      </Modal>
    </div>
  );
}

/** Where the clip with the same file is, or why it can't be opened. */
function copyOf(clip: Clip | null): string {
  if (!clip) return "Someone uploaded this exact file already. It shows up once it's ready.";
  const who = clip.isMine ? "You" : clip.uploader.displayName;
  return clip.teaser
    ? `${who} uploaded this exact file already. It's saved for the show.`
    : `${who} uploaded this exact file already, as “${clip.title}”.`;
}

/** One upload's file is here already: what to open instead. */
function HereAlready({ clip }: { clip: Clip | null }) {
  return (
    <div className="glass squircle flex flex-col items-start gap-4 rounded-[30px] px-5 pt-6 pb-5 sm:px-7">
      <Kip pose="king" className="h-24 w-24" />
      <h2 className="text-[28px] leading-tight font-extrabold">It's here already.</h2>
      <p dir="auto" className="text-[17px] text-soft">
        {copyOf(clip)} One copy is plenty, so this one wasn't uploaded.
      </p>
      {clip && !clip.teaser && (
        <Link
          to="/clips/$clipId"
          params={{ clipId: clip.id }}
          className={buttonClass("primary", "lg")}
        >
          Open it
        </Link>
      )}
    </div>
  );
}

/** One upload's file is a clip of yours in the trash: restore it, or upload it as new. */
function InYourTrash({ clip, onUploadAnyway }: { clip: Clip; onUploadAnyway: () => void }) {
  const navigate = useNavigate();
  const restore = useRestoreClip(clip.id);
  return (
    <div className="glass squircle flex flex-col items-start gap-4 rounded-[30px] px-5 pt-6 pb-5 sm:px-7">
      <Kip pose="asleep" className="h-24 w-24" />
      <h2 className="text-[28px] leading-tight font-extrabold">It's in your trash.</h2>
      <p dir="auto" className="text-[17px] text-soft">
        You deleted this exact file, “{clip.title}”,{" "}
        {clip.deletedAt ? timeAgo(clip.deletedAt) : "recently"}. Restore it with its reactions and
        tags, or upload it again as a new clip.
      </p>
      {restore.error && (
        <p role="alert" className="text-sm text-danger">
          Couldn't restore it: {restore.error.message}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <Button
          variant="primary"
          size="lg"
          disabled={restore.isPending}
          onClick={() =>
            restore.mutate(undefined, {
              onSuccess: () => navigate({ to: "/clips/$clipId", params: { clipId: clip.id } }),
            })
          }
        >
          Restore it
        </Button>
        <Button size="lg" onClick={onUploadAnyway} disabled={restore.isPending}>
          Upload it as new
        </Button>
      </div>
    </div>
  );
}

type Item = {
  key: number;
  file: File;
  state: Phase | { kind: "waiting" } | ({ kind: "rejected" } & Rejection);
  /** Upload it without asking whether it's here (it's in your trash; you said upload). */
  skipCheck?: boolean;
  clipId?: string;
  serverHold?: boolean;
  holdSending?: boolean;
  abort?: AbortController;
};

const isActive = (item: Item) =>
  item.state.kind === "waiting" ||
  item.state.kind === "checking" ||
  item.state.kind === "starting" ||
  item.state.kind === "uploading" ||
  item.state.kind === "finishing";

// Several files at once: a queue that uploads them one after another, each as its own
// clip titled from its filename. Map, players and the rest are for each clip's page; only
// "Save it for the show" is here, since a posted clip can't be held afterwards. The queue
// lives in a ref, not state, so it goes on after you've left the page, like one upload.
function Batch({
  files,
  add: addRef,
}: {
  files: File[];
  add: MutableRefObject<((files: File[]) => void) | null>;
}) {
  const api = useApi();
  const queryClient = useQueryClient();
  const { data: me } = useMe();
  const toast = useToast();
  const findCopy = useFindCopy();
  const items = useRef<Item[]>([]);
  const [, rerender] = useReducer((n: number) => n + 1, 0);
  const nextKey = useRef(0);
  const running = useRef(false);
  const left = useRef(false);

  // The hold switch: same default as for one upload, and for every clip in the queue.
  const [hold, setHold] = useState(() => !!me?.showsForEveryone);
  const holdTouched = useRef(false);
  const holdDefault = !!me?.showsForEveryone;
  useEffect(() => {
    if (!holdTouched.current) setHold(holdDefault);
  }, [holdDefault]);
  const holdNow = useRef(hold);
  holdNow.current = hold;
  const shows = useRef(false);
  shows.current = !!me?.shows;

  const update = (item: Item, patch: Partial<Item>) => {
    Object.assign(item, patch);
    rerender();
  };
  const remove = (item: Item) => {
    items.current = items.current.filter((i) => i !== item);
    rerender();
  };

  /** Brings a clip's hold in line with the switch: one request at a time, and again when
   *  it lands in case the switch moved since. */
  const syncHold = (item: Item) => {
    const want = holdNow.current;
    if (!shows.current || !item.clipId || item.holdSending || item.serverHold === want) return;
    if (!items.current.includes(item)) return;
    item.holdSending = true;
    const path = want ? "/api/clips/{id}/hold" : "/api/clips/{id}/release";
    call(api.POST(path, { params: { path: { id: item.clipId } } }))
      .then((clip) => {
        item.holdSending = false;
        update(item, { serverHold: !!clip.heldUntil });
        syncHold(item);
      })
      .catch((err) => {
        item.holdSending = false;
        toast(
          `Couldn't ${want ? "save" : "post"} ${item.file.name}${want ? " for the show" : ""}: ${errorText(err)}`,
          "danger",
        );
      });
  };
  // biome-ignore lint/correctness/useExhaustiveDependencies: syncHold reads refs.
  useEffect(() => {
    for (const item of items.current) syncHold(item);
  }, [hold]);

  const uploadOne = async (item: Item) => {
    const { file } = item;
    const controller = new AbortController();
    const { signal } = controller;
    let id: string | null = null;
    try {
      if (!item.skipCheck) {
        update(item, { state: { kind: "checking", hashed: null }, abort: controller });
        const copy = await findCopy(
          file,
          (hashed) => update(item, { state: { kind: "checking", hashed } }),
          signal,
        );
        if (copy.result === "duplicate") {
          update(item, { state: { kind: "duplicate", clip: copy.clip ?? null } });
          return;
        }
        if (copy.result === "inTrash" && copy.clip) {
          update(item, { state: { kind: "inTrash", clip: copy.clip } });
          return;
        }
      }
      update(item, { state: { kind: "starting" }, abort: controller });
      const created = await call(
        api.POST("/api/clips", {
          body: {
            title: titleFromFilename(file.name),
            myPov: true,
            filename: file.name,
            bytes: file.size,
            hold: holdNow.current,
          },
          signal,
        }),
      );
      id = created.clip.id;
      signal.throwIfAborted();
      update(item, { clipId: id, serverHold: !!created.clip.heldUntil });
      syncHold(item);
      const startedAt = Date.now();
      update(item, {
        state: { kind: "uploading", progress: { loaded: 0, total: file.size }, startedAt },
      });
      await uploadToBlob(
        file,
        created.uploadUrl,
        (progress) => update(item, { state: { kind: "uploading", progress, startedAt } }),
        signal,
      );
      update(item, { state: { kind: "finishing" } });
      await call(api.POST("/api/clips/{id}/complete", { params: { path: { id } }, signal }));
      update(item, { state: { kind: "done" } });
      void queryClient.invalidateQueries({ queryKey: ["clips"] });
    } catch (err) {
      if (signal.aborted) {
        // Cancelled: its clip goes to the trash, so no half-uploaded clip is left behind.
        if (id) {
          await call(api.DELETE("/api/clips/{id}", { params: { path: { id } } })).catch(() => {});
          queryClient.invalidateQueries({ queryKey: ["trash"] });
        }
        return;
      }
      update(item, { state: { kind: "error", message: errorText(err) } });
      if (left.current) toast(`Couldn't upload ${file.name}: ${errorText(err)}`, "danger");
    }
  };

  /** Brings back the clip in your trash that has `item`'s file, instead of uploading it. */
  const restore = async (item: Item, clip: Clip) => {
    try {
      const restored = await call(
        api.POST("/api/clips/{id}/restore", { params: { path: { id: clip.id } } }),
      );
      update(item, { state: { kind: "restored", clip: restored }, clipId: restored.id });
      void queryClient.invalidateQueries({ queryKey: ["clips"] });
      void queryClient.invalidateQueries({ queryKey: ["trash"] });
    } catch (err) {
      toast(`Couldn't restore ${clip.title}: ${errorText(err)}`, "danger");
    }
  };

  /** Uploads what's waiting, in order, until nothing is; what's added meanwhile joins in. */
  const run = async () => {
    if (running.current) return;
    running.current = true;
    try {
      for (;;) {
        const next = items.current.find((i) => i.state.kind === "waiting");
        if (!next) break;
        await uploadOne(next);
      }
    } finally {
      running.current = false;
    }
    if (left.current) {
      const up = items.current.filter((i) => i.state.kind === "done").length;
      const copies = items.current.filter(
        (i) => i.state.kind === "duplicate" || i.state.kind === "inTrash",
      ).length;
      if (copies > 0) {
        toast(
          `${copies === 1 ? "1 file was" : `${copies} files were`} here already, so ${copies === 1 ? "it wasn't" : "they weren't"} uploaded.`,
        );
      }
      if (up > 0)
        toast(`${up === 1 ? "1 clip is" : `${up} clips are`} up. Kip's getting them ready.`);
    }
  };

  const add = (picked: File[]) => {
    for (const file of picked) {
      const no = rejection(file);
      items.current.push({
        key: nextKey.current++,
        file,
        state: no ? { kind: "rejected", ...no } : { kind: "waiting" },
      });
    }
    rerender();
    void run();
  };
  const addNow = useRef(add);
  addNow.current = add;
  // Picks and drops join the queue while it shows.
  useEffect(() => {
    addRef.current = (picked) => addNow.current(picked);
    return () => {
      addRef.current = null;
    };
  }, [addRef]);
  // The files it was opened with: queued once, even when React runs this twice.
  const started = useRef(false);
  // biome-ignore lint/correctness/useExhaustiveDependencies: see above.
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    add(files);
  }, []);

  const active = items.current.some(isActive);
  const blocker = useBlocker({
    shouldBlockFn: () => true,
    disabled: !active,
    withResolver: true,
  });
  useEffect(() => {
    if (!active) return;
    const warn = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [active]);

  // Files that are here already aren't counted: there's nothing of theirs to upload.
  const valid = items.current.filter(
    (i) => i.state.kind !== "rejected" && i.state.kind !== "duplicate",
  );
  const up = valid.filter((i) => i.state.kind === "done" || i.state.kind === "restored").length;

  return (
    <div className="grid grid-cols-1 items-start gap-6 lg:grid-cols-[minmax(0,1fr)_340px]">
      <div className="glass squircle flex flex-col gap-1 rounded-[30px] p-3 sm:p-4">
        <ul className="flex flex-col divide-y divide-white/7" aria-label="Uploads">
          {items.current.map((item) => (
            <BatchRow
              key={item.key}
              item={item}
              onCancel={() => {
                item.abort?.abort();
                remove(item);
              }}
              onRetry={() => {
                update(item, { state: { kind: "waiting" }, clipId: undefined });
                void run();
              }}
              onRemove={() => remove(item)}
              onRestore={(clip) => restore(item, clip)}
              onUploadAnyway={() => {
                update(item, { state: { kind: "waiting" }, skipCheck: true });
                void run();
              }}
            />
          ))}
        </ul>
        <div className="flex flex-wrap items-center gap-3 border-t border-white/7 px-1 pt-3">
          <span className="text-sm text-soft">
            {active
              ? `${up} of ${valid.length} up · keep this tab open until they're all up.`
              : valid.length > 0
                ? `${up} of ${valid.length} up.`
                : "Nothing to upload."}
          </span>
          <FilePicker onPick={add} variant="secondary" size="md" className="ml-auto">
            Add more
          </FilePicker>
        </div>
      </div>
      <div className="flex flex-col gap-4">
        {me?.shows && (
          <div className="glass squircle rounded-[30px] px-5 py-4">
            <Switch
              checked={hold}
              onChange={(v) => {
                holdTouched.current = true;
                setHold(v);
              }}
              label="Save them for the show"
              hint={
                hold
                  ? "They join tonight's lineup. Nobody sees them until they play."
                  : "Post now: everyone sees each one as soon as it's ready."
              }
            />
          </div>
        )}
        <div className="squircle flex items-center gap-3.5 rounded-[22px] bg-white/4 py-3.5 pr-4.5 pl-2.5">
          <Kip className="h-14 w-14 shrink-0" />
          <p className="text-sm leading-relaxed text-soft">
            Titles come from the file names. Add the map, who's in it and the rest on each clip's
            page, once it's up. Kip reads the killfeed and tags 4Ks, aces and headshots on its own.
          </p>
        </div>
      </div>

      <Modal
        open={blocker.status === "blocked"}
        onClose={() => blocker.reset?.()}
        title="Leave while they upload?"
        width="max-w-md"
      >
        <p className="text-[15px] text-soft">
          They keep uploading in the background, one after another, while this tab stays open.
        </p>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="primary"
            onClick={() => {
              left.current = true;
              blocker.proceed?.();
            }}
          >
            Leave and keep uploading
          </Button>
          <Button variant="ghost" onClick={() => blocker.reset?.()}>
            Stay
          </Button>
        </div>
      </Modal>
    </div>
  );
}

function BatchRow({
  item,
  onCancel,
  onRetry,
  onRemove,
  onRestore,
  onUploadAnyway,
}: {
  item: Item;
  onCancel: () => void;
  onRetry: () => void;
  onRemove: () => void;
  onRestore: (clip: Clip) => Promise<void>;
  onUploadAnyway: () => void;
}) {
  const [restoring, setRestoring] = useState(false);
  const { file, state } = item;
  // Its first frame, from the file itself.
  const playable = state.kind !== "rejected";
  const [preview, setPreview] = useState<string | null>(null);
  useEffect(() => {
    if (!playable) return;
    const url = URL.createObjectURL(file);
    setPreview(url);
    return () => URL.revokeObjectURL(url);
  }, [file, playable]);

  let action: ReactNode;
  const openable =
    (state.kind === "duplicate" || state.kind === "restored") && state.clip && !state.clip.teaser
      ? state.clip
      : null;
  if (openable) {
    action = (
      <Link
        to="/clips/$clipId"
        params={{ clipId: openable.id }}
        className={buttonClass("secondary", "sm")}
        aria-label={`Open the clip ${file.name} is`}
      >
        Open
      </Link>
    );
  } else if (state.kind === "inTrash") {
    action = (
      <div className="flex shrink-0 gap-1.5">
        <Button
          size="sm"
          disabled={restoring}
          onClick={() => {
            setRestoring(true);
            void onRestore(state.clip).finally(() => setRestoring(false));
          }}
          aria-label={`Restore ${file.name} from your trash`}
        >
          Restore
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={restoring}
          onClick={onUploadAnyway}
          aria-label={`Upload ${file.name} as new`}
        >
          Upload as new
        </Button>
      </div>
    );
  } else if (state.kind === "waiting" || state.kind === "rejected" || state.kind === "duplicate") {
    action = (
      <Button size="sm" variant="ghost" onClick={onRemove} aria-label={`Remove ${file.name}`}>
        Remove
      </Button>
    );
  } else if (state.kind === "error") {
    action = (
      <Button size="sm" onClick={onRetry} aria-label={`Try ${file.name} again`}>
        Try again
      </Button>
    );
  } else if (state.kind === "done" && item.clipId) {
    action = (
      <Link
        to="/clips/$clipId"
        params={{ clipId: item.clipId }}
        className={buttonClass("secondary", "sm")}
        aria-label={`Open ${file.name}`}
      >
        Open
      </Link>
    );
  } else {
    action = (
      <Button size="sm" onClick={onCancel} aria-label={`Cancel ${file.name}`}>
        Cancel
      </Button>
    );
  }

  return (
    <li className="flex items-center gap-3 px-1 py-3">
      <div className="squircle relative hidden aspect-video w-28 shrink-0 overflow-hidden rounded-[10px] bg-black sm:block">
        {state.kind === "rejected" ? (
          <Kip pose="knocked-out" className="absolute inset-0 m-auto h-12 w-12" />
        ) : (
          preview && (
            <video
              src={preview}
              muted
              playsInline
              preload="metadata"
              className="h-full w-full object-cover"
            />
          )
        )}
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex items-start gap-2">
          <div className="flex min-w-0 flex-1 flex-col">
            <span dir="auto" className="text-[15px] font-bold [overflow-wrap:anywhere]">
              {file.name}
            </span>
            <span className="font-mono text-[11px] text-muted">{formatBytes(file.size)}</span>
          </div>
          {action}
        </div>
        {state.kind === "rejected" ? (
          <p className="text-sm text-danger">
            {state.headline} {state.body}
          </p>
        ) : state.kind === "waiting" ? (
          <p className="font-mono text-[11px] text-muted">Waiting its turn…</p>
        ) : (
          <UploadState phase={state} total={file.size} label={file.name} />
        )}
      </div>
    </li>
  );
}

const errorText = (err: unknown) => (err instanceof Error ? err.message : String(err));

function UploadState({
  phase,
  total,
  label = "Upload",
}: {
  phase: Phase;
  total: number;
  label?: string;
}) {
  if (phase.kind === "error") {
    return (
      <p role="alert" className="text-sm text-danger">
        {phase.message}
      </p>
    );
  }
  if (phase.kind === "duplicate" || phase.kind === "inTrash" || phase.kind === "restored") {
    const says =
      phase.kind === "duplicate"
        ? `Not uploaded: it's here already. ${copyOf(phase.clip)}`
        : phase.kind === "inTrash"
          ? `Not uploaded: it's in your trash, as “${phase.clip.title}”.`
          : `Restored ✓ · “${phase.clip.title}” is back from your trash.`;
    return (
      <p dir="auto" className="text-sm text-soft">
        {says}
      </p>
    );
  }
  const loaded =
    phase.kind === "uploading"
      ? phase.progress.loaded
      : phase.kind === "starting" || phase.kind === "checking"
        ? 0
        : total;
  const pct = total ? (loaded / total) * 100 : 0;
  let line: string;
  let left = "";
  if (phase.kind === "checking") {
    line =
      phase.hashed == null
        ? "Checking it isn't here already…"
        : `Comparing it with a clip that looks the same… ${Math.floor(phase.hashed * 100)}%`;
  } else if (phase.kind === "starting") line = "Starting…";
  else if (phase.kind === "finishing") line = "Finishing…";
  else if (phase.kind === "done") line = `Uploaded ✓ · ${formatBytes(total)}`;
  else {
    const seconds = (Date.now() - phase.startedAt) / 1000;
    const rate = seconds > 1 ? loaded / seconds : 0;
    line = `${Math.floor(pct)}% · ${formatBytes(loaded)} of ${formatBytes(total)}${
      rate > 0 ? ` · ${formatBytes(rate)}/s` : ""
    }`;
    if (rate > 0) {
      const s = Math.ceil((total - loaded) / rate);
      left = s >= 60 ? `about ${Math.ceil(s / 60)} min left` : `about ${s} s left`;
    }
  }
  return (
    <div className="flex flex-col gap-2">
      <div
        className="relative h-2 overflow-hidden rounded-full bg-white/10"
        role="progressbar"
        aria-label={label}
        aria-valuenow={Math.round(pct)}
        aria-valuemin={0}
        aria-valuemax={100}
      >
        <div
          className={`absolute inset-y-0 left-0 rounded-full transition-[width] ${
            phase.kind === "done" ? "bg-green" : "bg-gradient-to-r from-accent to-accent-strong"
          }`}
          style={{ width: `${pct}%` }}
        />
      </div>
      <p className="flex items-center font-mono text-[11px] text-soft">
        {line}
        {left && <span className="ml-auto text-muted">{left}</span>}
      </p>
    </div>
  );
}
