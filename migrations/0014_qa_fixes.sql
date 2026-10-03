-- Fixes from the QA pass of 2026-10-03.

-- Reactions at the same moment could leave `reaction_count` off by some (social::react
-- recounted; it now counts up and down in the statement that adds or removes one). Bring
-- the counts back in line once.
UPDATE clips c SET reaction_count = r.n
  FROM (SELECT cl.id, (SELECT count(*) FROM reactions WHERE clip_id = cl.id)::int AS n
          FROM clips cl) r
 WHERE r.id = c.id AND c.reaction_count <> r.n;

-- The original's ETag when its upload was completed (clips::set_original_etag): the
-- upload SAS stays valid for a while after that, so the worker checks the file it reads
-- is still the one that was completed. NULL for clips from before.
ALTER TABLE clips ADD COLUMN original_etag text;

-- One clip per place in a lineup. Clips added at the same moment could share one; every
-- lineup change now locks the show row (core::shows), and this makes sure. Shows from
-- before are numbered again first, in the same order. DEFERRABLE: `set_lineup` renumbers
-- the whole lineup in one statement, which passes through duplicates on the way; a
-- deferrable constraint is checked at the end of the statement, not after every row.
UPDATE show_clips sc SET position = r.n
  FROM (SELECT show_id, clip_id,
               (row_number() OVER (PARTITION BY show_id ORDER BY position, clip_id))::int - 1 AS n
          FROM show_clips) r
 WHERE r.show_id = sc.show_id AND r.clip_id = sc.clip_id AND sc.position <> r.n;
ALTER TABLE show_clips ADD CONSTRAINT show_clips_position_key UNIQUE (show_id, position)
    DEFERRABLE INITIALLY IMMEDIATE;

-- A clip saved for the show that this show released by playing it. If the show is
-- abandoned, the clip is saved for the show again (shows::abandon, decision 41).
ALTER TABLE show_clips ADD COLUMN released_hold boolean NOT NULL DEFAULT false;
