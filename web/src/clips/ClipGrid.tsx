import { type ReactNode, useEffect, useRef } from "react";
import { Kip } from "../kip/Kip";
import { Button } from "../ui/Button";
import { LoadError } from "../ui/LoadError";
import { ClipCard } from "./ClipCard";
import { type ClipFilters, useClips } from "./hooks";

const GRID = "grid gap-x-6 gap-y-8 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4";

/** Infinite grid of clips matching `filters`; loads the next page near the bottom. */
export function ClipGrid({ filters, empty }: { filters: ClipFilters; empty: ReactNode }) {
  const clips = useClips(filters);
  const sentinel = useRef<HTMLDivElement>(null);
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = clips;

  useEffect(() => {
    const el = sentinel.current;
    if (!el || !hasNextPage) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting && !isFetchingNextPage) fetchNextPage();
      },
      { rootMargin: "600px" },
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);

  if (clips.error) {
    return <LoadError error={clips.error} onRetry={clips.refetch} />;
  }
  if (clips.isPending) {
    return (
      <div className={GRID}>
        {[0, 1, 2, 3].map((i) => (
          <div key={i} className="squircle aspect-video animate-pulse rounded-[18px] bg-surface" />
        ))}
      </div>
    );
  }
  // Pages can overlap (under Top, a clip whose reactions changed between two fetches can
  // come back on the next page): each clip shows once.
  const seen = new Set<string>();
  const all = clips.data.pages
    .flatMap((p) => p.clips)
    .filter((c) => !seen.has(c.id) && !!seen.add(c.id));
  if (all.length === 0) {
    return (
      <div className="flex flex-col items-center gap-3 py-12 text-center text-soft">
        <Kip pose="asleep" className="h-28 w-28" />
        {empty}
      </div>
    );
  }
  return (
    <>
      <div className={GRID}>
        {all.map((clip) => (
          <ClipCard key={clip.id} clip={clip} />
        ))}
      </div>
      <div ref={sentinel} className="h-px" />
      {hasNextPage && (
        <Button
          onClick={() => fetchNextPage()}
          disabled={isFetchingNextPage}
          className="mt-8 self-center"
        >
          {isFetchingNextPage ? "Loading…" : "Show older clips"}
        </Button>
      )}
    </>
  );
}
