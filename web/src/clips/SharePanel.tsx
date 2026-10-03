import { useState } from "react";
import { Button } from "../ui/Button";
import { type Clip, useShareClip } from "./hooks";

/** The Share dialog's body (canvas 3.5): turn the public link on or off and copy it.
 *  Shown to the uploader and admins. */
export function SharePanel({ clip }: { clip: Clip }) {
  const { share, unshare } = useShareClip(clip.id);
  const [copied, setCopied] = useState(false);
  const [confirmStop, setConfirmStop] = useState(false);
  const busy = share.isPending || unshare.isPending;
  const error = share.error ?? unshare.error;

  return (
    <div className="flex flex-col gap-4">
      <p className="text-[15px] text-soft">
        Anyone with the link can watch this clip, e.g. in Discord or WhatsApp. They don't need an
        invite.
      </p>
      {clip.shareUrl ? (
        <>
          <div className="flex gap-2">
            <input
              readOnly
              value={clip.shareUrl}
              onFocus={(e) => e.target.select()}
              aria-label="Share link"
              className="squircle h-11 min-w-0 flex-1 rounded-[14px] bg-white/6 px-4 font-mono text-[13px] ring-1 ring-white/10 ring-inset focus:ring-2 focus:ring-accent focus:outline-none"
            />
            <Button
              variant="primary"
              onClick={async () => {
                await navigator.clipboard.writeText(clip.shareUrl ?? "");
                setCopied(true);
                setTimeout(() => setCopied(false), 2000);
              }}
            >
              {copied ? "Copied ✓" : "Copy"}
            </Button>
          </div>
          {confirmStop ? (
            <div className="flex flex-wrap items-center gap-2 text-sm">
              <span className="text-soft">The link stops working everywhere it was posted.</span>
              <Button
                size="sm"
                variant="danger"
                disabled={busy}
                // A link made again later starts with the plain Stop sharing button.
                onClick={() =>
                  unshare.mutate(undefined, { onSuccess: () => setConfirmStop(false) })
                }
              >
                Stop sharing
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setConfirmStop(false)}>
                Keep it
              </Button>
            </div>
          ) : (
            <Button
              variant="ghost"
              className="self-start text-danger hover:text-danger"
              onClick={() => setConfirmStop(true)}
            >
              Stop sharing
            </Button>
          )}
        </>
      ) : (
        <Button
          variant="primary"
          className="self-start"
          disabled={busy}
          onClick={() => share.mutate()}
        >
          Create share link
        </Button>
      )}
      {error && (
        <p role="alert" className="text-sm text-danger">
          {error.message}
        </p>
      )}
    </div>
  );
}
