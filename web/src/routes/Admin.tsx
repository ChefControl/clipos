import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { type FormEvent, useState } from "react";
import { call } from "../api/errors";
import type { components } from "../api/schema";
import { useApi } from "../auth/ApiProvider";
import type { Clip } from "../clips/hooks";
import { Kip } from "../kip/Kip";
import { shortDate } from "../lib/format";
import { useTitle } from "../lib/useTitle";
import { Avatar } from "../ui/Avatar";
import { Button } from "../ui/Button";
import { Pill } from "../ui/Pill";
import { type User, useMe } from "./useMe";

type Invite = components["schemas"]["Invite"];
type Role = components["schemas"]["Role"];
type UpdateUserAccess = components["schemas"]["UpdateUserAccess"];

const inviteLink = () => `${window.location.origin}/?invite=1`;

// Admin (canvas 5.1): invites beside members. Reached from your profile's Account panel.
export function Admin() {
  const { data: me } = useMe();
  useTitle("Admin");
  if (me && me.role !== "admin") {
    return <p className="text-muted">Admins only.</p>;
  }
  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-end gap-5">
        <div className="flex flex-col gap-1.5">
          <span className="font-mono text-xs text-accent">Admins only</span>
          <h1 className="text-5xl leading-[0.95] font-extrabold tracking-tight sm:text-[52px]">
            Admin
          </h1>
        </div>
        <Kip pose="bouncer" className="-mb-4 ml-auto h-24 w-24 sm:h-[120px] sm:w-[120px]" />
      </div>
      <div className="grid grid-cols-1 items-start gap-6 lg:grid-cols-2">
        <Invites meEmail={me?.email} />
        <Users meId={me?.id} />
      </div>
      <Duplicates />
    </div>
  );
}

const selectClass =
  "h-[34px] appearance-none rounded-full bg-white/8 bg-[length:14px] bg-[right_10px_center] bg-no-repeat pr-8 pl-3 text-[13px] font-semibold text-text disabled:opacity-50";
// The select's chevron, drawn as a background so the native control stays.
const chevron = {
  backgroundImage:
    "url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%23c9c6be' stroke-width='2.2' stroke-linecap='round'%3E%3Cpath d='M6 9l6 6 6-6'/%3E%3C/svg%3E\")",
};

function Invites({ meEmail }: { meEmail?: string }) {
  const api = useApi();
  const queryClient = useQueryClient();
  const invites = useQuery({
    queryKey: ["admin", "invites"],
    queryFn: () => call(api.GET("/api/admin/invites")),
  });
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["admin"] });

  const create = useMutation({
    mutationFn: (body: { email: string; role: Role }) =>
      call(api.POST("/api/admin/invites", { body })),
    onSuccess: refresh,
  });
  const revoke = useMutation({
    mutationFn: (email: string) => call(api.POST("/api/admin/invites/revoke", { body: { email } })),
    onSuccess: refresh,
  });

  const [email, setEmail] = useState("");
  const [role, setRole] = useState<Role>("member");
  const submit = (e: FormEvent) => {
    e.preventDefault();
    create.mutate({ email, role }, { onSuccess: () => setEmail("") });
  };

  return (
    <section className="glass squircle flex flex-col gap-3.5 rounded-[30px] px-5 pt-6 pb-3 sm:px-[26px]">
      <div className="flex flex-wrap items-center gap-3">
        <h2 className="text-[26px] font-extrabold">Invites</h2>
        <CopyButton text={inviteLink()} label="Copy invite link" />
      </div>
      <p className="text-[15px] text-soft">
        Add their Google email, then send them the invite link (no email is sent).
      </p>

      <form onSubmit={submit} className="flex flex-wrap gap-2">
        <input
          type="email"
          required
          value={email}
          onChange={(e) => setEmail(e.target.value)}
          placeholder="friend@gmail.com"
          aria-label="Email to invite"
          className="squircle h-[46px] min-w-0 flex-1 basis-56 rounded-[14px] bg-white/6 px-4 text-base ring-1 ring-white/10 ring-inset placeholder:text-muted focus:ring-accent focus:outline-none"
        />
        <select
          value={role}
          onChange={(e) => setRole(e.target.value as Role)}
          aria-label="Role"
          style={chevron}
          className={`${selectClass} squircle h-[46px] rounded-[14px]`}
        >
          <option value="member">Member</option>
          <option value="admin">Admin</option>
        </select>
        <Button type="submit" variant="primary" disabled={create.isPending} className="h-[46px]">
          Invite
        </Button>
      </form>
      <ErrorText error={create.error ?? revoke.error ?? invites.error} />

      <ul className="mt-1.5 flex flex-col">
        {invites.isPending && <li className="py-3 text-muted">Loading…</li>}
        {invites.data?.length === 0 && <li className="py-3 text-muted">No invites yet.</li>}
        {invites.data?.map((invite) => (
          <li
            key={invite.email}
            className="flex flex-wrap items-center gap-x-3 gap-y-2 border-t border-white/7 py-3"
          >
            <div className="flex min-w-0 flex-1 flex-col">
              <span
                className={`font-semibold [overflow-wrap:anywhere] ${invite.revokedAt ? "text-muted line-through" : ""}`}
              >
                {invite.email}
              </span>
              <span className="text-xs text-muted">
                {invite.role === "admin" ? "Admin · " : ""}
                {invite.invitedBy ? `invited by @${invite.invitedBy}` : "from ADMIN_EMAILS"}
              </span>
            </div>
            <div className="ml-auto flex shrink-0 items-center gap-2">
              <InviteStatus invite={invite} />
              {invite.email === meEmail ? null : invite.revokedAt ? (
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => create.mutate({ email: invite.email, role: invite.role })}
                >
                  Re-invite
                </Button>
              ) : (
                <Button
                  size="sm"
                  onClick={() => {
                    if (window.confirm(`Revoke ${invite.email}? They won't be able to sign in.`)) {
                      revoke.mutate(invite.email);
                    }
                  }}
                >
                  Revoke
                </Button>
              )}
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}

function InviteStatus({ invite }: { invite: Invite }) {
  if (invite.revokedAt) return <Pill>Revoked</Pill>;
  if (invite.acceptedAt) return <Pill tone="green">Joined</Pill>;
  return <Pill tone="accent">Pending</Pill>;
}

function Users({ meId }: { meId?: string }) {
  const api = useApi();
  const queryClient = useQueryClient();
  const users = useQuery({
    queryKey: ["admin", "users"],
    queryFn: () => call(api.GET("/api/admin/users")),
  });
  const update = useMutation({
    mutationFn: ({ id, body }: { id: string; body: UpdateUserAccess }) =>
      call(api.PATCH("/api/admin/users/{id}", { params: { path: { id } }, body })),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["admin"] }),
  });
  const disabled = users.data?.filter((u) => u.status === "disabled").length ?? 0;

  return (
    <section className="glass squircle flex flex-col gap-2 rounded-[30px] px-5 pt-6 pb-3 sm:px-[26px]">
      <div className="flex items-center gap-3 pb-1.5">
        <h2 className="text-[26px] font-extrabold">Members</h2>
        {users.data && (
          <span className="font-mono text-xs text-muted">
            {users.data.length}
            {disabled > 0 && ` · ${disabled} disabled`}
          </span>
        )}
      </div>
      <ErrorText error={update.error ?? users.error} />
      <ul className="flex flex-col">
        {users.isPending && <li className="py-3 text-muted">Loading…</li>}
        {users.data?.map((user) => (
          <UserRow
            key={user.id}
            user={user}
            isMe={user.id === meId}
            busy={update.isPending}
            onChange={(body) => update.mutate({ id: user.id, body })}
          />
        ))}
      </ul>
      <p className="py-2 text-[13px] text-muted">
        Disabled members can't sign in. Their clips stay in the archive.
      </p>
    </section>
  );
}

function UserRow({
  user,
  isMe,
  busy,
  onChange,
}: {
  user: User;
  isMe: boolean;
  busy: boolean;
  onChange: (body: UpdateUserAccess) => void;
}) {
  const disabled = user.status === "disabled";
  return (
    <li
      className={`flex flex-wrap items-center gap-x-3 gap-y-2 border-t border-white/7 py-2.5 ${disabled ? "opacity-60" : ""}`}
    >
      <Avatar name={user.displayName} url={user.avatarUrl} size={34} host={isMe} />
      <div className="flex min-w-0 flex-1 flex-col">
        <span dir="auto" className="truncate font-bold">
          {user.displayName}
        </span>
        <span className="truncate text-xs text-muted">
          @{user.handle}
          {user.role === "admin" && (isMe || disabled) ? " · Admin" : ""} · {user.email}
        </span>
      </div>
      <div className="ml-auto flex shrink-0 items-center gap-1.5">
        {isMe ? (
          <span className="text-xs text-muted">That's you</span>
        ) : disabled ? (
          <>
            <Pill tone="danger">Disabled</Pill>
            <Button size="sm" disabled={busy} onClick={() => onChange({ status: "active" })}>
              Enable
            </Button>
          </>
        ) : (
          <>
            <select
              value={user.role}
              disabled={busy}
              onChange={(e) => onChange({ role: e.target.value as Role })}
              aria-label={`Role for ${user.displayName}`}
              style={chevron}
              className={selectClass}
            >
              <option value="member">Member</option>
              <option value="admin">Admin</option>
            </select>
            <Button
              size="sm"
              variant="ghost"
              disabled={busy}
              onClick={() => onChange({ status: "disabled" })}
            >
              Disable
            </Button>
          </>
        )}
      </div>
    </li>
  );
}

// Copies of one file, from before uploads were checked (decision 55): keep one of each.
function Duplicates() {
  const api = useApi();
  const queryClient = useQueryClient();
  const copies = useQuery({
    queryKey: ["admin", "duplicates"],
    queryFn: () => call(api.GET("/api/admin/duplicates")),
  });
  const remove = useMutation({
    mutationFn: (id: string) => call(api.DELETE("/api/clips/{id}", { params: { path: { id } } })),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["admin", "duplicates"] });
      queryClient.invalidateQueries({ queryKey: ["clips"] });
    },
  });
  const groups = copies.data?.groups ?? [];
  const unchecked = copies.data?.unchecked ?? 0;

  return (
    <section className="glass squircle flex flex-col gap-3 rounded-[30px] px-5 pt-6 pb-4 sm:px-[26px]">
      <div className="flex items-center gap-3">
        <h2 className="text-[26px] font-extrabold">Duplicate clips</h2>
        {copies.data && groups.length > 0 && (
          <span className="font-mono text-xs text-muted">
            {groups.length} {groups.length === 1 ? "file" : "files"}
          </span>
        )}
      </div>
      <p className="text-[15px] text-soft">
        The same file uploaded more than once, before Kip checked uploads. Keep one of each and
        delete the rest: a deleted copy goes to its uploader's trash, and can't be restored while
        the one you kept is here.
      </p>
      {unchecked > 0 && (
        <p className="text-sm text-muted">
          Kip is still checking {unchecked === 1 ? "1 older clip" : `${unchecked} older clips`}, so
          this list may grow.
        </p>
      )}
      <ErrorText error={remove.error ?? copies.error} />
      {copies.isPending && <p className="py-2 text-muted">Loading…</p>}
      {copies.data && groups.length === 0 && <p className="py-2 text-muted">No duplicates.</p>}
      <ul className="flex flex-col gap-3">
        {groups.map((group) => (
          <li
            key={group.map((c) => c.id).join()}
            aria-label={`Copies of ${group[0]?.title}`}
            className="squircle flex flex-col rounded-[20px] bg-white/4 px-4 py-1"
          >
            <ul className="flex flex-col divide-y divide-white/7">
              {group.map((clip, i) => (
                <CopyRow
                  key={clip.id}
                  clip={clip}
                  oldest={i === 0}
                  busy={remove.isPending}
                  onDelete={() => {
                    const by = clip.uploader.displayName;
                    if (
                      window.confirm(`Delete “${clip.title}” by ${by}? It goes to their trash.`)
                    ) {
                      remove.mutate(clip.id);
                    }
                  }}
                />
              ))}
            </ul>
          </li>
        ))}
      </ul>
    </section>
  );
}

function CopyRow({
  clip,
  oldest,
  busy,
  onDelete,
}: {
  clip: Clip;
  oldest: boolean;
  busy: boolean;
  onDelete: () => void;
}) {
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-2 py-2.5">
      <Avatar name={clip.uploader.displayName} url={clip.uploader.avatarUrl} size={34} />
      <div className="flex min-w-0 flex-1 flex-col">
        {clip.teaser ? (
          <span dir="auto" className="truncate font-bold">
            {clip.title}
          </span>
        ) : (
          <Link
            to="/clips/$clipId"
            params={{ clipId: clip.id }}
            dir="auto"
            className="truncate font-bold hover:underline"
          >
            {clip.title}
          </Link>
        )}
        <span className="truncate text-xs text-muted">
          {clip.uploader.displayName} · {shortDate(clip.createdAt)}
          {clip.teaser
            ? " · saved for the show"
            : ` · ${clip.reactionCount} ${clip.reactionCount === 1 ? "reaction" : "reactions"}`}
        </span>
      </div>
      <div className="ml-auto flex shrink-0 items-center gap-1.5">
        {oldest && <Pill tone="green">First upload</Pill>}
        {clip.teaser ? (
          <span className="text-xs text-muted">Can be deleted once it's posted</span>
        ) : (
          <Button size="sm" disabled={busy} onClick={onDelete}>
            Delete
          </Button>
        )}
      </div>
    </li>
  );
}

function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      onClick={async () => {
        await navigator.clipboard.writeText(text);
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      }}
      className="frost ml-auto flex h-10 items-center gap-2 rounded-full pr-4 pl-3 text-sm font-semibold ring-1 ring-white/10 ring-inset hover:bg-white/10"
    >
      <svg
        viewBox="0 0 24 24"
        className="h-4 w-4"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        aria-hidden="true"
      >
        <rect x="9" y="9" width="12" height="12" rx="3" />
        <path d="M5 15V6a3 3 0 0 1 3-3h9" />
      </svg>
      {copied ? "Copied ✓" : label}
    </button>
  );
}

function ErrorText({ error }: { error: Error | null }) {
  if (!error) {
    return null;
  }
  return (
    <p role="alert" className="text-sm text-danger">
      {error.message}
    </p>
  );
}
