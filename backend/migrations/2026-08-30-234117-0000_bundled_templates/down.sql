-- The seeded workflows and lines stay: they are ordinary rows in the ordinary
-- tables, and a downgrade that deleted a person's line because Phos had once
-- written it would be the worst of both worlds. Only the bookkeeping goes, so
-- an upgrade back re-seeds a second copy of anything still present. The `_phos`
-- markers survive in the graphs, which is what makes that recoverable by hand.
DROP TABLE IF EXISTS bundled_templates;
