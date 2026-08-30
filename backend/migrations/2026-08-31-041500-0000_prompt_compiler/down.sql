-- Both columns are derived and both are re-derivable: dropping them loses the
-- cached descriptions, so the next run of a describe stage pays for the GPU
-- round-trip again, and loses the text a finished describe task published.
-- No file, no run and no line depends on either.
ALTER TABLE shots DROP COLUMN analysis_json;
ALTER TABLE enhancement_tasks DROP COLUMN text_output;
