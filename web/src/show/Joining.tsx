import { Link } from "@tanstack/react-router";
import { CardFan } from "../kip/CardFan";
import { Kip } from "../kip/Kip";
import { formatDuration, shortDate } from "../lib/format";
import { Avatar } from "../ui/Avatar";
import { Backdrop } from "../ui/Backdrop";
import { Button, buttonClass } from "../ui/Button";
import { Panel } from "../ui/Panel";
import { Pill } from "../ui/Pill";
import { useToast } from "../ui/Toast";
import { type Show, useShowActions } from "./hooks";
import { ShowHeader } from "./ShowHeader";
import type { ShowLive } from "./useShowLive";

// Joining a show that hasn't started (canvas 1.3): who's here and who's ready, and "I'm
// ready, sound on", the click the browser wants before clips can play with sound. The
// host gets Start instead. When the host starts, the show page turns into the show.
export function Joining({ show, live, meId }: { show: Show; live: ShowLive; meId: string }) {
  const hostId = live.presence?.hostId ?? show.host.id;
  const isHost = hostId === meId;
  const online = live.presence?.online ?? [];
  const me = show.participants.find((p) => p.member.id === meId);
  const clips = show.lineup.filter((l) => !l.dropped);
  const total = clips.reduce((sum, l) => sum + (l.clip.durationMs ?? 0), 0);
  const host = show.host.displayName;
  const { start } = useShowActions(show.id);
  const toast = useToast();

  return (
    <div
      className="flex min-h-dvh flex-col px-4 pb-10 sm:px-10"
      data-testid="show"
      data-connection={live.connection}
    >
      <Backdrop image={clips[0]?.clip.posterUrl} />
      <ShowHeader
        show={show}
        sub="waiting room"
        online={online}
        hostId={hostId}
        status={live.connection === "open" ? "Not started" : "Connecting…"}
        tone="grey"
      />
      <Panel
        as="section"
        padding="p-6 sm:px-16 sm:py-12"
        className="mt-5 grid flex-1 grid-cols-1 items-center gap-12 rounded-[30px] lg:grid-cols-[minmax(0,1fr)_520px]"
      >
        <div className="flex min-w-0 flex-col gap-[22px]">
          <span className="flex items-center gap-2.5">
            <Avatar name={host} url={show.host.avatarUrl} size={40} host />
            <span className="font-mono text-xs text-accent">
              {isHost ? "Your show" : `${host} invited you`} · {shortDate(show.createdAt)}
            </span>
          </span>
          <h2 className="text-[44px] leading-[0.92] font-extrabold tracking-tight text-balance sm:text-[68px]">
            {isHost ? "Your show" : `${host}'s show`}
            <br />
            starts soon.
          </h2>
          <p className="max-w-[520px] text-lg leading-snug text-soft">
            {isHost
              ? "Start when you like: whoever's here plays the first clip with you, in sync. Latecomers drop in where you are."
              : `You're in. When ${host} presses Start, the first clip plays right here, in sync with everyone. Keep talking in Discord voice.`}
          </p>
          <div className="flex flex-wrap items-center gap-4">
            {isHost ? (
              <>
                <Button
                  variant="primary"
                  size="lg"
                  className="h-14 px-7 text-[19px] font-extrabold"
                  disabled={start.isPending || clips.length === 0}
                  onClick={() =>
                    start.mutate(undefined, {
                      onError: (err) => toast(`Couldn't start the show: ${err.message}`, "danger"),
                    })
                  }
                >
                  Start the show
                </Button>
                <Link to="/tonight" className={`${buttonClass("secondary", "lg")} h-14`}>
                  Change the lineup
                </Link>
              </>
            ) : me?.ready ? (
              <Pill tone="green" className="h-10 px-4 text-[15px]">
                Ready, sound on
              </Pill>
            ) : (
              <>
                <Button
                  variant="primary"
                  size="lg"
                  className="h-14 px-7 text-[19px] font-extrabold"
                  disabled={live.connection !== "open"}
                  onClick={() => live.client?.send({ type: "ready", ready: true })}
                >
                  <SoundIcon />
                  I'm ready, sound on
                </Button>
                <span className="max-w-[220px] text-sm leading-tight text-muted">
                  Your browser needs one click before clips can play with sound.
                </span>
              </>
            )}
          </div>
          <div className="squircle flex max-w-[560px] flex-col rounded-[22px] bg-white/4 px-[18px] pt-3.5 pb-2">
            <span className="pb-1 font-mono text-[11px] text-muted">
              Who's here · {show.participants.filter((p) => online.includes(p.member.id)).length} of{" "}
              {show.participants.length}
            </span>
            <ul data-testid="whos-here">
              {show.participants.map((p) => {
                const isThere = online.includes(p.member.id);
                const theHost = p.member.id === hostId;
                const [label, dot] = !isThere
                  ? ["Away", "bg-[#5b5a56]"]
                  : theHost || p.ready
                    ? ["Ready", "bg-green"]
                    : ["Here, sound off", "bg-muted"];
                return (
                  <li
                    key={p.member.id}
                    className={`flex items-center gap-3 py-[7px] ${isThere ? "" : "opacity-55"}`}
                  >
                    <Avatar
                      name={p.member.displayName}
                      url={p.member.avatarUrl}
                      size={34}
                      host={theHost}
                    />
                    <span className="flex min-w-0 items-center gap-2 text-[17px] font-bold">
                      <bdi className="truncate">{p.member.displayName}</bdi>
                      {theHost && <Pill tone="accent">Host</Pill>}
                      {p.member.id === meId && !theHost && <Pill>You</Pill>}
                    </span>
                    <span className="ml-auto flex shrink-0 items-center gap-2 text-[13px] text-soft">
                      <span className={`h-[7px] w-[7px] rounded-full ${dot}`} />
                      {label}
                    </span>
                  </li>
                );
              })}
            </ul>
          </div>
        </div>
        <div className="relative hidden flex-col items-center gap-6 lg:flex">
          <CardFan width={176} spread={96} />
          <span className="flex flex-col items-center gap-1 text-center">
            <span className="text-[26px] font-bold">
              {clips.length} {clips.length === 1 ? "clip" : "clips"} tonight ·{" "}
              {formatDuration(total)}
            </span>
            <span className="text-[15px] text-muted">
              No spoilers. You see them when everyone does.
            </span>
          </span>
          <Kip pose="asleep" className="absolute -right-10 -bottom-10 h-[150px] w-[150px]" />
        </div>
      </Panel>
    </div>
  );
}

function SoundIcon() {
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
      <path d="M4 9v6h4l5 4V5L8 9z" fill="currentColor" />
      <path d="M16.5 8.5a5 5 0 0 1 0 7M19 6a8.5 8.5 0 0 1 0 12" />
    </svg>
  );
}
