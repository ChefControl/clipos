import type { HTMLAttributes } from "react";

// A frosted squircle panel, the redesign's surface for grouped content. `as` picks the
// element so sections and articles keep their meaning; `padding` replaces the default
// (a second padding class in `className` wouldn't reliably win).
export function Panel({
  as: Tag = "div",
  padding = "p-5",
  className = "",
  ...rest
}: HTMLAttributes<HTMLElement> & {
  as?: "div" | "section" | "article" | "aside";
  padding?: string;
}) {
  return <Tag className={`glass squircle rounded-[26px] ${padding} ${className}`} {...rest} />;
}
