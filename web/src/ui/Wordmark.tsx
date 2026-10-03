import { Logo } from "../kip/Kip";

// Kip and "clipos", linking home. A plain <a> so it works outside the router too (sign-in,
// privacy and share pages render without it).
export function Wordmark({ size = "md" }: { size?: "md" | "lg" }) {
  return (
    <a
      href="/"
      aria-label="clipos home"
      className="flex shrink-0 items-center gap-2 text-text hover:text-white"
    >
      <Logo className={size === "lg" ? "h-11 w-11" : "h-9 w-9 sm:h-10 sm:w-10"} />
      <span
        className={`text-shadow leading-none font-bold tracking-tight ${
          size === "lg" ? "text-[34px] font-extrabold" : "text-2xl"
        }`}
      >
        clipos
      </span>
    </a>
  );
}

// The top of the pages that sit outside the app: sign-in, privacy, share links.
export function PublicHeader({ children }: { children?: React.ReactNode }) {
  return (
    <header className="flex h-16 items-center gap-3 px-4 sm:h-[76px] sm:px-10">
      <Wordmark />
      {children}
    </header>
  );
}
