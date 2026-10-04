// Kip's kill card (S8, canvas 6.1 and the "Kip Card Motion" study), v1: the gold T card
// of a won round, 0 to 5 kills. The cards come in stacked, fan out one by one like a
// dealer spreading a hand, and the Kip card lands last, turning from the game's skull
// face into Kip; a 4K or an ace sets the card's fire alight, and the ace lands hard. Won
// and lost, T and CT, and rising out of CS2's own cards are v2.
//
// The cards are the study's SVGs, put in as they are: their layers (the cards, fire,
// flames, embers) are what the CSS in index.css animates.
import { useEffect, useRef } from "react";
import t0 from "./killcard/t-won-0.svg?raw";
import t1 from "./killcard/t-won-1.svg?raw";
import t2 from "./killcard/t-won-2.svg?raw";
import t3 from "./killcard/t-won-3.svg?raw";
import t4 from "./killcard/t-won-4.svg?raw";
import t5 from "./killcard/t-won-5.svg?raw";

const CARDS = [t0, t1, t2, t3, t4, t5];

/** When the cards start spreading, how far apart, and how long each glides. */
const SPREAD_AT_MS = 560;
const GAP_MS = 130;
const GLIDE_MS = 420;

interface Pose {
  dx: number;
  dy: number;
  da: number;
}

/** The pose that puts `card` exactly on `top`: rotate by the angle between them about the
 *  hand, then shift so their sideways offsets line up. */
function stacked(card: Element, top: Element): Pose {
  const a = (el: Element, k: "a" | "s") => Number((el as SVGElement).dataset[k] ?? 0);
  const d = ((a(top, "a") - a(card, "a")) * Math.PI) / 180;
  return {
    dx: a(top, "s") - a(card, "s") * Math.cos(d),
    dy: -a(card, "s") * Math.sin(d),
    da: a(top, "a") - a(card, "a"),
  };
}

function pose(p: Pose, k: number, lift = 0): Keyframe {
  return {
    translate: `${(p.dx * k).toFixed(2)}px ${(p.dy * k - lift).toFixed(2)}px`,
    rotate: `${(p.da * k).toFixed(2)}deg`,
  };
}

const animates = (el: Element) => typeof (el as HTMLElement).animate === "function";
const reduced = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

/** Spreads the hand: every card waits on the leftmost card's spot, then glides into
 *  place, the Kip card last. Returns when the Kip card has landed (ms). */
function deal(slot: HTMLElement): number {
  const top = slot.querySelector(".top");
  if (!top) return 0;
  const cards = [...slot.querySelectorAll(".back"), top];
  const reveal =
    cards.length === 1
      ? 520
      : SPREAD_AT_MS + Math.max(cards.length - 2, 0) * GAP_MS + GLIDE_MS * 0.8;
  const first = cards[0];
  if (!first || reduced() || !animates(first)) return reveal;
  cards.slice(1).forEach((card, k) => {
    const p = stacked(card, first);
    const start = SPREAD_AT_MS + k * GAP_MS;
    const total = start + GLIDE_MS;
    card.animate(
      [
        { offset: 0, ...pose(p, 1) },
        { offset: start / total, ...pose(p, 1) },
        { offset: (start + GLIDE_MS * 0.5) / total, ...pose(p, 0.5, 16) },
        { offset: 1, translate: "0px 0px", rotate: "0deg" },
      ],
      { duration: total, easing: "cubic-bezier(.3,.1,.25,1)", fill: "both" },
    );
  });
  // The Kip card keeps the game's skull face until it lands.
  top
    .querySelector(".cover")
    ?.animate([{ opacity: 1 }, { opacity: 1, offset: reveal / (reveal + 380) }, { opacity: 0 }], {
      duration: reveal + 380,
      easing: "ease-in",
      fill: "both",
    });
  return reveal;
}

/** The hand closes back into one stack, the skull face returns, and the card leaves. */
function outro(slot: HTMLElement) {
  const top = slot.querySelector(".top");
  const cards = [...slot.querySelectorAll(".back"), ...(top ? [top] : [])];
  const first = cards[0];
  if (first && animates(first) && !reduced()) {
    cards.forEach((card, i) => {
      for (const a of card.getAnimations()) a.cancel();
      if (i > 0) {
        card.animate([{ translate: "0px 0px", rotate: "0deg" }, pose(stacked(card, first), 1)], {
          duration: 300,
          easing: "ease-in",
          fill: "both",
        });
      }
    });
    const cover = top?.querySelector(".cover");
    if (cover) {
      for (const a of cover.getAnimations()) a.cancel();
      cover.animate([{ opacity: 0 }, { opacity: 1 }], {
        duration: 260,
        easing: "ease-out",
        fill: "both",
      });
    }
  }
  slot.classList.remove("in", "ace");
  void slot.offsetWidth;
  slot.classList.add("out");
}

/** Kip's kill card over the player, where CS2 shows its own. `leaving`: play the outro. */
export function KillCard({ kills, leaving }: { kills: number; leaving: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const n = Math.max(0, Math.min(5, Math.round(kills)));
  useEffect(() => {
    const slot = ref.current;
    if (!slot) return;
    // Our own bundled artwork, not user content.
    slot.innerHTML = CARDS[n] ?? "";
    slot.className = "kill-card";
    void slot.offsetWidth;
    slot.style.setProperty("--reveal", `${deal(slot)}ms`);
    slot.classList.add("in");
    if (n === 5) slot.classList.add("ace");
  }, [n]);
  useEffect(() => {
    if (leaving && ref.current) outro(ref.current);
  }, [leaving]);
  return <div ref={ref} className="kill-card" data-testid="kill-card" data-kills={n} />;
}
