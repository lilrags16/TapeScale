-- Destructive, laboratory-only Bacula btape qualification runs.
--
-- A passing run is evidence about this exact drive, firmware, device path, and
-- operator-designated scratch cartridge. It never promotes the product engine
-- or drive to physically_qualified; the broader G0 decision remains separate.

CREATE TABLE qualification_runs (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    drive_id uuid NOT NULL REFERENCES drives(id) ON DELETE RESTRICT,
    cartridge_id uuid NOT NULL REFERENCES cartridges(id) ON DELETE RESTRICT,
    expected_stable_id text NOT NULL,
    expected_path text NOT NULL CHECK (expected_path ~ '^/dev/nst[0-9]+$'),
    expected_vendor text,
    expected_model text,
    expected_serial text,
    expected_wwn text,
    expected_firmware text,
    expected_cartridge_label text NOT NULL CHECK (expected_cartridge_label <> ''),
    expected_media_generation smallint CHECK (expected_media_generation BETWEEN 1 AND 10),
    expected_media_format text,
    engine_name text NOT NULL DEFAULT 'Bacula Community' CHECK (engine_name = 'Bacula Community'),
    engine_version text NOT NULL DEFAULT '15.0.3' CHECK (engine_version = '15.0.3'),
    test_kind text NOT NULL DEFAULT 'btape_test' CHECK (test_kind = 'btape_test'),
    scratch_media_confirmed boolean NOT NULL CHECK (scratch_media_confirmed),
    destructive_overwrite_confirmed boolean NOT NULL CHECK (destructive_overwrite_confirmed),
    state text NOT NULL DEFAULT 'queued'
        CHECK (state IN ('queued', 'running', 'passed', 'failed', 'blocked', 'interrupted')),
    requested_by_label text NOT NULL,
    correlation_id uuid NOT NULL,
    lease_id uuid REFERENCES resource_leases(id) ON DELETE RESTRICT,
    fencing_token bigint,
    attempt integer NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    result jsonb NOT NULL DEFAULT '{}' CHECK (octet_length(result::text) <= 1048576),
    created_at timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz,
    finished_at timestamptz,
    CHECK ((lease_id IS NULL) = (fencing_token IS NULL)),
    CHECK (
        (state = 'queued' AND started_at IS NULL AND finished_at IS NULL) OR
        (state = 'running' AND started_at IS NOT NULL AND finished_at IS NULL) OR
        (state IN ('passed', 'failed', 'blocked', 'interrupted')
            AND started_at IS NOT NULL AND finished_at IS NOT NULL)
    )
);

CREATE UNIQUE INDEX qualification_runs_one_active_per_drive
    ON qualification_runs(drive_id) WHERE state IN ('queued', 'running');

CREATE UNIQUE INDEX qualification_runs_one_active_per_cartridge
    ON qualification_runs(cartridge_id) WHERE state IN ('queued', 'running');

CREATE INDEX qualification_runs_recent ON qualification_runs(created_at DESC);

CREATE TABLE qualification_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    qualification_run_id uuid NOT NULL REFERENCES qualification_runs(id) ON DELETE RESTRICT,
    event_type text NOT NULL CHECK (event_type IN (
        'queued', 'started', 'passed', 'failed', 'blocked', 'interrupted'
    )),
    evidence jsonb NOT NULL DEFAULT '{}' CHECK (octet_length(evidence::text) <= 1048576),
    correlation_id uuid NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX qualification_events_run_time
    ON qualification_events(qualification_run_id, occurred_at);

CREATE TRIGGER qualification_events_append_only
    BEFORE UPDATE OR DELETE ON qualification_events
    FOR EACH ROW EXECUTE FUNCTION tapescale_reject_history_mutation();
