import { Link } from "@tanstack/react-router";
import { Kip } from "../kip/Kip";
import { useTitle } from "../lib/useTitle";
import { buttonClass } from "../ui/Button";

// 404 (canvas 5.3): an unknown path, or a clip that doesn't exist or was deleted.
export function NotFound() {
  useTitle("Nothing here");
  return (
    <div className="flex flex-col items-center gap-5 py-16 text-center sm:py-24">
      <Kip pose="knocked-out" className="h-40 w-40 sm:h-48 sm:w-48" />
      <p className="font-mono text-sm text-muted">404</p>
      <h1 className="text-5xl font-bold tracking-tight text-balance sm:text-6xl">Nothing here.</h1>
      <p className="max-w-md text-lg text-soft text-balance">
        This clip or page doesn't exist. It may have been deleted, or the link is off by a letter.
      </p>
      <Link to="/" className={buttonClass("primary", "lg")}>
        Back to the archive
      </Link>
    </div>
  );
}
