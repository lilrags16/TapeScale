-- Keep bounded backup media-readiness previews set based.  The latest exact
-- qualification lookup is always scoped to one physical drive/cartridge pair.
CREATE INDEX qualification_runs_pair_latest
    ON qualification_runs(drive_id, cartridge_id, created_at DESC, id DESC);
