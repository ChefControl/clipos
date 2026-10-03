import { createContext, type ReactNode, useCallback, useContext, useRef, useState } from "react";

type Toast = { id: number; message: ReactNode; tone: "neutral" | "danger" };

const ToastContext = createContext<(message: ReactNode, tone?: Toast["tone"]) => void>(() => {});

// Short confirmations ("Link copied") and errors, bottom centre, gone after 4 s.
export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const next = useRef(0);
  const show = useCallback((message: ReactNode, tone: Toast["tone"] = "neutral") => {
    const id = next.current++;
    setToasts((t) => [...t, { id, message, tone }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 4000);
  }, []);
  return (
    <ToastContext.Provider value={show}>
      {children}
      <div
        aria-live="polite"
        className="pointer-events-none fixed inset-x-0 bottom-6 z-50 flex flex-col items-center gap-2 px-4"
      >
        {toasts.map((t) => (
          <div
            key={t.id}
            role={t.tone === "danger" ? "alert" : "status"}
            className={`glass rounded-full px-5 py-2.5 font-semibold shadow-[0_20px_40px_-20px_rgba(0,0,0,0.9)] ${
              t.tone === "danger" ? "text-danger" : "text-text"
            }`}
          >
            {t.message}
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export function useToast() {
  return useContext(ToastContext);
}
