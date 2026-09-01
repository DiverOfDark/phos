-- Typed parameters: what a person set a workflow's non-text inputs to.
--
-- `text_overrides` is a string→string map, which is why only prompt boxes were
-- ever offered: a seed put through it comes back as "156680208700286", and a
-- graph submitted with a string where ComfyUI declared an INT is refused. This
-- column is the same idea with the types kept — a JSON object keyed
-- "<node_id>.<field_name>", each value as its own JSON type:
--
--   {"3.seed": 4242, "3.cfg": 6.5, "3.sampler_name": "dpmpp_2m",
--    "4.ckpt_name": "sd_xl_base_1.0.safetensors", "12.add_noise": true}
--
-- Nothing is migrated out of `text_overrides`. It keeps carrying prompts and
-- the `role` directives the loader binder reads, and a task queued before this
-- column exists carries NULL here and runs exactly as it did.
--
-- Every value is resolved before the row is written — a random seed is drawn at
-- queue time, not at dispatch — so a run is reproducible from its row alone.
--
-- Fan-out (one request, four seeds) writes four rows, each with its own resolved
-- map. When runs arrive (FR5) they group these rows by adding a `run_id`
-- column; nothing written here has to change for that.
ALTER TABLE enhancement_tasks ADD COLUMN parameters TEXT;

-- A preset that cannot pin a seed or a step count is half a preset. Same shape,
-- same key format. NULL is a preset saved before this migration: prompts only.
ALTER TABLE workflow_presets ADD COLUMN parameters TEXT;
