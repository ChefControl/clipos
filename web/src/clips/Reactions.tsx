import { useState } from "react";
import { useToast } from "../ui/Toast";
import { type Clip, EMOJIS, useReact } from "./hooks";

/** Emoji buttons with counts; tap to toggle yours (amber when it's on). A tap shows at
 *  once; that button is busy (not disabled, so it keeps keyboard focus) until the server
 *  answers, and a failed one goes back with a toast. */
export function Reactions({ clip }: { clip: Clip }) {
  const react = useReact(clip.id);
  const toast = useToast();
  // Emoji → whether it's turning on, while its request is out.
  const [pending, setPending] = useState<Record<string, boolean>>({});
  const byEmoji = new Map(clip.reactions.map((r) => [r.emoji, r]));

  const toggle = (emoji: string, on: boolean) => {
    setPending((p) => ({ ...p, [emoji]: on }));
    react
      .mutateAsync({ emoji, on })
      .catch((err: Error) => toast(`Couldn't react: ${err.message}`, "danger"))
      .finally(() => setPending(({ [emoji]: _, ...rest }) => rest));
  };

  return (
    <div className="flex flex-wrap gap-2">
      {EMOJIS.map((emoji) => {
        const r = byEmoji.get(emoji);
        const busy = emoji in pending;
        const mine = pending[emoji] ?? r?.mine ?? false;
        const count = (r?.count ?? 0) + (busy && mine !== (r?.mine ?? false) ? (mine ? 1 : -1) : 0);
        return (
          <button
            key={emoji}
            type="button"
            aria-busy={busy}
            onClick={() => !busy && toggle(emoji, !mine)}
            aria-pressed={mine}
            aria-label={`${emoji} ${count}`}
            className={`flex h-10 min-w-10 items-center justify-center gap-1.5 rounded-full transition ${
              count > 0 ? "px-2.5" : ""
            } ${mine ? "bg-accent/16 shadow-[inset_0_0_0_1.5px_var(--color-accent)]" : "bg-white/7 hover:bg-white/12"}`}
          >
            <span className="text-lg leading-none">{emoji}</span>
            {count > 0 && (
              <span
                className={`text-sm font-bold tabular-nums ${mine ? "text-accent-strong" : "text-soft"}`}
              >
                {count}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}
