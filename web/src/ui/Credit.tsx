// The small print at the bottom of the app's pages (the lobby mockup's footer).
export function Credit({ className = "" }: { className?: string }) {
  return (
    <footer
      className={`mx-auto max-w-3xl px-4 text-center text-xs leading-relaxed text-muted/80 ${className}`}
    >
      clipos is a private fan project for a group of friends. It isn't affiliated with or endorsed
      by Valve Corporation. Counter-Strike 2, CS2 and the weapon and killfeed icons are trademarks
      and artwork of Valve Corporation. Clips belong to the players who recorded them.
    </footer>
  );
}
