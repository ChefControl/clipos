-- Uploaded clips. The original lives in Blob Storage (`originals/{id}/{file}`); the
-- worker writes a browser-friendly MP4 (`playback/{id}.mp4`) and a poster.
CREATE TABLE clips (
    id                uuid PRIMARY KEY,
    owner_id          uuid NOT NULL REFERENCES users (id),
    game_id           text NOT NULL DEFAULT 'cs2' REFERENCES games (id),
    title             text NOT NULL CHECK (char_length(title) BETWEEN 1 AND 100),
    description       text NOT NULL DEFAULT '' CHECK (char_length(description) <= 2000),
    map               text CHECK (char_length(map) BETWEEN 1 AND 40),
    -- Recorded from the uploader's own point of view (killfeed analysis needs this).
    my_pov            boolean NOT NULL DEFAULT true,
    status            text NOT NULL DEFAULT 'uploading'
                      CHECK (status IN ('uploading', 'processing', 'ready', 'failed')),
    original_blob     text NOT NULL,
    original_filename text NOT NULL,
    original_bytes    bigint NOT NULL CHECK (original_bytes > 0),
    playback_blob     text,
    poster_blob       text,
    duration_ms       integer,
    width             integer,
    height            integer,
    fps               real,
    -- ffprobe summary of the original (codecs, bitrate, …).
    metadata          jsonb NOT NULL DEFAULT '{}'::jsonb,
    -- Shown to the uploader when status = 'failed'.
    error             text,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now(),
    deleted_at        timestamptz
);

CREATE INDEX clips_feed_idx ON clips (created_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX clips_owner_idx ON clips (owner_id, created_at DESC);
CREATE INDEX clips_uploading_idx ON clips (created_at) WHERE status = 'uploading';
