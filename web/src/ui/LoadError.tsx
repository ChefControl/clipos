import { isTransient } from "../api/errors";
import { Button } from "./Button";

// Something on the page couldn't load: why, and Try again when asking again could help
// (the server or the network had a moment; a 403 or 404 won't change).
export function LoadError({
  error,
  onRetry,
  className = "",
}: {
  error: Error;
  onRetry?: () => void;
  className?: string;
}) {
  return (
    <div role="alert" className={`flex flex-wrap items-center gap-3 text-danger ${className}`}>
      <span className="min-w-0">{error.message}</span>
      {onRetry && isTransient(error) && (
        <Button size="sm" onClick={() => onRetry()}>
          Try again
        </Button>
      )}
    </div>
  );
}
