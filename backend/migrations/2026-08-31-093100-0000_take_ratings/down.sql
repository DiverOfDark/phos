-- Every rating a person gave is lost, and there is nowhere else it was written
-- down. That is the honest cost of dropping the column, and it is worth knowing
-- before running this on a library somebody has curated.
DROP INDEX IF EXISTS idx_files_rating;
ALTER TABLE files DROP COLUMN rating;
