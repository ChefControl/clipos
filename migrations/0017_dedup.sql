-- A file that's here already isn't kept twice (decision 55). Every clip's original and
-- playback file gets a fingerprint (core::dedup): its size, a hash of three 1 MiB samples
-- and a hash of all of it. Only the worker writes them, from the files themselves; the
-- browser's hashes are only ever compared with them.
CREATE TABLE clip_fingerprints (
    clip_id      uuid NOT NULL REFERENCES clips (id) ON DELETE CASCADE,
    file         text NOT NULL CHECK (file IN ('original', 'playback')),
    bytes        bigint NOT NULL CHECK (bytes > 0),
    sample_hash  bytea NOT NULL CHECK (length(sample_hash) = 32),
    content_hash bytea NOT NULL CHECK (length(content_hash) = 32),
    PRIMARY KEY (clip_id, file)
);

-- The browser's first question: a file of this size with these samples?
CREATE INDEX clip_fingerprints_sample_idx ON clip_fingerprints (bytes, sample_hash);
-- The worker's and a restore's: this exact file?
CREATE INDEX clip_fingerprints_content_idx ON clip_fingerprints (content_hash);

-- A clip the worker refused because the same file is here already as this one.
ALTER TABLE clips ADD COLUMN duplicate_of uuid REFERENCES clips (id) ON DELETE SET NULL;

-- Clips from before: one `fingerprint` job each, at core::jobs::PRIORITY_LOW so uploads go
-- first, starting after the deploy's new worker (which knows the job) is up. Clips still
-- processing are fingerprinted by their transcode.
INSERT INTO jobs (kind, payload, run_after, priority)
SELECT 'fingerprint', jsonb_build_object('clipId', id), now() + interval '10 minutes', -5
  FROM clips
 WHERE status = 'ready';
