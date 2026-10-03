import type { ReactNode } from "react";
import { Logo } from "./Kip";

// Three clip cards face down, fanned like a hand: the show's "no spoilers" card back
// (amber edge, Kip in the middle). `center` replaces the middle card's small Kip, e.g.
// with a big one standing on it. Decorative.
export function CardFan({
  width = 200,
  spread = 130,
  center,
}: {
  width?: number;
  spread?: number;
  center?: ReactNode;
}) {
  const cards = [-13, 0, 13];
  return (
    <div aria-hidden="true" className="relative" style={{ height: width * 1.4 + 60 }}>
      {cards.map((rot, i) => (
        <div
          key={rot}
          className="squircle absolute bottom-0 left-1/2 grid place-items-center"
          style={{
            width,
            aspectRatio: "5 / 7",
            translate: `calc(-50% + ${(i - 1) * spread}px) 0`,
            rotate: `${rot}deg`,
            transformOrigin: "50% 140%",
            zIndex: i + 1,
            borderRadius: Math.round(width * 0.12),
            background: "radial-gradient(circle at 50% 45%, #2b2418, #141210 70%)",
            boxShadow: `inset 0 0 0 ${Math.max(2, Math.round(width / 60))}px rgba(245,165,36,.55), inset 0 0 0 ${Math.round(width / 16)}px #141210, inset 0 0 0 ${Math.round(width / 16) + 1}px rgba(245,165,36,.25), 0 24px 40px -16px rgba(0,0,0,.9)`,
          }}
        >
          {i === 1 && center ? center : <Logo className="h-1/4 w-1/4 opacity-90" />}
        </div>
      ))}
    </div>
  );
}
