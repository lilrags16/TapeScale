-- Physical discovery must not invent an LTO generation from a model string.
-- A detected drive remains useful for identity and access diagnostics while its
-- generation is unknown and while physical qualification is still pending.
ALTER TABLE drives ALTER COLUMN generation DROP NOT NULL;
