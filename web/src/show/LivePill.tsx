import { Link } from "@tanstack/react-router";
import { useCurrentShow } from "./hooks";

// "● Live · Robin's show · Join" while a show is on (the lobby says it's starting), for
// everyone the show is open to: in the top bar of every page, or as a strip under it on
// phones, where the bar has no room. Not on the lobby page, which says so itself, nor in
// the show.
export function LivePill({
  enabled,
  strip = false,
  className = "",
}: {
  enabled: boolean;
  /** The phone version: the width of the page. */
  strip?: boolean;
  className?: string;
}) {
  const current = useCurrentShow(enabled);
  const show = current.data;
  if (!enabled || !show) return null;
  const host = show.host.displayName;
  const lobby = show.status === "lobby";
  return (
    <Link
      to="/shows/$showId"
      params={{ showId: show.id }}
      aria-label={`Join ${host}'s show, ${lobby ? "starting soon" : "live now"}`}
      data-testid={strip ? "live-strip" : "live-pill"}
      className={`glass min-w-0 items-center gap-2 rounded-full py-1.5 pr-1.5 pl-3.5 text-sm font-semibold text-text hover:text-white ${strip ? "flex" : ""} ${className}`}
    >
      <span
        aria-hidden="true"
        className={`h-2 w-2 shrink-0 rounded-full ${lobby ? "bg-accent" : "animate-pulse bg-danger shadow-[0_0_7px_var(--color-danger)]"}`}
      />
      <span className="shrink-0">{lobby ? "Starting" : "Live"}</span>
      <span className={`min-w-0 truncate text-soft ${strip ? "" : "hidden lg:inline"}`}>
        · <bdi>{host}</bdi>'s show
      </span>
      <span className="ml-auto shrink-0 rounded-full bg-accent px-3 py-0.5 font-bold text-on-accent">
        Join
      </span>
    </Link>
  );
}
