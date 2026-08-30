-- Every exposed key becomes a key the line simply does not set, which is what
-- the column replaced: the stage runs its workflow author's own value for it.
-- A run in flight loses the answers its later stages had not read yet, and
-- those stages run the line's own values instead.
ALTER TABLE runs DROP COLUMN stage_values;
ALTER TABLE line_stages DROP COLUMN exposed;
