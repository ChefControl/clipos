// Kip, the show's chicken mascot. One SVG per pose (docs/PLAN.md, redesign; the design
// canvas's "Kip: poses and where they show up").
import asleep from "./asleep.svg";
import bananaSlip from "./banana-slip.svg";
import bouncer from "./bouncer.svg";
import cheer from "./cheer.svg";
import idle from "./idle.svg";
import king from "./king.svg";
import knockedOut from "./knocked-out.svg";

const POSES = {
  // The logo, sign-in and waiting.
  idle,
  // A clip is uploaded or processing.
  cheer,
  // Empty lobby and archive: nothing new.
  asleep,
  // Clip of the night.
  king,
  // Fail of the night.
  "banana-slip": bananaSlip,
  // Not on the list.
  bouncer,
  // 404 and failures.
  "knocked-out": knockedOut,
} as const;

export type KipPose = keyof typeof POSES;

export function Kip({
  pose = "idle",
  size,
  className,
  label,
}: {
  pose?: KipPose;
  size?: number;
  className?: string;
  // Decorative unless given a label.
  label?: string;
}) {
  return (
    <img
      src={POSES[pose]}
      alt={label ?? ""}
      aria-hidden={label ? undefined : true}
      width={size}
      height={size}
      draggable={false}
      className={`drop-shadow-[0_1px_3px_rgba(0,0,0,0.6)] ${className ?? ""}`}
    />
  );
}

// The app's logo is Kip.
export function Logo({ className }: { className?: string }) {
  return <Kip pose="idle" className={className} />;
}
