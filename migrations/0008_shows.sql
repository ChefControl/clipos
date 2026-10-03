-- "The show" (docs/PLAN.md, redesign, S4): a synced watch party of tonight's clips with a
-- finale vote for clip and fail of the night. One show at a time for the whole group.

CREATE TABLE shows (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    host_id         uuid NOT NULL REFERENCES users (id),
    -- lobby → live → finale → ended. `abandoned`: everyone left; no finale, nothing
    -- counts as played.
    status          text NOT NULL DEFAULT 'lobby'
                    CHECK (status IN ('lobby', 'live', 'finale', 'ended', 'abandoned')),
    created_at      timestamptz NOT NULL DEFAULT now(),
    started_at      timestamptz,
    ended_at        timestamptz,
    clip_winner_id  uuid REFERENCES clips (id) ON DELETE SET NULL,
    fail_winner_id  uuid REFERENCES clips (id) ON DELETE SET NULL
);
-- One open show at a time (decision 28).
CREATE UNIQUE INDEX shows_one_open_idx ON shows ((true))
    WHERE status IN ('lobby', 'live', 'finale');
CREATE INDEX shows_ended_idx ON shows (ended_at DESC) WHERE status = 'ended';

-- The lineup, in order. A dropped clip keeps its row (marked) so it stays for the next show.
CREATE TABLE show_clips (
    show_id   uuid NOT NULL REFERENCES shows (id) ON DELETE CASCADE,
    clip_id   uuid NOT NULL REFERENCES clips (id) ON DELETE CASCADE,
    position  integer NOT NULL,
    added_by  uuid REFERENCES users (id) ON DELETE SET NULL,
    dropped   boolean NOT NULL DEFAULT false,
    played_at timestamptz,
    PRIMARY KEY (show_id, clip_id)
);
CREATE INDEX show_clips_clip_idx ON show_clips (clip_id);

CREATE TABLE show_participants (
    show_id   uuid NOT NULL REFERENCES shows (id) ON DELETE CASCADE,
    user_id   uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    joined_at timestamptz NOT NULL DEFAULT now(),
    -- Clicked "I'm ready, sound on". Information for the host, not a gate (decision 31).
    ready     boolean NOT NULL DEFAULT false,
    PRIMARY KEY (show_id, user_id)
);

-- One vote per person per category; never for your own clip (checked in core::shows).
CREATE TABLE show_votes (
    show_id    uuid NOT NULL REFERENCES shows (id) ON DELETE CASCADE,
    voter_id   uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    category   text NOT NULL CHECK (category IN ('clip', 'fail')),
    clip_id    uuid NOT NULL REFERENCES clips (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (show_id, voter_id, category)
);

-- Every tap in the show's reaction dock, with the moment in the clip, for the replay.
-- 🍌 makes the clip a fail contender (decision 30); the other six also turn on the
-- tapper's reaction on the clip page (decision 32).
CREATE TABLE show_reactions (
    id         bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    show_id    uuid NOT NULL REFERENCES shows (id) ON DELETE CASCADE,
    clip_id    uuid NOT NULL REFERENCES clips (id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    emoji      text NOT NULL,
    at_ms      integer NOT NULL CHECK (at_ms >= 0),
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX show_reactions_show_idx ON show_reactions (show_id, clip_id, at_ms);
