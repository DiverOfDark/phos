-- A rating on a file, which is what the Takes lane's `1`–`5` keys write.
--
-- Curation is the point of the farm: generation is cheap and deciding is not,
-- so the number somebody put on a take has to outlive the ten seconds they
-- spent looking at it. A rating that lives in a component's `ref` is a lie told
-- to a person reviewing two hundred takes in ten minutes.
--
-- On `files` rather than on `enhancement_tasks`, for the same reason
-- `is_original` is: a rating is about the picture, and the picture keeps
-- meaning something after the task that made it has been swept. The cost is
-- that a describe stage's take — a sentence, with no file anywhere — cannot
-- carry one, which is the right answer: there is nothing there to rate one to
-- five.
--
-- Nullable, because "not rated" and "rated zero" are different answers and the
-- lane draws them differently. No CHECK constraint: the API clamps to 1–5, and
-- a scale that has to be migrated to widen is a scale nobody widens.
ALTER TABLE files ADD COLUMN rating INTEGER;

-- The lane sorts and filters on it, over a library where nearly every file is a
-- photograph that will never have one. Partial, so the index is the size of the
-- ratings rather than the size of the library.
CREATE INDEX idx_files_rating ON files (rating) WHERE rating IS NOT NULL;
