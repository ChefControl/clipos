import { Link } from "@tanstack/react-router";
import { useState } from "react";
import type { Clip } from "../clips/hooks";
import { Kip } from "../kip/Kip";
import { Avatar } from "../ui/Avatar";
import { buttonClass } from "../ui/Button";
import { useServerNow } from "./Finale";
import type { Category, Show } from "./hooks";
import { ShowHeader } from "./ShowHeader";
import type { ShowLive } from "./useShowLive";

/** How long fail of the night is on before clip of the night (2.6, then 2.7). */
export const FAIL_REVEAL_MS = 8_000;

// The winners (canvas 2.6 and 2.7), once the show has ended: fail of the night first,
// in violet with Kip on a banana, then clip of the night in gold with Kip crowned. The
// counts show now (decision 43). Opened later, it's clip of the night, with the fail a
// click away.
export function Reveal({ show, live }: { show: Show; live: ShowLive }) {
  const now = useServerNow(live);
  const [picked, setPicked] = useState<Category | null>(null);
  const clipOf = (id: string | null | undefined) =>
    show.lineup.find((l) => l.clip.id === id)?.clip ?? null;
  const fail = clipOf(show.failWinnerId);
  const clip = clipOf(show.clipWinnerId);
  const endedAt = show.endedAt ? Date.parse(show.endedAt) : 0;
  const shown: Category = picked ?? (fail && now - endedAt < FAIL_REVEAL_MS ? "fail" : "clip");
  const votes = (category: Category, id: string | undefined) => {
    const mine = show.votes.filter((v) => v.category === category);
    const of = mine.reduce((sum, v) => sum + v.votes, 0);
    const got = mine.find((v) => v.clipId === id)?.votes ?? 0;
    return `${got} of ${of} ${of === 1 ? "vote" : "votes"}`;
  };
  const isFail = shown === "fail";
  const winner = isFail ? fail : clip;

  return (
    <div
      className={`relative flex min-h-dvh flex-col px-4 pb-10 sm:px-10 ${isFail ? "bg-[#0c0b12]" : "bg-[#1a120a]"}`}
      data-testid="reveal"
    >
      <div
        aria-hidden="true"
        className={`pointer-events-none absolute inset-0 ${
          isFail
            ? "bg-[radial-gradient(ellipse_26%_80%_at_36%_-6%,rgba(200,190,255,.30),rgba(140,120,230,.10)_50%,rgba(0,0,0,0)_75%),radial-gradient(ellipse_85%_85%_at_50%_50%,rgba(0,0,0,0)_50%,rgba(0,0,0,.55)_100%)]"
            : "bg-[radial-gradient(ellipse_90%_70%_at_50%_110%,rgba(245,150,40,.30),rgba(245,150,40,0)_70%),radial-gradient(ellipse_34%_92%_at_34%_-8%,rgba(255,236,196,.45),rgba(255,200,110,.2)_42%,rgba(245,165,36,0)_74%)]"
        }`}
      />
      <ShowHeader
        show={show}
        sub={fail ? `results · ${isFail ? 1 : 2} of 2` : "winners"}
        online={live.presence?.online ?? []}
        hostId={show.host.id}
        status="Show over"
        tone="grey"
      />
      <div className="relative grid flex-1 grid-cols-1 items-center justify-center gap-10 py-6 lg:grid-cols-[minmax(0,600px)_400px] lg:gap-16">
        <div className="flex min-w-0 flex-col gap-4">
          <h1
            className={`flex items-center gap-2 font-mono text-[13px] ${isFail ? "text-[#c9bcff]" : "text-accent-strong"}`}
          >
            {isFail ? "Fail of the night" : "Clip of the night"}
          </h1>
          {winner ? (
            <WinnerCard clip={winner} fail={isFail} votes={votes(shown, winner.id)} />
          ) : (
            <p className="text-[28px] font-bold">
              {isFail ? "No fail of the night." : "Nobody voted for clip of the night."}
            </p>
          )}
          <div className="flex flex-wrap items-center gap-2.5">
            {winner && (
              <span className="frost flex h-10 items-center rounded-full px-4 text-[15px]">
                {isFail ? "On the fail shelf now" : "On the trophy shelf now"}
              </span>
            )}
            {fail && (
              <button
                type="button"
                onClick={() => setPicked(isFail ? "clip" : "fail")}
                className={buttonClass("secondary", "md")}
              >
                {isFail ? "Clip of the night" : "Fail of the night"}
              </button>
            )}
            {winner && (
              <Link
                to="/clips/$clipId"
                params={{ clipId: winner.id }}
                className={buttonClass("secondary", "md")}
              >
                Watch it again
              </Link>
            )}
            <Link to="/tonight" className={buttonClass("primary", "md")}>
              Done
            </Link>
          </div>
        </div>
        <div className="hidden flex-col items-center gap-2 lg:flex">
          <Kip
            key={shown}
            pose={isFail ? "banana-slip" : winner ? "king" : "asleep"}
            label={isFail ? "Kip slipping on a banana peel" : "Kip with a crown"}
            className="h-[360px] w-[360px]"
          />
          {isFail && !picked && (
            <span className="text-[17px] text-[#a9a4b8]">Next: clip of the night</span>
          )}
        </div>
      </div>
    </div>
  );
}

function WinnerCard({ clip, fail, votes }: { clip: Clip; fail: boolean; votes: string }) {
  return (
    <div
      className={`squircle flex flex-col gap-3.5 rounded-[34px] p-3.5 pb-5 backdrop-blur-[14px] ${
        fail
          ? "-rotate-[2.5deg] border border-[rgba(200,188,255,0.14)] bg-[rgba(32,30,46,0.66)] shadow-[0_0_0_2px_var(--color-violet),0_0_50px_rgba(164,139,255,0.28),0_30px_80px_-30px_rgba(0,0,0,0.8)]"
          : "border border-[rgba(255,230,190,0.16)] bg-[rgba(54,42,28,0.62)] shadow-[0_0_0_2px_var(--color-accent),0_0_40px_rgba(255,190,90,0.45),0_0_120px_rgba(245,165,36,0.35)]"
      }`}
      data-testid="winner"
    >
      <span
        className="squircle block aspect-video rounded-[22px] bg-surface bg-cover bg-center"
        style={{ backgroundImage: clip.posterUrl ? `url(${clip.posterUrl})` : undefined }}
      />
      <span className="flex items-center gap-3.5 px-2">
        <Avatar name={clip.uploader.displayName} url={clip.uploader.avatarUrl} size={44} />
        <span className="flex min-w-0 flex-col">
          <span className="truncate text-[30px] leading-none font-bold">
            {clip.isMine ? "Your clip" : `${clip.uploader.displayName}'s clip`}
          </span>
          <span dir="auto" className="truncate text-lg text-soft">
            {clip.title}
          </span>
        </span>
        <span
          className={`ml-auto shrink-0 rounded-full px-3.5 py-1.5 text-[15px] font-extrabold whitespace-nowrap ${
            fail
              ? "bg-violet/18 text-[#c9bcff] ring-[1.5px] ring-violet ring-inset"
              : "bg-gradient-to-b from-accent-strong to-accent text-on-accent"
          }`}
        >
          {votes}
        </span>
      </span>
    </div>
  );
}
