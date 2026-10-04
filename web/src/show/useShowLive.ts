import { useAuth0 } from "@auth0/auth0-react";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import type { Show } from "./hooks";
import { type Connection, LiveClient, liveUrl, type Presence, type ServerMsg } from "./live";
import type { LiveState } from "./sync";

export interface ShowLive {
  client: LiveClient | null;
  connection: Connection;
  state: LiveState | null;
  presence: Presence | null;
  userId: string | null;
  /** The show has ended (the server said so); the connection is closed for good. */
  over: boolean;
  /** Why the server closed the connection for good ("not allowed", …), if it did. Errors
   *  it survives ("slow down") are events. */
  error: string | null;
}

/** One-off things that happen in the room: reactions, replay requests, errors, the end. */
export type ShowEvent = Exclude<
  ServerMsg,
  { type: "welcome" | "state" | "pong" | "presence" | "showChanged" }
>;

const EMPTY: ShowLive = {
  client: null,
  connection: "connecting",
  state: null,
  presence: null,
  userId: null,
  over: false,
  error: null,
};

/** The show's live room: its playback state, who's there, and its events. REST data
 *  (`["show", id]`) is refetched whenever the server says the show changed. Events go to
 *  `onEvent` one by one as they arrive, so a burst of reactions isn't squashed into one
 *  render. */
export function useShowLive(showId: string, onEvent?: (event: ShowEvent) => void): ShowLive {
  const { getAccessTokenSilently } = useAuth0();
  const queryClient = useQueryClient();
  const [live, setLive] = useState<ShowLive>(EMPTY);
  const token = useRef(getAccessTokenSilently);
  token.current = getAccessTokenSilently;
  const handler = useRef(onEvent);
  handler.current = onEvent;

  useEffect(() => {
    const client = new LiveClient({
      url: liveUrl(showId),
      token: async (fresh) => {
        const t = await token.current(fresh ? { cacheMode: "off" } : undefined);
        if (!t) throw new Error("no access token");
        return t;
      },
      onConnection: (connection, reason) =>
        setLive((l) => ({ ...l, connection, error: reason ?? l.error })),
      onMessage: (msg) => {
        switch (msg.type) {
          case "welcome":
            setLive((l) => ({
              ...l,
              userId: msg.userId,
              state: msg.state,
              presence: msg.presence,
              error: null,
            }));
            queryClient.invalidateQueries({ queryKey: ["show", showId] });
            return;
          case "state":
            setLive((l) => ({ ...l, state: msg.state }));
            return;
          case "presence": {
            setLive((l) => ({ ...l, presence: msg.presence }));
            // Connecting joins the show: someone who wasn't in it yet is in its details
            // now.
            const show = queryClient.getQueryData<Show>(["show", showId]);
            const known = new Set(show?.participants.map((p) => p.member.id));
            if (show && msg.presence.online.some((id) => !known.has(id))) {
              queryClient.invalidateQueries({ queryKey: ["show", showId] });
            }
            return;
          }
          case "showChanged":
            queryClient.invalidateQueries({ queryKey: ["show", showId] });
            return;
          case "pong":
            return;
          case "showOver":
            setLive((l) => ({ ...l, over: true }));
            queryClient.invalidateQueries({ queryKey: ["show", showId] });
            break;
        }
        handler.current?.(msg);
      },
    });
    // Another show: nothing of the last one carries over.
    setLive({ ...EMPTY, client });
    client.start();
    return () => client.stop();
  }, [showId, queryClient]);

  return live;
}
