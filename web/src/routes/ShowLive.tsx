import { getRouteApi, Link } from "@tanstack/react-router";
import { useRef } from "react";
import { isNotFound } from "../api/errors";
import { Kip } from "../kip/Kip";
import { usePhone } from "../lib/usePhone";
import { useTitle } from "../lib/useTitle";
import { Finale } from "../show/Finale";
import { joinLink, type Show as ShowView, useShow } from "../show/hooks";
import { Joining } from "../show/Joining";
import { Reveal } from "../show/Reveal";
import { Stage } from "../show/Stage";
import { type ShowEvent, useShowLive } from "../show/useShowLive";
import { Backdrop } from "../ui/Backdrop";
import { Button, buttonClass } from "../ui/Button";
import { LoadError } from "../ui/LoadError";
import { Panel } from "../ui/Panel";
import { useToast } from "../ui/Toast";
import { NotFound } from "./NotFound";
import { useMe } from "./useMe";

const route = getRouteApi("/shows/$showId");

// A show's page, the join link: the waiting room until the host starts (1.3), then the
// show (2.2, 2.3). It takes the whole window; the app's top bar steps aside.
export function ShowLive() {
  const { showId } = route.useParams();
  // The show is PC-only until Epic 2 (decision 39): a phone gets the way to a PC, and
  // doesn't join the room.
  if (usePhone()) return <OpenOnAPc showId={showId} />;
  return <Show showId={showId} />;
}

function Show({ showId }: { showId: string }) {
  const me = useMe();
  const show = useShow(showId);
  const toast = useToast();
  useTitle(show.data && `${show.data.host.displayName}'s show`);
  // The screen on now takes the room's one-off events.
  const events = useRef<((e: ShowEvent) => void) | null>(null);
  // Refusals the connection survives ("only while the show is live") as toasts; the rate
  // limit's "slow down" quietly.
  const live = useShowLive(showId, (e) => {
    if (e.type === "error" && e.message !== "slow down") toast(e.message, "danger");
    events.current?.(e);
  });

  // No such show (or shows aren't open to you yet).
  if (isNotFound(show.error)) return <NotFound />;
  if (show.error) {
    return (
      <Centered>
        <LoadError error={show.error} onRetry={show.refetch} />
      </Centered>
    );
  }
  if (!show.data || !me.data) return null;

  const status = show.data.status;
  // Who you are in the room: the server says so when you connect.
  const meId = live.userId ?? me.data.id;
  if (status === "ended") return <Reveal show={show.data} live={live} />;
  // The room said it's over: after a finale the winners are on their way (no flash of
  // "over" before them); otherwise it ended by itself.
  if (live.over && status === "finale") return null;
  if (status === "abandoned" || live.over) return <Over show={show.data} />;
  // Closed for good: why, once (the server's reason).
  if (live.connection === "closed") {
    return (
      <Centered>
        <p role="alert" className="text-lg text-danger">
          {live.error ?? "The connection to the show closed."}
        </p>
      </Centered>
    );
  }
  if (status === "lobby") return <Joining show={show.data} live={live} meId={meId} />;
  if (status === "finale") return <Finale show={show.data} live={live} meId={meId} />;
  return <Stage show={show.data} live={live} meId={meId} events={events} />;
}

function Centered({ children }: { children: React.ReactNode }) {
  return <div className="grid min-h-dvh place-items-center p-6">{children}</div>;
}

function Over({ show }: { show: ShowView }) {
  return (
    <Centered>
      <Backdrop />
      <Panel className="flex max-w-lg flex-col items-center gap-4 p-10 text-center">
        <Kip pose="asleep" className="h-32 w-32" />
        <h1 className="text-4xl font-extrabold tracking-tight" data-testid="show-over">
          The show's over.
        </h1>
        <p className="text-soft">
          {show.status === "abandoned"
            ? "Everyone left, so it ended without a finale. Its clips come back for the next show."
            : "Thanks for watching. The clips are in the archive now."}
        </p>
        <div className="flex gap-3">
          <Link to="/tonight" className={buttonClass("secondary")}>
            Tonight
          </Link>
          <Link to="/" className={buttonClass("primary")}>
            Archive
          </Link>
        </div>
      </Panel>
    </Centered>
  );
}

/** A show link opened on a phone (decision 39): the show needs a bigger screen for now. */
function OpenOnAPc({ showId }: { showId: string }) {
  const toast = useToast();
  useTitle("Open this on a PC");
  return (
    <Centered>
      <Backdrop />
      <Panel className="flex max-w-md flex-col items-center gap-4 p-8 text-center">
        <Kip pose="bouncer" className="h-32 w-32" />
        <h1 className="text-[34px] leading-tight font-extrabold tracking-tight">
          Open this on a PC
        </h1>
        <p className="text-soft">
          The show is made for a big screen for now: open the link on your computer and keep talking
          in Discord. Phones get their own show soon.
        </p>
        <div className="flex flex-wrap justify-center gap-3">
          <Button
            variant="primary"
            onClick={() =>
              navigator.clipboard
                .writeText(joinLink(showId))
                .then(() => toast("Link copied. Paste it on your PC."))
                .catch(() => toast(`Couldn't copy it. The link: ${joinLink(showId)}`, "danger"))
            }
          >
            Copy the link
          </Button>
          <Link to="/" className={buttonClass("secondary")}>
            Back to clipos
          </Link>
        </div>
      </Panel>
    </Centered>
  );
}
