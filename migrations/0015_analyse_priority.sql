-- Killfeed analyses run at core::jobs::PRIORITY_LOW, after every normal job: a bulk
-- upload's analyses (minutes each) used to share the transcodes' priority, so a new
-- upload's transcode waited behind them. Lowers the ones already queued (or running, and
-- so maybe retried). Safe to run more than once.
UPDATE jobs SET priority = -5
 WHERE kind = 'analyse' AND status IN ('queued', 'running') AND priority = 0;
