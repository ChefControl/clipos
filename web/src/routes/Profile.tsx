import { Navigate } from "@tanstack/react-router";
import { type FormEvent, useState } from "react";
import { Button } from "../ui/Button";
import { type User, useMe, useUpdateMe } from "./useMe";

// `/me` (old links): your profile with Edit profile open.
export function Profile() {
  const { data: me } = useMe();
  if (!me) return null;
  return (
    <Navigate to="/u/$handle" params={{ handle: me.handle }} search={{ edit: true }} replace />
  );
}

export function EditProfileForm({ me, onDone }: { me: User; onDone: (user?: User) => void }) {
  const update = useUpdateMe();
  const [displayName, setDisplayName] = useState(me.displayName);
  const [handle, setHandle] = useState(me.handle);
  const [steamName, setSteamName] = useState(me.steamName ?? "");

  const submit = (e: FormEvent) => {
    e.preventDefault();
    update.mutate({ displayName, handle, steamName }, { onSuccess: (user) => onDone(user) });
  };

  return (
    <form onSubmit={submit} className="flex flex-col gap-4">
      <Input label="Display name" value={displayName} onChange={setDisplayName} maxLength={50} />
      <Input
        label="Handle"
        value={handle}
        onChange={setHandle}
        maxLength={32}
        hint="a–z, 0–9, - and _. Shown as @handle."
      />
      <Input
        label="Steam name"
        value={steamName}
        onChange={setSteamName}
        maxLength={64}
        hint="So friends can tag you in clips. Optional."
      />
      {update.error && (
        <p role="alert" className="text-sm text-danger">
          {update.error.message}
        </p>
      )}
      <div className="flex gap-2">
        <Button type="submit" variant="primary" disabled={update.isPending}>
          Save
        </Button>
        <Button variant="ghost" onClick={() => onDone()}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

function Input({
  label,
  value,
  onChange,
  maxLength,
  hint,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  maxLength: number;
  hint?: string;
}) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="text-sm font-semibold text-soft">{label}</span>
      <input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        maxLength={maxLength}
        className="squircle h-11 rounded-[14px] bg-white/6 px-4 text-base ring-1 ring-white/10 ring-inset focus:ring-accent focus:outline-none"
      />
      {hint && <span className="text-xs text-muted">{hint}</span>}
    </label>
  );
}
