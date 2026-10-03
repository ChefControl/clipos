-- "Save it for the show" (docs/PLAN.md, redesign, S3; decision 29): a held clip is hidden
-- from everyone but its uploader until it plays in a show, the uploader posts it, or this
-- time passes (7 days after upload). NULL: not held.
ALTER TABLE clips ADD COLUMN hold_until timestamptz;
