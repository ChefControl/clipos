-- A hold now ends 7 days after the upload, however often "Save it for the show" is set
-- (decision 29; `clips::hold`). Holds set before this ran counted from when the switch was
-- pressed, so pressing it again kept a clip hidden for good: bring them back in line.
UPDATE clips SET hold_until = created_at + interval '7 days'
 WHERE hold_until > created_at + interval '7 days';
