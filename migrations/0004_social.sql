-- Tags, "friends in this clip", and emoji reactions.

CREATE TABLE tags (
    id   bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    -- Lower-case slug, e.g. `ace`, `1v3`, `smoke-kill`.
    name text NOT NULL UNIQUE CHECK (name ~ '^[a-z0-9][a-z0-9-]{0,23}$')
);

CREATE TABLE clip_tags (
    clip_id uuid NOT NULL REFERENCES clips (id) ON DELETE CASCADE,
    tag_id  bigint NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    -- `auto` tags come from killfeed analysis (phase 8).
    source  text NOT NULL DEFAULT 'user' CHECK (source IN ('user', 'auto')),
    PRIMARY KEY (clip_id, tag_id)
);
CREATE INDEX clip_tags_tag_idx ON clip_tags (tag_id);

CREATE TABLE clip_players (
    clip_id uuid NOT NULL REFERENCES clips (id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (clip_id, user_id)
);
CREATE INDEX clip_players_user_idx ON clip_players (user_id);

CREATE TABLE reactions (
    clip_id    uuid NOT NULL REFERENCES clips (id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    emoji      text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (clip_id, user_id, emoji)
);

-- Kept in step with `reactions` by core::social, for the "top" sort.
ALTER TABLE clips ADD COLUMN reaction_count integer NOT NULL DEFAULT 0;
CREATE INDEX clips_top_idx ON clips (reaction_count DESC, created_at DESC, id DESC)
    WHERE deleted_at IS NULL AND status = 'ready';
CREATE INDEX clips_map_idx ON clips (map) WHERE deleted_at IS NULL;
CREATE INDEX clips_deleted_idx ON clips (deleted_at) WHERE deleted_at IS NOT NULL;
