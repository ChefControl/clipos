import { useNavigate } from "@tanstack/react-router";
import type { Member } from "../clips/hooks";
import { formatDuration } from "../lib/format";
import { Avatar } from "../ui/Avatar";
import { Button } from "../ui/Button";

/** How long the host has to be gone before anyone can take over (the hub's rule). */
export const TAKEOVER_AFTER_MS = 60_000;

// The host dropped out (canvas 2.8, with decision 33's rule): the clip on plays to its end
// and the show holds there. A minute after they went, anyone can take over as host, and
// everyone follows their player from then on. Until the clip ends it's only a note.
export function HostAway({
  host,
  awayMs,
  clipDone,
  onTakeOver,
}: {
  host: Member;
  /** How long the host has been gone. */
  awayMs: number;
  /** Nothing's playing any more: the show is holding. */
  clipDone: boolean;
  onTakeOver: () => void;
}) {
  const navigate = useNavigate();
  const waitMs = Math.max(0, TAKEOVER_AFTER_MS - awayMs);
  if (!clipDone) {
    return (
      <span
        role="status"
        className="glass absolute top-5 left-1/2 z-10 flex -translate-x-1/2 items-center gap-2.5 rounded-full py-2 pr-4 pl-2.5 font-semibold whitespace-nowrap"
      >
        <Avatar name={host.displayName} url={host.avatarUrl} size={26} host />
        <bdi>{host.displayName}</bdi> dropped out. The clip plays to its end.
      </span>
    );
  }
  return (
    <div
      role="alertdialog"
      aria-label={`${host.displayName} dropped out`}
      className="absolute inset-0 z-10 grid place-items-center bg-[rgba(8,9,10,0.72)] p-6 backdrop-blur-sm"
    >
      <div className="flex max-w-[520px] flex-col items-center gap-3.5 text-center">
        <span className="rounded-full opacity-80 outline-3 outline-offset-4 outline-accent/70 outline-dashed">
          <Avatar name={host.displayName} url={host.avatarUrl} size={84} />
        </span>
        <span className="text-[34px] leading-tight font-extrabold">
          <bdi>{host.displayName}</bdi> dropped out
        </span>
        <span className="text-soft">
          The show holds here until <bdi>{host.displayName}</bdi> is back. Anyone can take over:
          everyone then follows your player.
        </span>
        <div className="mt-1 flex flex-wrap justify-center gap-2">
          <Button
            variant="primary"
            size="lg"
            disabled={waitMs > 0}
            onClick={onTakeOver}
            className="h-[52px] font-extrabold"
          >
            {waitMs > 0 ? `Take over in ${formatDuration(waitMs)}` : "Take over as host"}
          </Button>
          <Button size="lg" className="h-[52px]" onClick={() => navigate({ to: "/tonight" })}>
            Leave the show
          </Button>
        </div>
      </div>
    </div>
  );
}
