import { useAuth0 } from "@auth0/auth0-react";
import { getRouteApi, Link, useNavigate } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { isNotFound } from "../api/errors";
import { ClipGrid } from "../clips/ClipGrid";
import { type Clip, useProfile, useRestoreClip, useTrash } from "../clips/hooks";
import { useTitle } from "../lib/useTitle";
import { Avatar } from "../ui/Avatar";
import { Button } from "../ui/Button";
import { LoadError } from "../ui/LoadError";
import { Modal } from "../ui/Modal";
import { Panel } from "../ui/Panel";
import { Pill } from "../ui/Pill";
import { useToast } from "../ui/Toast";
import { NotFound } from "./NotFound";
import { EditProfileForm } from "./Profile";
import { type User, useMe } from "./useMe";

const route = getRouteApi("/u/$handle");

/** `/u/{handle}` search params: `edit` opens Edit profile (your own page only); `tab`
 *  shows the clips they play in instead of their uploads. */
export function userSearch(search: Record<string, unknown>): {
  edit?: boolean;
  tab?: "featured";
} {
  return {
    ...(search.edit === true || search.edit === "true" ? { edit: true } : {}),
    ...(search.tab === "featured" ? { tab: "featured" as const } : {}),
  };
}

const monthYear = (iso: string) =>
  new Date(iso).toLocaleDateString("en-US", { month: "short", year: "numeric" });

// Your profile and a friend's page (canvas 4.2, 4.3). The trophy shelf and the show
// stats (shows hosted, clips and fails of the night) arrive with the shows in S7.
export function UserPage() {
  const { handle } = route.useParams();
  const { edit, tab: tabParam } = route.useSearch();
  const navigate = useNavigate({ from: "/u/$handle" });
  const profile = useProfile(handle);
  const { data: me } = useMe();
  const isMe = me?.handle === handle;
  const tab = tabParam ?? "uploads";
  useTitle(profile.data?.displayName);

  if (isNotFound(profile.error)) {
    return <NotFound />;
  }
  if (profile.error) {
    return <LoadError error={profile.error} onRetry={profile.refetch} />;
  }
  const p = profile.data;
  const setTab = (t: "uploads" | "featured") =>
    navigate({ search: t === "featured" ? { tab: "featured" } : {}, replace: true });
  // Closing Edit profile leaves you on the tab you were on.
  const keepTab = tabParam ? { tab: tabParam } : {};

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-wrap items-center gap-x-6 gap-y-4">
        <Avatar
          name={p?.displayName ?? handle}
          url={p?.avatarUrl}
          size={104}
          className={isMe ? "ring-[3px] ring-accent ring-offset-[3px] ring-offset-bg" : ""}
        />
        <div className="flex min-w-0 flex-1 flex-col gap-1.5">
          <h1
            dir="auto"
            className="text-5xl leading-[0.9] font-extrabold tracking-tight [overflow-wrap:anywhere] sm:text-[56px]"
          >
            {p?.displayName ?? "…"}
          </h1>
          <p className="text-base text-soft [overflow-wrap:anywhere]">
            @{handle}
            {p?.steamName && ` · Steam: ${p.steamName}`}
            {isMe && <span className="text-accent"> · you</span>}
          </p>
        </div>
        {isMe && me && <OwnProfileActions />}
      </div>

      {p && (
        <div className="glass squircle flex flex-wrap items-center gap-x-11 gap-y-3 rounded-3xl px-6 py-4">
          <Stat n={p.clipCount} label={p.clipCount === 1 ? "clip" : "clips"} />
          <Stat n={p.featuredCount} label="featured in" />
          <Stat n={`🔥 ${p.fireCount}`} label="reactions" />
          <span className="text-sm text-muted sm:ml-auto">Since {monthYear(p.joinedAt)}</span>
        </div>
      )}

      <div
        className={`grid grid-cols-1 items-start gap-6 ${isMe ? "lg:grid-cols-[minmax(0,1fr)_360px]" : ""}`}
      >
        <div className="flex min-w-0 flex-col gap-5">
          <div className="flex self-start rounded-full bg-white/5 p-1">
            {(
              [
                ["uploads", `Uploads${p ? ` · ${p.clipCount}` : ""}`],
                ["featured", `Featured in${p ? ` · ${p.featuredCount}` : ""}`],
              ] as const
            ).map(([key, label]) => (
              <button
                key={key}
                type="button"
                aria-pressed={tab === key}
                onClick={() => setTab(key)}
                className={`h-9 rounded-full px-4 text-[15px] font-semibold ${
                  tab === key ? "bg-white/14 text-text" : "text-soft hover:text-text"
                }`}
              >
                {label}
              </button>
            ))}
          </div>
          <ClipGrid
            key={tab}
            filters={tab === "uploads" ? { uploader: handle } : { player: handle }}
            empty={
              <p>{tab === "uploads" ? "No clips uploaded yet." : "Not tagged in any clips yet."}</p>
            }
          />
        </div>
        {isMe && me && (
          <aside className="flex flex-col gap-4">
            <Account me={me} />
            <Trash />
          </aside>
        )}
      </div>

      {isMe && me && (
        <Modal
          open={!!edit}
          // Saving has already navigated (maybe to a new handle): nothing more to do then.
          onClose={() => edit && navigate({ search: keepTab, replace: true })}
          title="Edit profile"
        >
          <EditProfileForm
            me={me}
            onDone={(user) =>
              navigate({
                to: "/u/$handle",
                params: { handle: user?.handle ?? handle },
                search: keepTab,
                replace: true,
              })
            }
          />
        </Modal>
      )}
    </div>
  );
}

function Stat({ n, label }: { n: ReactNode; label: string }) {
  return (
    <span className="flex flex-col gap-0.5">
      <span className="text-3xl leading-none font-extrabold">{n}</span>
      <span className="text-[13px] text-muted">{label}</span>
    </span>
  );
}

function OwnProfileActions() {
  const { logout } = useAuth0();
  const { tab } = route.useSearch();
  return (
    // A row of its own on phones, so it doesn't squeeze the name.
    <div className="flex w-full gap-2 sm:w-auto">
      <Link
        to="."
        search={{ edit: true, ...(tab ? { tab } : {}) }}
        className="frost flex h-[42px] items-center gap-2 rounded-full pr-4 pl-3.5 text-[15px] font-semibold ring-1 ring-white/10 ring-inset hover:bg-white/10"
      >
        <Icon d="M4 20h4L19 9l-4-4L4 16zM14 6l4 4" />
        Edit profile
      </Link>
      <button
        type="button"
        onClick={() => logout({ logoutParams: { returnTo: window.location.origin } })}
        className="frost flex h-[42px] items-center gap-2 rounded-full pr-4 pl-3.5 text-[15px] font-semibold text-soft ring-1 ring-white/10 ring-inset hover:bg-white/10 hover:text-text"
      >
        <Icon d="M15 4h3a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-3M10 17l5-5-5-5M15 12H4" />
        Sign out
      </button>
    </div>
  );
}

function Icon({ d }: { d: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      className="h-[17px] w-[17px]"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d={d} />
    </svg>
  );
}

function Account({ me }: { me: User }) {
  const row = (label: string, value: ReactNode) => (
    <div className="flex items-center gap-3 border-t border-white/7 py-2.5">
      <dt className="w-[92px] shrink-0 text-sm text-muted">{label}</dt>
      <dd className="min-w-0 text-[15px] font-semibold [overflow-wrap:anywhere]">{value}</dd>
    </div>
  );
  return (
    <Panel as="section" padding="px-5 pt-4 pb-2" className="flex flex-col">
      <h2 className="pb-1.5 text-lg font-bold">Account</h2>
      <dl>
        {row("Email", me.email)}
        {row("Steam", me.steamName ?? "—")}
        {row(
          "Role",
          <Pill tone={me.role === "admin" ? "accent" : "neutral"}>
            {me.role === "admin" ? "Admin" : "Member"}
          </Pill>,
        )}
        {row(
          "Joined",
          new Date(me.createdAt).toLocaleDateString("en-US", {
            month: "short",
            day: "numeric",
            year: "numeric",
          }),
        )}
      </dl>
      {me.role === "admin" && (
        <Link
          to="/admin"
          className="mt-1.5 mb-2 flex items-center gap-2 border-t border-white/7 py-2.5 text-sm font-semibold text-accent hover:text-accent-strong"
        >
          Invites and members
          <Icon d="M9 6l6 6-6 6" />
        </Link>
      )}
    </Panel>
  );
}

const TRASH_DAYS = 7;

function Trash() {
  const trash = useTrash(true);
  return (
    <Panel as="section" padding="px-5 py-4" className="flex flex-col gap-2.5">
      <h2 className="text-lg font-bold">Trash</h2>
      <p className="text-[13px] text-muted">Deleted clips are removed for good after 7 days.</p>
      {trash.error && <LoadError error={trash.error} onRetry={trash.refetch} className="text-sm" />}
      {trash.data?.length === 0 && <p className="text-sm text-soft">Nothing in the trash.</p>}
      {trash.data?.map((clip) => (
        <TrashRow key={clip.id} clip={clip} />
      ))}
    </Panel>
  );
}

function TrashRow({ clip }: { clip: Clip }) {
  const restore = useRestoreClip(clip.id);
  const toast = useToast();
  const deleted = clip.deletedAt ? new Date(clip.deletedAt).getTime() : Date.now();
  const days = Math.max(
    0,
    Math.ceil((deleted + TRASH_DAYS * 86_400_000 - Date.now()) / 86_400_000),
  );
  return (
    <div className="flex items-center gap-3">
      <Link
        to="/clips/$clipId"
        params={{ clipId: clip.id }}
        aria-label={`Open ${clip.title}`}
        className="shrink-0"
      >
        {clip.posterUrl ? (
          <img
            src={clip.posterUrl}
            alt=""
            className="squircle aspect-video w-[88px] rounded-[10px] object-cover opacity-80 grayscale-[.6]"
          />
        ) : (
          <span className="squircle block aspect-video w-[88px] rounded-[10px] bg-surface-2" />
        )}
      </Link>
      <div className="flex min-w-0 flex-col">
        <span dir="auto" className="truncate text-[15px] font-bold">
          {clip.title}
        </span>
        <span className="text-xs text-muted">
          {days <= 1 ? "gone for good tomorrow" : `gone for good in ${days} days`}
        </span>
      </div>
      <Button
        size="sm"
        className="ml-auto"
        disabled={restore.isPending}
        onClick={() =>
          restore.mutate(undefined, {
            onSuccess: () => toast(`Restored ${clip.title}.`),
            onError: (err) => toast(`Couldn't restore it: ${err.message}`, "danger"),
          })
        }
      >
        Restore
      </Button>
    </div>
  );
}
