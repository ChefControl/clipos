// The small print at the bottom of the app's pages (the lobby mockup's footer): one line,
// with the full notice on the privacy page (`COPYRIGHT_ID`).
export function Credit({ className = "" }: { className?: string }) {
  return (
    <footer
      className={`mx-auto max-w-3xl px-4 text-center text-xs leading-relaxed text-muted/80 ${className}`}
    >
      CS2 © Valve Corporation, all rights reserved. Not affiliated with Valve.{" "}
      <a href={`/privacy#${COPYRIGHT_ID}`} className="text-muted hover:text-text">
        Copyright notice
      </a>
    </footer>
  );
}

/** The privacy page's copyright section. */
export const COPYRIGHT_ID = "copyright";
