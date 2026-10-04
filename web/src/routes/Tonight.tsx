import { Link, useNavigate } from "@tanstack/react-router";
import { type ReactNode, useState } from "react";
import { isNotFound } from "../api/errors";
import type { Clip } from "../clips/hooks";
import { Kip } from "../kip/Kip";
import { formatDuration, shortDate } from "../lib/format";
import { useTitle } from "../lib/useTitle";
import {
  joinLink,
  moveTo,
  type PastShow,
  type Show,
  useCreateShow,
  usePastShows,
  useShow,
  useShowActions,
  useTonight,
} from "../show/hooks";
import { useShowLive } from "../show/useShowLive";
import { Avatar } from "../ui/Avatar";
import { Backdrop } from "../ui/Backdrop";
import { Button, buttonClass } from "../ui/Button";
import { LoadError } from "../ui/LoadError";
import { Panel } from "../ui/Panel";
import { useToast } from "../ui/Toast";
import { NotFound } from "./NotFound";
import { useMe } from "./useMe";

// The lobby, /tonight (canvas 2.1): tonight's clips and the way into the show. With no
// show on, anyone can host one; the host's open lobby is where the lineup gets its order
// and the join link goes out; while someone else's show is on, it's the way in. Nothing
// new since the last show is its own screen (2.1, empty).
export function Tonight() {
  const me = useMe();
  const tonight = useTonight();
  useTitle("Tonight");

  // Shows aren't open to you yet (decision 40): there's no such page.
  if (me.data?.shows === false || isNotFound(tonight.error)) return <NotFound />;
  if (tonight.error) return <LoadError error={tonight.error} onRetry={tonight.refetch} />;
  if (!tonight.data || !me.data) return null;

  const { show, clips, lastShow } = tonight.data;
  if (show?.status === "lobby" && show.host.id === me.data.id) {
    return <HostLobby initial={show} />;
  }
  if (show) return <ShowOn show={show} mine={show.host.id === me.data.id} />;
  if (clips.length === 0) return <NothingNew lastShow={lastShow ?? null} />;
  return <NextShow clips={clips} />;
}

/** No show on yet: what the next one would play, and Host. */
function NextShow({ clips }: { clips: Clip[] }) {
  const create = useCreateShow();
  return (
    <Page backdrop={clips[0]?.posterUrl}>
      <Hero
        eyebrow={tonightLabel()}
        title={newClips(clips.length)}
        body="Host tonight's show: you get the lineup to put in order and a link to drop in your Discord call. Everyone watches on their own screen, in sync with you."
        art={<PosterFan clips={clips} />}
      >
        <Button
          variant="primary"
          size="lg"
          className="h-14 px-7 text-[19px] font-extrabold"
          disabled={create.isPending}
          onClick={() => create.mutate()}
        >
          <PlayIcon />
          Host tonight's show
        </Button>
        {create.error && <LoadError error={create.error} />}
      </Hero>
      <Below>
        <Lineup clips={clips} note="since the last show" />
      </Below>
    </Page>
  );
}

/** Your show, in the lobby: put the lineup in order, send the link, Start. */
function HostLobby({ initial }: { initial: Show }) {
  const show = useShow(initial.id, initial).data ?? initial;
  const { setLineup, addClip, start } = useShowActions(show.id);
  // In the room while the lobby's open: it shows who's here, keeps the lineup current,
  // and lets the show end by itself if you leave it (15 minutes after the room empties).
  const live = useShowLive(show.id);
  const navigate = useNavigate();
  const toast = useToast();
  const failed = (what: string) => (err: Error) =>
    toast(`Couldn't ${what}: ${err.message}`, "danger");

  const waiting = show.lineup.filter((l) => !l.dropped && !l.playedAt);
  const dropped = show.lineup.filter((l) => l.dropped);
  const byId = new Map(show.lineup.map((l) => [l.clip.id, l.clip]));
  // While a change is on its way, the order it asked for.
  const order = setLineup.isPending ? setLineup.variables : waiting.map((l) => l.clip.id);
  const clips = order.flatMap((id) => byId.get(id) ?? []);
  const reorder = (ids: string[]) =>
    setLineup.mutate(ids, { onError: failed("change the lineup") });
  const online = new Set(live.presence?.online ?? []);

  return (
    <Page backdrop={clips[0]?.posterUrl}>
      <Hero
        eyebrow={`Your lobby · ${tonightLabel()}`}
        title={clips.length ? newClips(clips.length) : <>Nothing in the lineup.</>}
        body="Start the show and drop the link in your Discord call. Everyone watches on their own screen, in sync with you. Keep talking in voice."
        art={<PosterFan clips={clips} />}
        note="You host: only you play, pause and skip. Everyone else reacts, asks for replays and adds clips."
      >
        <Button
          variant="primary"
          size="lg"
          className="h-14 px-7 text-[19px] font-extrabold"
          disabled={start.isPending || clips.length === 0}
          onClick={() =>
            start.mutate(undefined, {
              onSuccess: () => navigate({ to: "/shows/$showId", params: { showId: show.id } }),
              onError: failed("start the show"),
            })
          }
        >
          <PlayIcon />
          Start the show
        </Button>
        <Button
          size="lg"
          className="frost h-14 px-6"
          onClick={() =>
            navigator.clipboard
              .writeText(joinLink(show.id))
              .then(() => toast("Join link copied. Paste it in Discord."))
              .catch(() => toast(`Couldn't copy it. The link: ${joinLink(show.id)}`, "danger"))
          }
        >
          <CopyIcon />
          Copy the join link
        </Button>
        <div className="flex w-full items-center gap-3" data-testid="in-lobby">
          <span className="text-sm text-muted">In the lobby</span>
          <div className="flex -space-x-1.5">
            {show.participants.map((p) => (
              <span
                key={p.member.id}
                title={`${p.member.displayName}${online.has(p.member.id) ? "" : " (away)"}`}
                className={online.has(p.member.id) ? "" : "opacity-50"}
              >
                <Avatar
                  name={p.member.displayName}
                  url={p.member.avatarUrl}
                  size={30}
                  host={p.member.id === show.host.id}
                  className="ring-2 ring-bg"
                />
              </span>
            ))}
          </div>
          <span className="text-sm text-soft">
            {show.participants.length === 1
              ? "Just you so far"
              : `${show.participants.length} people`}
          </span>
        </div>
      </Hero>
      <Below>
        <Lineup
          clips={clips}
          note="since the last show"
          edit={{
            move: (id, to) => reorder(moveTo(order, id, to)),
            drop: (id) => reorder(order.filter((c) => c !== id)),
          }}
        >
          {dropped.length > 0 && (
            <div className="mt-2 flex flex-col border-t border-white/8 pt-3">
              <span className="px-1 pb-1 font-mono text-xs text-muted">
                Dropped · they wait for the next show
              </span>
              {dropped.map((l) => (
                <div key={l.clip.id} className="flex items-center gap-3 px-1 py-1.5">
                  <span dir="auto" className="min-w-0 truncate font-semibold text-soft">
                    {l.clip.title}
                  </span>
                  <Button
                    size="sm"
                    className="ml-auto"
                    disabled={addClip.isPending}
                    onClick={() => addClip.mutate(l.clip.id, { onError: failed("put it back") })}
                  >
                    Put back
                  </Button>
                </div>
              ))}
            </div>
          )}
        </Lineup>
      </Below>
    </Page>
  );
}

/** A show is on: someone else's lobby, or a show already playing or voting. */
function ShowOn({ show, mine }: { show: Show; mine: boolean }) {
  const host = show.host.displayName;
  const waiting = show.lineup.filter((l) => !l.dropped).map((l) => l.clip);
  const copy = {
    lobby: [
      <>{host}'s show starts soon.</>,
      `Join the lobby and get ready. The first clip plays when ${host} presses Start.`,
    ],
    live: [
      <>{mine ? "Your" : `${host}'s`} show is on.</>,
      "Drop in at the live position. Late joiners vote on everything in the finale.",
    ],
    finale: [
      <>{mine ? "Your" : `${host}'s`} show is voting.</>,
      "Clip and fail of the night are being picked. Join to vote.",
    ],
  } as const;
  const [title, body] = copy[show.status as keyof typeof copy] ?? copy.live;
  return (
    <Page backdrop={waiting[0]?.posterUrl}>
      <Hero
        eyebrow={
          show.status === "lobby" ? (
            `${host} is hosting · ${tonightLabel()}`
          ) : (
            <span className="flex items-center gap-2">
              <span className="h-2 w-2 rounded-full bg-danger" /> Live · {tonightLabel()}
            </span>
          )
        }
        title={title}
        body={body}
        art={<PosterFan clips={waiting} />}
      >
        <Link
          to="/shows/$showId"
          params={{ showId: show.id }}
          className={`${buttonClass("primary", "lg")} h-14 px-7 text-[19px] font-extrabold`}
        >
          <PlayIcon />
          {mine ? "Back to the show" : "Join the show"}
        </Link>
        <span className="flex items-center gap-2 text-sm text-soft">
          <Avatar name={host} url={show.host.avatarUrl} size={26} host />
          <span>
            <bdi>{host}</bdi> hosts
          </span>
        </span>
      </Hero>
      <Below>
        {show.status === "lobby" ? (
          <Lineup clips={waiting} note={`${host} puts these in order`} />
        ) : (
          <div />
        )}
      </Below>
    </Page>
  );
}

/** Nothing new since the last show (2.1, empty). */
function NothingNew({ lastShow }: { lastShow: PastShow | null }) {
  const winner = lastShow?.clips.find((c) => c.id === lastShow.clipWinnerId);
  return (
    <Page backdrop={winner?.posterUrl}>
      <Hero
        eyebrow={tonightLabel()}
        title={lastShow ? <>No new clips since the last show.</> : <>No new clips yet.</>}
        body={
          lastShow?.endedAt
            ? `Clips uploaded after ${weekday(lastShow.endedAt)}'s show land here for the next one. Upload one to get it going.`
            : "Clips uploaded this week land here for the first show. Upload one to get it going."
        }
        art={
          <div className="relative grid place-items-center">
            <span
              aria-hidden="true"
              className="absolute bottom-6 h-8 w-72 rounded-[50%] bg-[radial-gradient(closest-side,rgba(0,0,0,0.5),rgba(0,0,0,0))]"
            />
            <Kip pose="asleep" label="Kip asleep" className="h-64 w-64 lg:h-80 lg:w-80" />
          </div>
        }
      >
        <Link
          to="/upload"
          className={`${buttonClass("primary", "lg")} h-14 px-7 text-[19px] font-extrabold`}
        >
          Upload a clip
        </Link>
        {lastShow && (
          <Link
            to="/shows/$showId/replay"
            params={{ showId: lastShow.id }}
            className={`${buttonClass("secondary", "lg")} frost h-14 px-6`}
          >
            Watch last show's replay
          </Link>
        )}
      </Hero>
      {lastShow && winner && (
        <Link
          to="/clips/$clipId"
          params={{ clipId: winner.id }}
          className="glass squircle flex max-w-[640px] items-center gap-3.5 rounded-[22px] p-3 pr-5 text-text hover:text-white"
        >
          <Poster clip={winner} className="w-[120px] shrink-0 rounded-xl ring-2 ring-accent" />
          <span className="flex min-w-0 flex-col gap-0.5">
            <span className="font-mono text-[10px] text-accent-strong">
              Last show · {lastShow.endedAt && showDate(lastShow.endedAt)}
            </span>
            <span dir="auto" className="truncate text-[17px] font-bold">
              Clip of the night: {winner.title}
            </span>
            <span className="text-[13px] text-muted">
              <bdi>{winner.uploader.displayName}</bdi> · {lastShow.clipWinnerVotes} of{" "}
              {lastShow.clipVoters} votes
            </span>
          </span>
        </Link>
      )}
    </Page>
  );
}

function Page({ backdrop, children }: { backdrop?: string | null; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-6">
      <Backdrop image={backdrop} />
      {children}
    </section>
  );
}

function Hero({
  eyebrow,
  title,
  body,
  art,
  note,
  children,
}: {
  eyebrow: ReactNode;
  title: ReactNode;
  body: string;
  art: ReactNode;
  note?: string;
  children: ReactNode;
}) {
  return (
    <Panel
      as="section"
      padding="p-6 sm:px-14 sm:py-12"
      className="grid grid-cols-1 items-center gap-8 rounded-[30px] lg:grid-cols-[minmax(0,1fr)_minmax(0,560px)] lg:gap-10"
    >
      <div className="flex min-w-0 flex-col gap-[18px]">
        <span className="font-mono text-xs text-accent">{eyebrow}</span>
        <h1 className="text-[44px] leading-[0.92] font-extrabold tracking-tight text-balance sm:text-[60px] xl:text-[64px]">
          {title}
        </h1>
        <p className="max-w-[540px] text-lg leading-snug text-soft">{body}</p>
        <div className="mt-1.5 flex flex-wrap items-center gap-3">{children}</div>
        {note && <span className="text-sm text-muted">{note}</span>}
      </div>
      <div className="hidden lg:block">{art}</div>
    </Panel>
  );
}

/** The lineup beside the trophy shelf and past shows. */
function Below({ children }: { children: ReactNode }) {
  return (
    <div className="grid grid-cols-1 gap-6 lg:grid-cols-[minmax(0,1fr)_420px]">
      {children}
      <div className="flex flex-col gap-6">
        <PastShows />
      </div>
    </div>
  );
}

interface LineupEdit {
  move: (id: string, to: number) => void;
  drop: (id: string) => void;
}

/** Tonight's clips in order. The host's lobby can drag them (or move them with the arrow
 *  keys on the handle) and drop them. */
function Lineup({
  clips,
  note,
  edit,
  children,
}: {
  clips: Clip[];
  note: string;
  edit?: LineupEdit;
  children?: ReactNode;
}) {
  // The order while a row is being dragged; it's sent when the drag ends.
  const [drag, setDrag] = useState<{ id: string; order: string[] } | null>(null);
  const [moved, setMoved] = useState("");
  const ids = clips.map((c) => c.id);
  const byId = new Map(clips.map((c) => [c.id, c]));
  const shown = (drag?.order ?? ids).flatMap((id) => byId.get(id) ?? []);
  const total = clips.reduce((sum, c) => sum + (c.durationMs ?? 0), 0);

  const step = (clip: Clip, by: number) => {
    const to = ids.indexOf(clip.id) + by;
    if (!edit || to < 0 || to >= ids.length) return;
    edit.move(clip.id, to);
    setMoved(`${clip.title} moved to ${to + 1} of ${ids.length}`);
  };

  return (
    <Panel as="section" padding="px-5 pt-5 pb-3" className="flex min-w-0 flex-col rounded-[30px]">
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1 px-1 pb-2.5">
        <h2 className="text-2xl font-bold">Tonight's lineup</h2>
        <span className="font-mono text-xs text-muted">
          {note} · {formatDuration(total)}
        </span>
      </div>
      {clips.length === 0 && (
        <p className="px-1 pb-3 text-soft">Every clip is dropped. Put one back to start.</p>
      )}
      <ol className="flex flex-col" data-testid="lineup">
        {shown.map((clip, i) => (
          <li
            key={clip.id}
            draggable={!!edit}
            onDragStart={(e) => {
              e.dataTransfer.effectAllowed = "move";
              e.dataTransfer.setData("text/plain", clip.id);
              setDrag({ id: clip.id, order: ids });
            }}
            onDragOver={(e) => {
              if (!drag) return;
              e.preventDefault();
              if (clip.id !== drag.id) {
                setDrag({
                  ...drag,
                  order: moveTo(drag.order, drag.id, drag.order.indexOf(clip.id)),
                });
              }
            }}
            onDrop={(e) => e.preventDefault()}
            onDragEnd={() => {
              if (drag && edit && drag.order.join() !== ids.join()) {
                edit.move(drag.id, drag.order.indexOf(drag.id));
              }
              setDrag(null);
            }}
            className={`flex items-center gap-3 rounded-2xl py-2.5 pr-2 pl-1 sm:gap-4 ${
              drag?.id === clip.id ? "bg-white/6" : ""
            } ${edit ? "cursor-grab" : ""}`}
          >
            <span className="w-[22px] shrink-0 text-center font-mono text-xs text-muted">
              {i + 1}
            </span>
            <Poster clip={clip} className="w-24 shrink-0 rounded-[14px] sm:w-[140px]" />
            <span className="flex min-w-0 flex-col gap-0.5">
              <span className="flex min-w-0 items-center gap-2.5">
                <span dir="auto" className="truncate text-lg font-bold sm:text-xl">
                  {clip.title}
                </span>
                {clip.multiKill && (
                  <span className="rounded-full bg-accent px-2 py-px text-xs font-extrabold text-[#0e0f11] uppercase">
                    {clip.multiKill}
                  </span>
                )}
              </span>
              <span className="truncate text-sm text-muted">
                <bdi>{clip.uploader.displayName}</bdi>
                {clip.map ? ` · ${clip.map}` : ""} · {shortDate(clip.createdAt)}
              </span>
            </span>
            <span className="ml-auto shrink-0 font-mono text-xs text-muted">
              {clip.durationMs != null && formatDuration(clip.durationMs)}
            </span>
            {edit && (
              <span className="flex shrink-0 gap-1">
                <button
                  type="button"
                  aria-label={`Drop ${clip.title} from tonight`}
                  title="Drop it: it waits for the next show"
                  onClick={() => edit.drop(clip.id)}
                  className="frost grid h-10 w-10 place-items-center rounded-full text-muted hover:text-text"
                >
                  <svg
                    viewBox="0 0 24 24"
                    className="h-4 w-4"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2.4"
                    strokeLinecap="round"
                    aria-hidden="true"
                  >
                    <path d="M6 6l12 12M18 6L6 18" />
                  </svg>
                </button>
                <button
                  type="button"
                  aria-label={`Move ${clip.title}`}
                  aria-keyshortcuts="ArrowUp ArrowDown"
                  title="Drag to move, or use the arrow keys"
                  onKeyDown={(e) => {
                    const by = { ArrowUp: -1, ArrowDown: 1 }[e.key];
                    if (!by) return;
                    e.preventDefault();
                    step(clip, by);
                  }}
                  className="frost grid h-10 w-10 cursor-grab place-items-center rounded-full text-muted hover:text-text"
                >
                  <svg
                    viewBox="0 0 24 24"
                    className="h-[18px] w-[18px]"
                    fill="currentColor"
                    aria-hidden="true"
                  >
                    <circle cx="9" cy="6" r="1.6" />
                    <circle cx="15" cy="6" r="1.6" />
                    <circle cx="9" cy="12" r="1.6" />
                    <circle cx="15" cy="12" r="1.6" />
                    <circle cx="9" cy="18" r="1.6" />
                    <circle cx="15" cy="18" r="1.6" />
                  </svg>
                </button>
              </span>
            )}
          </li>
        ))}
      </ol>
      <span role="status" className="sr-only">
        {moved}
      </span>
      {children}
    </Panel>
  );
}

/** The trophy shelf and the last few shows. */
function PastShows() {
  const past = usePastShows();
  const shows = past.data ?? [];
  const trophies = shows.flatMap((s) =>
    (
      [
        ["clip", s.clipWinnerId],
        ["fail", s.failWinnerId],
      ] as const
    ).flatMap(([kind, id]) => {
      const clip = s.clips.find((c) => c.id === id);
      return clip ? [{ kind, clip, show: s }] : [];
    }),
  );
  return (
    <>
      <Panel as="section" className="flex flex-col gap-3 rounded-[30px]">
        <h2 className="text-[22px] font-bold">Trophy shelf</h2>
        {trophies.length === 0 ? (
          <div className="flex items-center gap-4">
            <Kip pose="asleep" className="h-20 w-20 shrink-0" />
            <p className="text-[15px] leading-snug text-soft">
              Empty for now. Clip and fail of the night winners land here after each show.
            </p>
          </div>
        ) : (
          <ul className="flex flex-col gap-2">
            {trophies.slice(0, 4).map((t) => (
              <li key={`${t.show.id}-${t.kind}`}>
                <Link
                  to="/clips/$clipId"
                  params={{ clipId: t.clip.id }}
                  className="flex items-center gap-3 text-text hover:text-white"
                >
                  <Poster
                    clip={t.clip}
                    className={`w-20 shrink-0 rounded-xl ring-2 ${t.kind === "clip" ? "ring-accent" : "ring-violet"}`}
                  />
                  <span className="flex min-w-0 flex-col">
                    <span
                      className={`font-mono text-[10px] ${t.kind === "clip" ? "text-accent-strong" : "text-violet"}`}
                    >
                      {t.kind === "clip" ? "Clip of the night" : "Fail of the night"}
                      {t.show.endedAt && ` · ${shortDate(t.show.endedAt)}`}
                    </span>
                    <span dir="auto" className="truncate font-bold">
                      {t.clip.title}
                    </span>
                    <span className="truncate text-[13px] text-muted">
                      <bdi>{t.clip.uploader.displayName}</bdi>
                    </span>
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        )}
      </Panel>
      <Panel as="section" padding="px-[22px] py-5" className="flex flex-col gap-1.5 rounded-[30px]">
        <h2 className="text-[22px] font-bold">Past shows</h2>
        {shows.length === 0 ? (
          <span className="text-[15px] text-muted">None yet. Tonight's is the first.</span>
        ) : (
          <ul className="flex flex-col gap-1">
            {shows.slice(0, 3).map((s) => (
              <li key={s.id}>
                <Link
                  to="/shows/$showId/replay"
                  params={{ showId: s.id }}
                  className="-mx-2 flex min-h-11 items-center gap-2 rounded-xl px-2 text-[15px] text-text hover:bg-white/5 hover:text-white"
                >
                  <span className="font-semibold">
                    {s.endedAt ? showDate(s.endedAt) : "A show"}
                  </span>
                  <span className="min-w-0 truncate text-muted">
                    <bdi>{s.host.displayName}</bdi> hosted · {s.clips.length}{" "}
                    {s.clips.length === 1 ? "clip" : "clips"}
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        )}
        <Link
          to="/"
          className="frost mt-2 inline-flex h-[38px] items-center self-start rounded-full px-4 text-sm font-semibold text-accent hover:text-accent-strong"
        >
          Browse the archive
        </Link>
      </Panel>
    </>
  );
}

/** Up to three posters fanned like a hand, the first on top (2.1). Decorative. */
function PosterFan({ clips }: { clips: Clip[] }) {
  const fan = clips.slice(0, 3);
  const mid = (fan.length - 1) / 2;
  return (
    <div aria-hidden="true" className="relative h-[360px]">
      {fan.map((clip, i) => {
        const at = fan.length - 1 - i;
        return (
          <div
            key={clip.id}
            className="squircle absolute bottom-0 left-1/2 w-[200px] overflow-hidden rounded-[26px] bg-surface bg-cover bg-center shadow-[0_0_0_1px_rgba(255,255,255,0.1),0_30px_50px_-20px_rgba(0,0,0,0.9)]"
            style={{
              aspectRatio: "5 / 7",
              translate: `calc(-50% + ${(at - mid) * 110}px) 0`,
              rotate: `${(at - mid) * 14}deg`,
              transformOrigin: "50% 140%",
              zIndex: fan.length - i,
              backgroundImage: clip.posterUrl ? `url(${clip.posterUrl})` : undefined,
            }}
          >
            <span
              dir="auto"
              className="absolute inset-x-0 bottom-0 truncate bg-gradient-to-b from-transparent to-[rgba(8,9,10,0.92)] px-4 pt-10 pb-3.5 text-lg font-bold"
            >
              {clip.title}
            </span>
          </div>
        );
      })}
      <Kip className="absolute -right-3.5 -bottom-8 z-10 h-[140px] w-[140px]" />
    </div>
  );
}

function Poster({ clip, className }: { clip: Clip; className: string }) {
  return (
    <span
      className={`squircle block aspect-video bg-surface bg-cover bg-center ${className}`}
      style={{ backgroundImage: clip.posterUrl ? `url(${clip.posterUrl})` : undefined }}
    />
  );
}

function PlayIcon() {
  return (
    <svg viewBox="0 0 24 24" className="h-4 w-4" fill="currentColor" aria-hidden="true">
      <path d="M6 4v16l14-8z" />
    </svg>
  );
}

function CopyIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      className="h-[18px] w-[18px]"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <rect x="9" y="9" width="12" height="12" rx="3" />
      <path d="M5 15V6a3 3 0 0 1 3-3h9" />
    </svg>
  );
}

/** "3 new clips. Nobody's seen them yet." */
function newClips(n: number) {
  return (
    <>
      {n} new {n === 1 ? "clip" : "clips"}.<br />
      Nobody's seen {n === 1 ? "it" : "them"} yet.
    </>
  );
}

function weekday(iso: string | Date) {
  return new Date(iso).toLocaleDateString("en-US", { weekday: "long" });
}

/** "Friday night · Oct 2". */
function tonightLabel(now = new Date()) {
  return `${weekday(now)} night · ${shortDate(now.toISOString())}`;
}

/** "Friday, Oct 2". */
function showDate(iso: string) {
  return `${weekday(iso)}, ${shortDate(iso)}`;
}
