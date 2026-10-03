import { type ReactNode, useEffect, useRef } from "react";

// A dialog over the page (Edit, Share, Delete on the clip page). Uses <dialog> so focus,
// Escape and the inert background come from the browser.
export function Modal({
  open,
  onClose,
  title,
  children,
  width = "max-w-lg",
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
  width?: string;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) d.showModal();
    if (!open && d.open) d.close();
  }, [open]);
  // Every way of closing (Escape, the ✕, the backdrop, `open` turning false) goes through
  // the dialog's own close event, so onClose fires exactly once.
  const close = () => ref.current?.close();

  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: the backdrop click mirrors Escape, which <dialog> handles natively.
    <dialog
      ref={ref}
      aria-label={title}
      onClose={onClose}
      // A click on the backdrop lands on the <dialog> itself.
      onClick={(e) => e.target === ref.current && close()}
      className={`glass squircle m-auto w-[calc(100%-2rem)] ${width} rounded-[30px] p-0 text-text backdrop:bg-black/60 backdrop:backdrop-blur-sm`}
    >
      {open && (
        <div className="flex flex-col gap-4 p-6">
          <div className="flex items-start gap-3">
            <h2 className="text-2xl font-bold leading-tight">{title}</h2>
            <button
              type="button"
              onClick={close}
              aria-label="Close"
              className="-mt-1 -mr-2 ml-auto grid h-9 w-9 place-items-center rounded-full text-soft hover:bg-white/10 hover:text-text"
            >
              <svg viewBox="0 0 24 24" className="h-5 w-5" aria-hidden="true">
                <path
                  d="M6 6l12 12M18 6L6 18"
                  stroke="currentColor"
                  strokeWidth="2.2"
                  strokeLinecap="round"
                />
              </svg>
            </button>
          </div>
          {children}
        </div>
      )}
    </dialog>
  );
}
