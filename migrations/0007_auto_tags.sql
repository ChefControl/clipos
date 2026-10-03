-- Auto tags (phase 8) for clips analysed before the worker started writing them: the
-- recording player's multi-kill, and one tag per modifier they killed with. Same rules as
-- `clipos_core::analysis::auto_tags`; later analyses write their own.
CREATE TEMP TABLE wanted_auto_tags ON COMMIT DROP AS
SELECT clip_id, lower(stats->>'multi_kill') AS name
  FROM analysis_results
 WHERE stats->>'multi_kill' IS NOT NULL
UNION
SELECT a.clip_id, m.tag
  FROM analysis_results a
  JOIN (VALUES ('headshot', 'hs'), ('wallbang', 'wallbang'), ('through_smoke', 'smoke-kill'),
               ('noscope', 'noscope'), ('blind', 'blind-kill'), ('in_air', 'air-kill'))
       AS m (modifier, tag)
    ON coalesce((a.stats->'modifiers'->>m.modifier)::int, 0) > 0;

INSERT INTO tags (name) SELECT DISTINCT name FROM wanted_auto_tags ON CONFLICT (name) DO NOTHING;

INSERT INTO clip_tags (clip_id, tag_id, source)
SELECT w.clip_id, t.id, 'auto'
  FROM wanted_auto_tags w JOIN tags t ON t.name = w.name
    ON CONFLICT DO NOTHING;
