-- The show hub's live state (S5): which clip, playing or paused, where, and from when,
-- saved on every change so a deploy restart resumes the show where it was.
ALTER TABLE shows ADD COLUMN live_state jsonb;
