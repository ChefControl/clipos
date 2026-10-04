import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { Logo } from "../kip/Kip";
import { Avatar } from "../ui/Avatar";
import type { Show } from "./hooks";

export type Tone = "green" | "amber" | "grey" | "red";

const DOT: Record<Tone, string> = {
  green: "bg-green shadow-[0_0_7px_var(--color-green)]",
  amber: "bg-accent",
  grey: "bg-muted",
  red: "bg-danger",
};

/** "Friday night show". */
export function showTitle(show: Show): string {
  const day = new Date(show.startedAt ?? show.createdAt).toLocaleDateString("en-US", {
    weekday: "long",
  });
  return `${day} night show`;
}

// The show's own top bar (canvas 1.3, 2.2, 2.3): Kip back to clipos, the show's name and
// where it's at, who's here (the host ringed in amber), and how this screen is doing.
export function ShowHeader({
  show,
  sub,
  online,
  hostId,
  status,
  tone,
  actions,
}: {
  show: Show;
  sub: string;
  /** Who's connected right now (user ids). */
  online: string[];
  hostId: string;
  status: string;
  tone: Tone;
  actions?: ReactNode;
}) {
  const here = show.participants.filter((p) => online.includes(p.member.id));
  return (
    <header className="flex h-16 items-center gap-2.5 sm:h-[76px]">
      <Link to="/" aria-label="clipos home" className="shrink-0">
        <Logo className="h-10 w-10" />
      </Link>
      <h1 className="text-shadow truncate text-[22px] font-bold">{showTitle(show)}</h1>
      <span className="text-shadow ml-3.5 hidden font-mono text-xs text-soft sm:inline">{sub}</span>
      <div className="ml-auto flex shrink-0 items-center gap-3">
        <ul className="flex -space-x-1.5" aria-label="Who's here">
          {here.map((p) => (
            <li key={p.member.id} title={p.member.displayName}>
              <Avatar
                name={p.member.displayName}
                url={p.member.avatarUrl}
                size={32}
                host={p.member.id === hostId}
                className="ring-2 ring-bg"
              />
              <span className="sr-only">{p.member.displayName}</span>
            </li>
          ))}
        </ul>
        <span className="text-shadow flex items-center gap-2 text-sm text-soft">
          <span className={`h-2 w-2 rounded-full ${DOT[tone]}`} />
          <span data-testid="sync-status">{status}</span>
        </span>
        {actions}
      </div>
    </header>
  );
}
