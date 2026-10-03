import type { ButtonHTMLAttributes } from "react";

// A toggleable filter or choice: maps, emoji filters, "Yours / All". Pressed chips are
// solid; the state is exposed with aria-pressed.
export function Chip({
  pressed,
  className = "",
  type = "button",
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { pressed: boolean }) {
  return (
    <button
      type={type}
      aria-pressed={pressed}
      className={`inline-flex h-8 shrink-0 items-center gap-1.5 rounded-full px-3.5 text-sm font-semibold whitespace-nowrap transition focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent ${
        pressed ? "bg-text text-bg" : "frost text-soft hover:bg-white/15 hover:text-text"
      } ${className}`}
      {...rest}
    />
  );
}
