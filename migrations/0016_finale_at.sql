-- When a show went to its finale (S7): the vote's two 20 s windows, fail of the night
-- then clip of the night, run from it, the same on every screen. A show already in its
-- finale votes from now. Safe to run more than once.
ALTER TABLE shows ADD COLUMN IF NOT EXISTS finale_at timestamptz;
UPDATE shows SET finale_at = now() WHERE status = 'finale' AND finale_at IS NULL;
