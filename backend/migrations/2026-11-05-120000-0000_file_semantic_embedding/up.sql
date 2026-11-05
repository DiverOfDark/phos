-- SigLIP 2 vector of each file (videos: first frame), and the model that
-- produced it, so a model change marks every vector stale. A shot's vector is
-- its main file's, so changing the main photo needs no bookkeeping.
ALTER TABLE files ADD COLUMN semantic_embedding BLOB;
ALTER TABLE files ADD COLUMN semantic_model TEXT;
