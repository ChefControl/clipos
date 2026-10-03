import { useState } from "react";
import { initials } from "../lib/initials";

// People are circles (things are squircles). `host` adds the amber ring the show uses for
// whoever is hosting; `ring` separates overlapping avatars in a stack. A picture that
// doesn't load (an expired Google link) falls back to the initials.
export function Avatar({
  name,
  url,
  size = 32,
  host = false,
  className = "",
}: {
  name: string;
  url?: string | null;
  size?: number;
  host?: boolean;
  className?: string;
}) {
  const style = { width: size, height: size, fontSize: Math.round(size * 0.4) };
  const ring = host ? "ring-2 ring-accent ring-offset-2 ring-offset-bg" : "";
  const [broken, setBroken] = useState<string | null>(null);
  if (url && url !== broken) {
    return (
      <img
        src={url}
        alt=""
        style={style}
        referrerPolicy="no-referrer"
        onError={() => setBroken(url)}
        className={`shrink-0 rounded-full bg-surface-2 object-cover ${ring} ${className}`}
      />
    );
  }
  return (
    <span
      style={style}
      className={`grid shrink-0 place-items-center rounded-full bg-surface-2 font-bold text-text ${ring} ${className}`}
      aria-hidden="true"
    >
      {initials(name)}
    </span>
  );
}
