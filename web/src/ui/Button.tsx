import type { ButtonHTMLAttributes } from "react";

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "violet";
export type ButtonSize = "sm" | "md" | "lg";

const VARIANTS: Record<ButtonVariant, string> = {
  // Amber is the one thing to press: Start the show, Save, Upload.
  primary:
    "bg-gradient-to-b from-accent-strong to-accent text-on-accent shadow-[0_0_24px_-6px_rgba(245,165,36,0.55)] hover:brightness-110",
  secondary: "bg-white/10 text-text hover:bg-white/15",
  ghost: "text-soft hover:bg-white/10 hover:text-text",
  danger: "bg-danger-strong text-white hover:brightness-110",
  violet: "bg-violet text-[#1b1238] hover:brightness-110",
};

const SIZES: Record<ButtonSize, string> = {
  sm: "h-8 px-3.5 text-sm gap-1.5",
  md: "h-10 px-5 text-[15px] gap-2",
  lg: "h-12 px-7 text-base gap-2.5",
};

// Every button is a pill. Use `buttonClass` to style a router <Link> the same way.
export function buttonClass(variant: ButtonVariant = "secondary", size: ButtonSize = "md") {
  return `inline-flex shrink-0 items-center justify-center rounded-full font-semibold whitespace-nowrap transition focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:pointer-events-none disabled:opacity-50 ${VARIANTS[variant]} ${SIZES[size]}`;
}

export function Button({
  variant = "secondary",
  size = "md",
  className = "",
  type = "button",
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: ButtonVariant; size?: ButtonSize }) {
  return <button type={type} className={`${buttonClass(variant, size)} ${className}`} {...rest} />;
}
