-- Job priorities: `claim` takes the highest first (see core::jobs), so background work
-- such as 0011's keyframes backfill, queued for every clip at once, no longer holds up
-- new uploads. Safe to run more than once.
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS priority smallint NOT NULL DEFAULT 0;

DROP INDEX IF EXISTS jobs_claimable_idx;
CREATE INDEX IF NOT EXISTS jobs_claimable_priority_idx
    ON jobs (priority DESC, run_after) WHERE status = 'queued';

-- Keyframes jobs still waiting (or running, and so maybe retried) go to the background:
-- core::jobs::PRIORITY_BACKGROUND.
UPDATE jobs SET priority = -10
 WHERE kind = 'keyframes' AND status IN ('queued', 'running') AND priority <> -10;
