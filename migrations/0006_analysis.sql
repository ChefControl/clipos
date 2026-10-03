-- Killfeed analysis results (phase 8). One row per clip: the latest analysis replaces the
-- previous one. In shadow mode nothing reads these into tags yet; they are checked
-- against real clips first.
CREATE TABLE analysis_results (
    clip_id          uuid PRIMARY KEY REFERENCES clips (id) ON DELETE CASCADE,
    -- Which models and reader produced it, e.g. `rows=killfeed-rows/v1 icons=killfeed-icons/v1`.
    analyser_version text NOT NULL,
    -- Every kill the tracker followed: time, owner, weapon, modifiers.
    raw              jsonb NOT NULL,
    -- The recording player's kills summarised: count, multi-kill, weapons, modifiers.
    stats            jsonb NOT NULL,
    -- A person's correction of `raw`, when one is made.
    corrected        jsonb,
    created_at       timestamptz NOT NULL DEFAULT now()
);
