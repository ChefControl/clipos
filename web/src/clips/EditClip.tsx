import { type FormEvent, useState } from "react";
import { CS2_MAPS } from "../lib/maps";
import { Avatar } from "../ui/Avatar";
import { Button } from "../ui/Button";
import { Chip } from "../ui/Chip";
import { Switch } from "../ui/Switch";
import { type Clip, useMembers, useTagSuggestions, useUpdateClip } from "./hooks";

/** Splits "ace, 1v3 clutch" into tags; the API normalizes them. */
export function parseTags(raw: string): string[] {
  return raw
    .split(",")
    .map((t) => t.trim())
    .filter(Boolean);
}

export function EditClip({ clip, onDone }: { clip: Clip; onDone: () => void }) {
  const update = useUpdateClip(clip.id);
  const members = useMembers();
  const [title, setTitle] = useState(clip.title);
  const [description, setDescription] = useState(clip.description);
  const [map, setMap] = useState(clip.map ?? "");
  const [myPov, setMyPov] = useState(clip.myPov);
  const [tags, setTags] = useState(clip.tags.join(", "));
  const [players, setPlayers] = useState(new Set(clip.players.map((p) => p.id)));
  const lastTag = tags.split(",").pop()?.trim() ?? "";
  const suggestions = useTagSuggestions(lastTag);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    update.mutate(
      { title, description, map, myPov, tags: parseTags(tags), players: [...players] },
      { onSuccess: onDone },
    );
  };
  const togglePlayer = (id: string) =>
    setPlayers((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <form onSubmit={submit} className="grid gap-4">
      <label className="grid gap-1 text-sm">
        <span className="text-muted">Title</span>
        <input
          required
          maxLength={100}
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          className="input"
        />
      </label>
      <label className="grid gap-1 text-sm">
        <span className="text-muted">Description</span>
        <textarea
          maxLength={2000}
          rows={3}
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          className="input"
        />
      </label>
      <fieldset className="grid gap-2 text-sm">
        <legend className="mb-1 text-muted">Map</legend>
        <div className="flex flex-wrap gap-1.5">
          {/* A map typed before the chips existed stays choosable. */}
          {[...CS2_MAPS, ...(map && !CS2_MAPS.includes(map) ? [map] : [])].map((m) => (
            <Chip key={m} pressed={map === m} onClick={() => setMap(map === m ? "" : m)}>
              {m}
            </Chip>
          ))}
        </div>
      </fieldset>
      <label className="grid gap-1 text-sm">
        <span className="text-muted">Tags, comma-separated</span>
        <input
          list="edit-tags"
          value={tags}
          placeholder="ace, 1v3, smoke-kill"
          onChange={(e) => setTags(e.target.value)}
          className="input"
        />
      </label>
      <datalist id="edit-tags">
        {suggestions.data?.map((t) => {
          const head = tags.includes(",") ? `${tags.slice(0, tags.lastIndexOf(",") + 1)} ` : "";
          return <option key={t} value={`${head}${t}`} />;
        })}
      </datalist>

      <fieldset className="grid gap-2 text-sm">
        <legend className="mb-1 text-muted">Friends in this clip</legend>
        <div className="flex flex-wrap gap-2">
          {members.data?.map((m) => {
            const on = players.has(m.id);
            return (
              <button
                key={m.id}
                type="button"
                onClick={() => togglePlayer(m.id)}
                aria-pressed={on}
                className={`flex h-8 items-center gap-1.5 rounded-full pr-3 pl-1 text-[13px] font-semibold ${
                  on
                    ? "bg-accent/16 shadow-[inset_0_0_0_1.5px_var(--color-accent)]"
                    : "bg-white/8 hover:bg-white/12"
                }`}
              >
                <Avatar name={m.displayName} url={m.avatarUrl} size={22} />
                <bdi>{m.displayName}</bdi>
              </button>
            );
          })}
        </div>
      </fieldset>

      <Switch
        checked={myPov}
        onChange={setMyPov}
        label="Recorded from the uploader's point of view"
        hint="Kip only counts kills as theirs when it's their screen."
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
        <Button variant="ghost" onClick={onDone}>
          Cancel
        </Button>
      </div>
    </form>
  );
}
