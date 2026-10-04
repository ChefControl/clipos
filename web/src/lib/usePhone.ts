import { useSyncExternalStore } from "react";

/** A phone, held either way: narrow, or a touch screen too short for the show. Tablets
 *  and laptops with touch screens count as PCs. */
const PHONE = "(max-width: 767px), (pointer: coarse) and (max-height: 500px)";

function subscribe(change: () => void) {
  const query = window.matchMedia(PHONE);
  query.addEventListener("change", change);
  return () => query.removeEventListener("change", change);
}

/** Whether this is a phone; follows rotation and resizing. */
export function usePhone(): boolean {
  return useSyncExternalStore(subscribe, () => window.matchMedia(PHONE).matches);
}
