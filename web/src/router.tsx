import {
  createRootRoute,
  createRoute,
  createRouter,
  lazyRouteComponent,
} from "@tanstack/react-router";
import { Admin } from "./routes/Admin";
import { Archive, archiveSearch } from "./routes/Archive";
import { Layout } from "./routes/Layout";
import { NotFound } from "./routes/NotFound";
import { Profile } from "./routes/Profile";
import { Tonight } from "./routes/Tonight";
import { Upload } from "./routes/Upload";
import { UserPage, userSearch } from "./routes/UserPage";

const rootRoute = createRootRoute({ component: Layout, notFoundComponent: NotFound });

const routeTree = rootRoute.addChildren([
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/",
    component: Archive,
    validateSearch: archiveSearch,
  }),
  createRoute({ getParentRoute: () => rootRoute, path: "/tonight", component: Tonight }),
  createRoute({ getParentRoute: () => rootRoute, path: "/me", component: Profile }),
  createRoute({ getParentRoute: () => rootRoute, path: "/admin", component: Admin }),
  createRoute({ getParentRoute: () => rootRoute, path: "/upload", component: Upload }),
  // Its own chunk: the player (Media Chrome) is only needed here. Preloaded on hover.
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/clips/$clipId",
    component: lazyRouteComponent(() => import("./routes/ClipPage"), "ClipPage"),
  }),
  // The show's live page (S5, bare until S6). Its own chunk.
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/shows/$showId",
    component: lazyRouteComponent(() => import("./routes/ShowLive"), "ShowLive"),
  }),
  // A past show's replay, alone (S7). Its own chunk.
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/shows/$showId/replay",
    component: lazyRouteComponent(() => import("./routes/ShowReplay"), "ShowReplay"),
  }),
  createRoute({
    getParentRoute: () => rootRoute,
    path: "/u/$handle",
    component: UserPage,
    validateSearch: userSearch,
  }),
]);

export const router = createRouter({ routeTree, defaultPreload: "intent" });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
