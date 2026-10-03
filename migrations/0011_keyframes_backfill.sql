-- Keyframes every 2 s for the show's quick seeks (S5c, decision 35): one `keyframes` job
-- per ready clip, which re-encodes it only if its keyframes are more than 4 s apart. They
-- start 10 minutes from now, after the deploy's new worker (which knows the job) is up.
INSERT INTO jobs (kind, payload, run_after)
SELECT 'keyframes', jsonb_build_object('clipId', id), now() + interval '10 minutes'
  FROM clips
 WHERE status = 'ready' AND deleted_at IS NULL;
