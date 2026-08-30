-- Contracts are derived, so dropping the column loses only the corrections a
-- person typed — the derivation runs again on the next start. Nothing else
-- reads the column, and no run depends on it.
ALTER TABLE comfyui_workflows DROP COLUMN contract_json;
