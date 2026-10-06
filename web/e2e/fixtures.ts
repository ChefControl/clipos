// API responses for the phone tests. Deliberately awkward data: unbroken file-name
// titles, long names and emails, lots of tags and players. That's what broke the
// layout on a real phone.
import { readFileSync } from "node:fs";
import type { Page, Route } from "@playwright/test";
import type { components } from "../src/api/schema";

type Clip = components["schemas"]["ClipView"];
type Member = components["schemas"]["Member"];
type PastShow = components["schemas"]["PastShow"];
type Show = components["schemas"]["ShowView"];
type Tonight = components["schemas"]["Tonight"];
type User = components["schemas"]["User"];

const POSTER = "/e2e-media/poster.jpg";
const VIDEO = "/e2e-media/clip.mp4";
/** 40 s of test pattern (VP9: Playwright's Chromium has no H.264), for the show tests. */
const SHOW_VIDEO = "/e2e-media/show.webm";
const NOW = "2026-10-01T12:00:00Z";
const PAST_SHOW_ID = "20000000-0000-4000-8000-000000000000";

export const me: User = {
  id: "00000000-0000-4000-8000-000000000001",
  email: "robin.with.a.rather.long.address@example.com",
  handle: "robin",
  displayName: "Robin",
  avatarUrl: null,
  steamName: "RobinPlays",
  role: "admin",
  status: "active",
  createdAt: NOW,
};

const members: Member[] = [
  { id: me.id, handle: "robin", displayName: "Robin", avatarUrl: null, steamName: "RobinPlays" },
  {
    id: "00000000-0000-4000-8000-000000000002",
    handle: "jamie",
    displayName: "Jamie Doe",
    avatarUrl: null,
    steamName: null,
  },
  {
    id: "00000000-0000-4000-8000-000000000003",
    handle: "maximilian-the-awper",
    displayName: "Maximilian Alexander Konstantinopoulos",
    avatarUrl: null,
    steamName: "maxawp",
  },
];

const base = {
  description: "",
  gameId: "cs2",
  myPov: true,
  error: null,
  isMine: false,
  canEdit: true,
  originalFilename: "MedalTVCounterStrike220250408185133.mp4",
  originalBytes: 344_654_036,
  durationMs: 180_237,
  width: 1920,
  height: 1080,
  fps: 59.97,
  tags: [] as string[],
  autoTags: [] as string[],
  multiKill: null as string | null,
  players: [] as Member[],
  reactions: [] as Clip["reactions"],
  reactionCount: 0,
  playbackUrl: null,
  posterUrl: POSTER,
  createdAt: NOW,
  deletedAt: null,
  shareUrl: null,
  map: null,
  heldUntil: null,
  teaser: false,
  clipOfTheNight: false,
  failOfTheNight: false,
} satisfies Partial<Clip>;

export const clips = {
  long: {
    ...base,
    id: "10000000-0000-4000-8000-000000000001",
    title: "MedalTVCounterStrike220250408185133",
    description:
      "Mirage A site retake, 1v3 with the AK. Watch the smoke at 0:42 — no idea how that went through.",
    status: "ready",
    map: "Mirage",
    uploader: { handle: "jamie", displayName: "Jamie Doe", avatarUrl: null },
    tags: ["1v3-clutch", "ak47", "retake", "mirage-a"],
    autoTags: ["ace", "hs", "smoke-kill", "wallbang"],
    multiKill: "ace",
    players: members,
    reactions: [
      { emoji: "🔥", count: 4, mine: true },
      { emoji: "💀", count: 2, mine: false },
      { emoji: "🐐", count: 1, mine: false },
    ],
    reactionCount: 7,
    shareUrl: "https://clips.spawnpoint.run/s/exampleShareToken12345",
    // The last show's clip of the night (pastShow).
    clipOfTheNight: true,
    playedIn: {
      showId: PAST_SHOW_ID,
      startedAt: "2026-09-25T19:00:00Z",
      position: 1,
      count: 2,
      lostTo: null,
    },
  },
  normal: {
    ...base,
    id: "10000000-0000-4000-8000-000000000002",
    title: "Ace on Inferno banana",
    status: "ready",
    map: "Inferno",
    isMine: true,
    uploader: { handle: "robin", displayName: "Robin", avatarUrl: null },
    durationMs: 30_177,
    // The last show's fail of the night (pastShow).
    failOfTheNight: true,
    playedIn: {
      showId: PAST_SHOW_ID,
      startedAt: "2026-09-25T19:00:00Z",
      position: 2,
      count: 2,
      lostTo: {
        clipId: "10000000-0000-4000-8000-000000000001",
        title: "MedalTVCounterStrike220250408185133",
        uploader: "Jamie Doe",
      },
    },
  },
  processing: {
    ...base,
    id: "10000000-0000-4000-8000-000000000003",
    title: "Counter-strike 2 · 2026-10-01 21:04",
    status: "processing",
    isMine: true,
    posterUrl: null,
    durationMs: null,
    width: null,
    height: null,
    fps: null,
    uploader: { handle: "robin", displayName: "Robin", avatarUrl: null },
  },
  failed: {
    ...base,
    id: "10000000-0000-4000-8000-000000000004",
    title: "Broken upload",
    status: "failed",
    isMine: true,
    error: "clips can be at most 5 minutes (this one is 7:12)",
    failureReason: "tooLong",
    posterUrl: null,
    durationMs: null,
    uploader: { handle: "robin", displayName: "Robin", avatarUrl: null },
  },
  held: {
    ...base,
    id: "10000000-0000-4000-8000-000000000005",
    title: "Saved for Friday",
    status: "ready",
    map: "Ancient",
    isMine: true,
    durationMs: 42_000,
    heldUntil: "2026-10-08T12:00:00Z",
    uploader: { handle: "robin", displayName: "Robin", avatarUrl: null },
  },
} satisfies Record<string, Clip>;

/** The show tests' clips, each with the 40 s video (at its own URL). */
export const showClip: Clip = {
  ...base,
  id: "10000000-0000-4000-8000-0000000000aa",
  title: "Show clip",
  status: "ready",
  map: "Mirage",
  durationMs: 40_000,
  uploader: { handle: "jamie", displayName: "Jamie Doe", avatarUrl: null },
};
export const showClip2: Clip = {
  ...showClip,
  id: "10000000-0000-4000-8000-0000000000bb",
  title: "Second show clip",
  map: "Nuke",
};
const showVideos: Record<string, string> = {
  [showClip.id]: SHOW_VIDEO,
  [showClip2.id]: `${SHOW_VIDEO}?clip=2`,
};

export const SHOW_ID = "20000000-0000-4000-8000-000000000001";

export const show: Show = {
  id: SHOW_ID,
  status: "live",
  host: { id: me.id, handle: "robin", displayName: "Robin", avatarUrl: null, steamName: null },
  createdAt: NOW,
  startedAt: NOW,
  endedAt: null,
  lineup: [showClip, showClip2].map((clip, position) => ({
    clip,
    position,
    dropped: false,
    spare: false,
    playedAt: null,
    addedBy: me.id,
  })),
  participants: members.slice(0, 2).map((member) => ({
    member,
    joinedAt: NOW,
    ready: false,
  })),
  failContenders: [],
  voters: { clip: [], fail: [] },
  votes: [],
  myVotes: { clip: null, fail: null },
  clipWinnerId: null,
  failWinnerId: null,
  reactions: [],
};

/** The last show: the long clip won, the normal one was the fail. */
export const pastShow: PastShow = {
  id: PAST_SHOW_ID,
  host: { ...show.host },
  startedAt: "2026-09-25T19:00:00Z",
  endedAt: "2026-09-25T20:00:00Z",
  participants: members,
  clips: [clips.long, clips.normal],
  clipWinnerId: clips.long.id,
  failWinnerId: clips.normal.id,
  clipWinnerVotes: 2,
  failWinnerVotes: 3,
  clipVoters: 3,
  failVoters: 3,
};

/** /tonight with no show on: three clips waiting, one of them saved for the show. */
export const tonight: Tonight = {
  show: null,
  clips: [clips.normal, { ...clips.held, isMine: false, teaser: true }, clips.long],
  lastShow: pastShow,
};

type ClipAnalysis = components["schemas"]["ClipAnalysis"];

/** The long clip's killfeed: an ace, a death, and others' kills, some close together. */
const analysisOfLong: ClipAnalysis = {
  status: "done",
  stats: {
    kills: 9,
    myKills: 5,
    myDeaths: 1,
    multiKill: "ace",
    weapons: { ak47: 4, deagle: 1 },
    modifiers: { headshot: 3, through_smoke: 1, wallbang: 1 },
  },
  kills: [
    { t: 12, owner: "other", weapon: "ak47", modifiers: ["headshot"] },
    { t: 41, owner: "myKill", weapon: "ak47", modifiers: ["through_smoke"] },
    { t: 43, owner: "myKill", weapon: "ak47", modifiers: ["headshot"] },
    { t: 43, owner: "other", weapon: "m4a1_silencer", modifiers: [] },
    { t: 44, owner: "myKill", weapon: "ak47", modifiers: ["headshot", "wallbang"] },
    { t: 47, owner: "myKill", weapon: "deagle", modifiers: ["headshot"] },
    { t: 52, owner: "myKill", weapon: "ak47", modifiers: [] },
    { t: 90, owner: "other", weapon: "awp", modifiers: ["noscope"] },
    { t: 150, owner: "myDeath", weapon: "awp", modifiers: [] },
  ],
};

const withPlayback = (clip: Clip): Clip =>
  clip.status === "ready" ? { ...clip, playbackUrl: VIDEO } : clip;

const json = (route: Route, body: unknown, status = 200) =>
  route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

/** A share link that was revoked (its clip.json is a 404). */
export const GONE_SHARE = "gone0gone0gone0gone0go";

/** Answers every API, share and media request the app makes. `meError` makes /api/me
 *  refuse the account (`not_invited`, `account_disabled`). */
export async function mockApi(page: Page, { meError }: { meError?: string } = {}) {
  const media = {
    "/e2e-media/poster.jpg": [
      "image/jpeg",
      readFileSync(new URL("./media/poster.jpg", import.meta.url)),
    ],
    "/e2e-media/clip.mp4": [
      "video/mp4",
      readFileSync(new URL("./media/clip.mp4", import.meta.url)),
    ],
    "/e2e-media/show.webm": [
      "video/webm",
      readFileSync(new URL("./media/show.webm", import.meta.url)),
    ],
  } as const;
  // Byte ranges, like Blob Storage: browsers seek video with them.
  await page.route("**/e2e-media/**", (route) => {
    const path = new URL(route.request().url()).pathname as keyof typeof media;
    const [contentType, body] = media[path];
    const range = /bytes=(\d+)-(\d*)/.exec(route.request().headers().range ?? "");
    if (!range) {
      return route.fulfill({
        status: 200,
        contentType,
        body,
        headers: { "accept-ranges": "bytes" },
      });
    }
    const start = Number(range[1]);
    const end = range[2] ? Math.min(Number(range[2]), body.length - 1) : body.length - 1;
    return route.fulfill({
      status: 206,
      contentType,
      body: body.subarray(start, end + 1),
      headers: {
        "accept-ranges": "bytes",
        "content-range": `bytes ${start}-${end}/${body.length}`,
      },
    });
  });

  await page.route("**/s/*/clip.json", (route) =>
    route.request().url().includes(GONE_SHARE)
      ? json(route, { error: "not_found", message: "not found" }, 404)
      : json(route, {
          title: clips.long.title,
          uploader: "Jamie Doe",
          map: "Mirage",
          durationMs: 180_237,
          width: 1920,
          height: 1080,
          fps: 59.97,
          videoUrl: VIDEO,
          posterUrl: POSTER,
          createdAt: NOW,
        }),
  );

  // Uploads: blocks and the block list go to "Blob Storage" on the page's own origin.
  await page.route("**/e2e-blob/**", (route) => route.fulfill({ status: 201, body: "" }));

  await page.route("**/api/**", (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    const method = route.request().method();
    const all = Object.values(clips);
    // A new upload becomes the processing clip, so the clip page has something to show.
    if (method === "POST" && path === "/api/clips") {
      return json(
        route,
        {
          clip: { ...clips.processing, status: "uploading" },
          uploadUrl: `${url.origin}/e2e-blob/originals/new.mp4?sig=e2e`,
        },
        201,
      );
    }
    // Nothing uploaded is here already, unless a test says otherwise.
    if (method === "POST" && path === "/api/clips/check") {
      return json(route, { result: "new", clip: null });
    }
    if (method === "POST" && path.endsWith("/complete")) return json(route, clips.processing);
    if (/^\/api\/clips\/[^/]+\/download$/.test(path)) return json(route, { url: VIDEO });
    const hold = path.match(/^\/api\/clips\/([^/]+)\/(hold|release)$/);
    if (method === "POST" && hold) {
      const found = all.find((c) => c.id === hold[1]) ?? clips.processing;
      return json(route, {
        ...found,
        heldUntil: hold[2] === "hold" ? "2026-10-08T12:00:00Z" : null,
      });
    }
    if (method === "PATCH" && /^\/api\/clips\/[^/]+$/.test(path)) {
      return json(route, { ...clips.processing, ...route.request().postDataJSON() });
    }
    if (path === "/api/config") {
      return json(route, {
        auth0Domain: "e2e.example",
        auth0ClientId: "e2e",
        auth0Audience: "https://clips.example/api",
      });
    }
    if (path === "/api/me") {
      return meError
        ? json(route, { error: meError, message: meError }, 403)
        : json(route, { ...me, shows: true, showsForEveryone: false });
    }
    if (path === "/api/me/trash") return json(route, [{ ...clips.normal, deletedAt: NOW }]);
    if (path === "/api/clips") {
      const tag = url.searchParams.get("tag");
      const list = tag ? all.filter((c) => c.tags.includes(tag) || c.autoTags.includes(tag)) : all;
      return json(route, { clips: list, nextCursor: null });
    }
    const analysis = path.match(/^\/api\/clips\/([^/]+)\/analysis$/);
    if (analysis) {
      if (analysis[1] === clips.long.id) return json(route, analysisOfLong);
      if (analysis[1] === clips.normal.id) {
        return json(route, { status: "pending", stats: null, kills: [] });
      }
      return json(route, { error: "not_found", message: "not analysed" }, 404);
    }
    if (path === `/api/shows/${SHOW_ID}`) return json(route, show);
    if (path === "/api/shows/tonight") return json(route, tonight);
    // No show on, unless a test says otherwise.
    if (path === "/api/shows/current") return json(route, null);
    if (method === "GET" && path === "/api/shows") return json(route, [pastShow]);
    const forShow = [showClip, showClip2].find((c) => path === `/api/clips/${c.id}`);
    if (forShow) return json(route, { ...forShow, playbackUrl: showVideos[forShow.id] });
    const clip = path.match(/^\/api\/clips\/([^/]+)$/);
    if (clip) {
      const found = all.find((c) => c.id === clip[1]);
      return found
        ? json(route, withPlayback(found))
        : json(route, { error: "not_found", message: "not found" }, 404);
    }
    if (path === "/api/users") return json(route, members);
    const profile = path.match(/^\/api\/users\/([^/]+)$/);
    if (profile) {
      const member = members.find((m) => m.handle === profile[1]);
      return member
        ? json(route, {
            ...member,
            clipCount: 12,
            featuredCount: 3,
            fireCount: 17,
            joinedAt: NOW,
            shows: {
              hosted: member.handle === "robin" ? 1 : 0,
              trophies: [
                { category: "clip" as const, clip: clips.long },
                { category: "fail" as const, clip: clips.normal },
              ]
                .filter((t) => t.clip.uploader.handle === member.handle)
                .map((t) => ({ ...t, showId: PAST_SHOW_ID, showStartedAt: pastShow.startedAt })),
            },
          })
        : json(route, { error: "not_found", message: "not found" }, 404);
    }
    if (path === "/api/tags") return json(route, ["ace", "1v3-clutch"]);
    if (method === "POST" && /^\/api\/admin\/clips\/[^/]+\/analyse$/.test(path)) {
      return json(route, { pending: true }, 202);
    }
    if (path === "/api/admin/duplicates") return json(route, { groups: [], unchecked: 0 });
    if (path === "/api/admin/invites") {
      return json(route, [
        {
          email: "maximilian.alexander.konstantinopoulos.gaming@example.com",
          role: "member",
          invitedBy: "robin",
          createdAt: NOW,
          acceptedAt: null,
          revokedAt: null,
        },
        {
          email: "jamie@example.com",
          role: "member",
          invitedBy: "robin",
          createdAt: NOW,
          acceptedAt: NOW,
          revokedAt: null,
        },
        {
          email: me.email,
          role: "admin",
          invitedBy: null,
          createdAt: NOW,
          acceptedAt: NOW,
          revokedAt: null,
        },
        {
          email: "old@friend.com",
          role: "member",
          invitedBy: "robin",
          createdAt: NOW,
          acceptedAt: null,
          revokedAt: NOW,
        },
      ]);
    }
    if (path === "/api/admin/users") {
      return json(
        route,
        members.map((m, i) => ({
          ...me,
          id: m.id,
          handle: m.handle,
          displayName: m.displayName,
          email: `${m.handle}.long.email.address@example.com`,
          role: i === 0 ? "admin" : "member",
          status: i === 2 ? "disabled" : "active",
        })),
      );
    }
    return json(route, { error: "not_found", message: `no fixture for ${path}` }, 404);
  });
}
