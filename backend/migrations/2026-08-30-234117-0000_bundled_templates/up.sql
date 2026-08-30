-- Bundled templates: the five lines a fresh install already has.
--
-- A template is a line, the workflow graphs its stages run, and a manifest of
-- what has to be installed for it to work. The graphs and the line go into the
-- tables that already exist -- `comfyui_workflows`, `production_lines`,
-- `line_stages` -- because a seeded line must be exactly as editable, runnable
-- and deletable as one somebody drew by hand. There is no second kind of line.
--
-- This table is the bookkeeping that makes an *upgrade* possible: which
-- templates this library has seen, at what version, and which rows they wrote.
--
--   template_key      stable across releases; the identity an upgrade matches
--   template_version  the version last seeded, so a newer build knows to sync
--   line_id           the line that was created. NULL once the user deletes it
--   workflow_ids      {"<workflow key>": "<comfyui_workflows.id>"} -- so an
--                     upgrade finds the rows it wrote without scanning, and so
--                     the console can still say "you edited this one" after the
--                     marker has been dropped
--
-- What it deliberately does *not* hold is the update decision. That lives in
-- the `_phos` block inside each seeded workflow's own JSON -- key, workflow
-- key, version and a hash of the graph exactly as shipped -- so it travels with
-- the graph when it is exported, and so no upgrade depends on a database row
-- agreeing with a file. Rehash on upgrade: matching means untouched, and the
-- new version is written over it; differing means the user edited it, and the
-- marker is dropped so nothing ever touches it again.
--
-- A row here is the claim on a key. It is why an abandoned template is not
-- re-seeded as a duplicate on the next restart, and why deleting a template in
-- the console (which deletes this row) is what makes "Install" offer it again.
CREATE TABLE bundled_templates (
    template_key TEXT PRIMARY KEY NOT NULL,
    template_version INTEGER NOT NULL,
    line_id TEXT REFERENCES production_lines(id),
    workflow_ids TEXT NOT NULL DEFAULT '{}',
    installed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
