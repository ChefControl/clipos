// Geometry checks for a rendered page on a phone viewport. Each returns readable
// problems instead of throwing, so one run reports everything that's wrong.
import type { Page } from "@playwright/test";

export async function layoutProblems(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const problems: string[] = [];
    const vw = window.innerWidth;
    const describe = (el: Element) => {
      const text = (el.textContent ?? "").trim().replace(/\s+/g, " ").slice(0, 40);
      const label = el.getAttribute("aria-label");
      return `<${el.tagName.toLowerCase()}> "${label ?? text}"`;
    };
    const visible = (el: Element) => {
      const r = el.getBoundingClientRect();
      const style = getComputedStyle(el);
      // sr-only elements are 1px and clipped on purpose.
      return (
        r.width > 1 && r.height > 1 && style.visibility !== "hidden" && style.display !== "none"
      );
    };
    const clipsX = (el: Element) => {
      // Truncated text (overflow hidden + ellipsis) is clipped on purpose.
      for (let e: Element | null = el; e; e = e.parentElement) {
        const o = getComputedStyle(e).overflowX;
        if (o === "hidden" || o === "clip") return true;
      }
      return false;
    };

    // 1. The page itself must not scroll sideways.
    const width = document.documentElement.scrollWidth;
    if (width > vw + 1) problems.push(`page scrolls sideways: ${width}px wide on a ${vw}px screen`);

    // 2. Nothing visible may stick out of the screen or out of its own box.
    for (const el of document.querySelectorAll("body *")) {
      if (!visible(el) || el.closest("video, svg, datalist")) continue;
      const r = el.getBoundingClientRect();
      if ((r.right > vw + 1 || r.left < -1) && !clipsX(el)) {
        problems.push(
          `off screen: ${describe(el)} spans ${Math.round(r.left)}–${Math.round(r.right)}px`,
        );
      }
      const isText = /^(H1|H2|H3|P|SPAN|A|BUTTON|DD|DT|LABEL)$/.test(el.tagName);
      if (isText && el.scrollWidth > el.clientWidth + 1 && el.clientWidth > 0 && !clipsX(el)) {
        const display = getComputedStyle(el).display;
        if (display !== "inline") {
          problems.push(
            `text overflows its box: ${describe(el)} (${el.scrollWidth} > ${el.clientWidth}px)`,
          );
        }
      }
    }

    // 3. Controls must not overlap, and must be big enough to tap (WCAG 2.5.8: 24px).
    // With a modal dialog open, the page behind it is inert: check only the dialog.
    const scope = document.querySelector("dialog[open]") ?? document;
    const controls = [
      ...scope.querySelectorAll("a[href], button, input:not([type=hidden]), select, textarea"),
    ].filter((el) => visible(el) && !el.closest("video"));
    const boxes = controls.map((el) => ({ el, r: el.getBoundingClientRect() }));
    for (const { el, r } of boxes) {
      const inlineLink = el.tagName === "A" && getComputedStyle(el).display === "inline";
      // A checkbox inside its label is tapped through the (bigger) label.
      const label = el.matches("input[type=checkbox], input[type=radio]")
        ? el.closest("label")
        : null;
      const target = label ? label.getBoundingClientRect() : r;
      if (!inlineLink && (target.height < 24 || target.width < 24)) {
        problems.push(
          `too small to tap: ${describe(el)} is ${Math.round(r.width)}×${Math.round(r.height)}px`,
        );
      }
    }
    for (let i = 0; i < boxes.length; i++) {
      for (let j = i + 1; j < boxes.length; j++) {
        const a = boxes[i];
        const b = boxes[j];
        if (!a || !b || a.el.contains(b.el) || b.el.contains(a.el)) continue;
        const w = Math.min(a.r.right, b.r.right) - Math.max(a.r.left, b.r.left);
        const h = Math.min(a.r.bottom, b.r.bottom) - Math.max(a.r.top, b.r.top);
        if (w > 2 && h > 2) problems.push(`overlap: ${describe(a.el)} and ${describe(b.el)}`);
      }
    }
    return [...new Set(problems)];
  });
}
