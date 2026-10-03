import { getRouteApi, Link, useNavigate } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { ClipGrid } from "../clips/ClipGrid";
import { type ClipFilters, EMOJIS, useMembers } from "../clips/hooks";
import { CS2_MAPS } from "../lib/maps";
import { useTitle } from "../lib/useTitle";
import { Avatar } from "../ui/Avatar";
import { buttonClass } from "../ui/Button";
import { Chip } from "../ui/Chip";

const route = getRouteApi("/");

/** Archive search params, validated for the `/` route. */
export function archiveSearch(search: Record<string, unknown>): ClipFilters {
  const str = (k: string) =>
    typeof search[k] === "string" && search[k] ? (search[k] as string) : undefined;
  const reaction = str("reaction");
  return {
    sort: search.sort === "top" ? "top" : undefined,
    map: str("map"),
    tag: str("tag"),
    player: str("player"),
    uploader: str("uploader"),
    q: str("q"),
    reaction: EMOJIS.some((e) => e === reaction) ? reaction : undefined,
  };
}

/** Everything "All" clears (the sort isn't a filter). */
const NO_FILTERS = {
  tag: undefined,
  reaction: undefined,
  q: undefined,
  uploader: undefined,
  player: undefined,
  map: undefined,
} satisfies Partial<ClipFilters>;

// "4K and aces": the killfeed's auto tags.
const BIG_ROUNDS = "4k,ace";

// The emoji filters, named as on the canvas (4.1).
const REACTION_FILTERS = [
  ["🔥", "Fire"],
  ["😂", "Funny"],
  ["💀", "Dead"],
  ["🐐", "GOAT"],
  ["😮", "Wow"],
] as const;

// The archive (canvas 4.1, lower half): every clip, newest first, with search and filters
// kept in the URL. Past shows join the top of this page in S7.
export function Archive() {
  const filters = route.useSearch();
  const navigate = useNavigate({ from: "/" });
  // Chips, the sort and the map are each a step Back undoes. A search is one step too:
  // once it's in the URL, more typing replaces it, so Back doesn't go word by word.
  const set = (patch: Partial<ClipFilters>, replace = false) =>
    navigate({ search: { ...filters, ...patch }, replace });
  const members = useMembers();
  useTitle("Archive");

  // The search box updates the URL once typing pauses. When the URL changes some other
  // way (the Archive link, All, Back), the box follows it: `pushed` is what it last wrote,
  // so its own writes don't bounce back while you're still typing.
  const [query, setQuery] = useState(filters.q ?? "");
  const pushed = useRef(filters.q);
  useEffect(() => {
    if (filters.q === pushed.current) return;
    pushed.current = filters.q;
    setQuery(filters.q ?? "");
  }, [filters.q]);
  useEffect(() => {
    const q = query.trim() || undefined;
    if (q === filters.q) return;
    const t = setTimeout(() => {
      pushed.current = q;
      set({ q }, filters.q !== undefined);
    }, 300);
    return () => clearTimeout(t);
  });

  const filtered = Object.keys(NO_FILTERS).some((k) => filters[k as keyof typeof NO_FILTERS]);
  // Filters that arrive from links elsewhere (a tag, a player, an uploader).
  const linked = (
    [
      ["tag", filters.tag && filters.tag !== BIG_ROUNDS && `#${filters.tag}`],
      ["player", filters.player && `with @${filters.player}`],
      ["uploader", filters.uploader && `by @${filters.uploader}`],
    ] as const
  ).filter(([, label]) => label);
  // A map from a link that isn't in the list (maps are free text) still shows.
  const maps =
    filters.map && !CS2_MAPS.includes(filters.map) ? [...CS2_MAPS, filters.map] : CS2_MAPS;

  return (
    <section className="flex flex-col gap-5">
      <div className="flex flex-wrap items-end gap-4">
        <div className="flex flex-col gap-1.5">
          <span className="font-mono text-xs text-muted">
            {filters.sort === "top"
              ? "Every clip, most reactions first"
              : "Every clip, newest first"}
          </span>
          <h1 className="text-[40px] leading-[0.95] font-extrabold tracking-tight">Archive</h1>
        </div>
        <div className="ml-auto flex w-full gap-2 sm:w-auto">
          <label className="glass relative flex h-11 shrink-0 items-center rounded-full">
            <span className="sr-only">Map</span>
            <select
              value={filters.map ?? ""}
              onChange={(e) => set({ map: e.target.value || undefined })}
              className="h-full appearance-none rounded-full bg-transparent pr-9 pl-4 text-[15px] font-semibold text-text [color-scheme:dark] focus:outline-none focus-visible:outline-2 focus-visible:outline-accent"
            >
              <option value="">All maps</option>
              {maps.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
            <svg
              viewBox="0 0 24 24"
              className="pointer-events-none absolute right-3.5 h-4 w-4 text-muted"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.4"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
            >
              <path d="M6 9l6 6 6-6" />
            </svg>
          </label>
          <label className="glass flex h-11 min-w-0 flex-1 items-center gap-2.5 rounded-full px-4 focus-within:outline-2 focus-within:outline-offset-2 focus-within:outline-accent sm:w-[300px] sm:flex-none">
            <svg
              viewBox="0 0 24 24"
              className="h-[18px] w-[18px] shrink-0 text-muted"
              fill="none"
              stroke="currentColor"
              strokeWidth="2.2"
              strokeLinecap="round"
              aria-hidden="true"
            >
              <circle cx="11" cy="11" r="7" />
              <path d="M20 20l-4-4" />
            </svg>
            <span className="sr-only">Search clips</span>
            <input
              type="search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search clips"
              className="h-full min-w-0 flex-1 bg-transparent text-[15px] text-text placeholder:text-muted focus:outline-none"
            />
          </label>
        </div>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        {(["new", "top"] as const).map((sort) => (
          <Chip
            key={sort}
            pressed={(filters.sort ?? "new") === sort}
            onClick={() => set({ sort: sort === "new" ? undefined : sort })}
          >
            {sort === "new" ? "New" : "Top"}
          </Chip>
        ))}
        <span aria-hidden="true" className="mx-1.5 h-6 w-px bg-white/12" />
        <Chip pressed={!filtered} onClick={() => set(NO_FILTERS)}>
          All
        </Chip>
        <Chip
          pressed={filters.tag === BIG_ROUNDS}
          onClick={() =>
            set({
              tag: filters.tag === BIG_ROUNDS ? undefined : BIG_ROUNDS,
              reaction: undefined,
            })
          }
        >
          4K and aces
        </Chip>
        <span aria-hidden="true" className="mx-1.5 h-6 w-px bg-white/12" />
        {REACTION_FILTERS.map(([emoji, label]) => (
          <Chip
            key={emoji}
            pressed={filters.reaction === emoji}
            onClick={() =>
              set({
                reaction: filters.reaction === emoji ? undefined : emoji,
                tag: filters.tag === BIG_ROUNDS ? undefined : filters.tag,
              })
            }
          >
            <span className="text-base">{emoji}</span>
            {label}
          </Chip>
        ))}
        {members.data && members.data.length > 0 && (
          <div className="flex items-center gap-2 lg:ml-auto">
            <span className="text-sm text-muted">Who</span>
            <div className="flex gap-1">
              {members.data.map((m) => {
                const on = filters.uploader === m.handle;
                return (
                  <button
                    key={m.id}
                    type="button"
                    aria-pressed={on}
                    title={`Clips by ${m.displayName}`}
                    onClick={() => set({ uploader: on ? undefined : m.handle })}
                    className={`rounded-full ring-2 transition ${
                      on ? "ring-accent" : "ring-transparent hover:ring-white/40"
                    }`}
                  >
                    <Avatar name={m.displayName} url={m.avatarUrl} size={30} />
                  </button>
                );
              })}
            </div>
          </div>
        )}
      </div>

      {linked.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {linked.map(([key, label]) => (
            <Chip
              key={key}
              pressed
              onClick={() => set({ [key]: undefined })}
              aria-label={`Remove filter ${label}`}
            >
              {label} ✕
            </Chip>
          ))}
        </div>
      )}

      <ClipGrid
        filters={filters}
        empty={
          filtered ? (
            <p>No clips match. Clear a filter or two.</p>
          ) : (
            <>
              <p>No clips yet. Be the first.</p>
              <Link to="/upload" className={buttonClass("primary", "lg")}>
                Upload a clip
              </Link>
            </>
          )
        }
      />
    </section>
  );
}
