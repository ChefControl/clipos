import { useEffect } from "react";

/** The tab's title, "Archive · clipos". Until `title` is known (still loading) the last
 *  page's title stays. */
export function useTitle(title: string | null | undefined) {
  useEffect(() => {
    if (title) document.title = `${title} · clipos`;
  }, [title]);
}
