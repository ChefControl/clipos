-- A show plays at most 10 clips (decision 56). The ones that don't fit wait under the
-- lineup like dropped clips, but the host didn't drop them, so they don't count towards
-- "dropped in two shows stops coming back" (decision 28). Safe to run more than once.
ALTER TABLE show_clips ADD COLUMN IF NOT EXISTS spare boolean NOT NULL DEFAULT false;
