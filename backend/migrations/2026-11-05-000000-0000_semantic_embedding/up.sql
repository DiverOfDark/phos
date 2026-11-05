-- SigLIP 2 vector of the shot's main photo, and the '<model>:<file id>' it was
-- computed from, so a new model or a new main photo marks it stale.
ALTER TABLE shots ADD COLUMN semantic_embedding BLOB;
ALTER TABLE shots ADD COLUMN semantic_source TEXT;
