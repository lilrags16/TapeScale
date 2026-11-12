-- G0.2 laboratory-only, single-volume full-file backup and physical restore exercise.
--
-- These records intentionally do not establish production qualification. A passed
-- run means one exact immutable source snapshot was written through Bacula to one
-- exact cartridge, restored from physical media, and compared with retained
-- manifests. General scheduling, incrementals, spanning, native ZFS, encryption,
-- and automatic reuse remain outside this gate.

ALTER TABLE job_runs DROP CONSTRAINT job_runs_state_check;
ALTER TABLE job_runs ADD CONSTRAINT job_runs_state_check CHECK (state IN (
    'queued', 'preparing', 'waiting_for_media', 'writing', 'written', 'verifying',
    'restoring', 'comparing', 'verified', 'blocked', 'interrupted', 'failed',
    'cancelled', 'completed'
));

CREATE TABLE single_tape_data_profiles (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    job_id uuid NOT NULL UNIQUE REFERENCES jobs(id) ON DELETE RESTRICT,
    source_id uuid NOT NULL REFERENCES sources(id) ON DELETE RESTRICT,
    source_snapshot_id uuid NOT NULL REFERENCES snapshots(id) ON DELETE RESTRICT,
    name text NOT NULL UNIQUE CHECK (name <> '' AND char_length(name) <= 128),
    source_subpath text NOT NULL
        CHECK (source_subpath <> '' AND octet_length(source_subpath) <= 512
               AND source_subpath !~ '(^|/)\.\.?(/|$)'
               AND source_subpath !~ '^/' AND source_subpath !~ '/$'
               AND source_subpath !~ '//' AND source_subpath !~ '[[:cntrl:]]'),
    restore_subpath text NOT NULL
        CHECK (restore_subpath <> '' AND octet_length(restore_subpath) <= 512
               AND restore_subpath !~ '(^|/)\.\.?(/|$)'
               AND restore_subpath !~ '^/' AND restore_subpath !~ '/$'
               AND restore_subpath !~ '//' AND restore_subpath !~ '[[:cntrl:]]'),
    source_mount_path text NOT NULL
        CHECK (source_mount_path LIKE '/%' AND source_mount_path !~ '[[:cntrl:]]'),
    restore_root_path text NOT NULL
        CHECK (restore_root_path LIKE '/%' AND restore_root_path !~ '[[:cntrl:]]'),
    logical_root text NOT NULL
        CHECK (logical_root LIKE '/%' AND logical_root !~ '[[:cntrl:]]'),
    engine_name text NOT NULL DEFAULT 'Bacula Community'
        CHECK (engine_name = 'Bacula Community'),
    engine_version text NOT NULL DEFAULT '15.0.3'
        CHECK (engine_version = '15.0.3'),
    adapter_version text NOT NULL DEFAULT 'g0.2-lab-v1'
        CHECK (adapter_version = 'g0.2-lab-v1'),
    format_version text NOT NULL DEFAULT 'bacula-community-15.0.3-lab-v1'
        CHECK (format_version = 'bacula-community-15.0.3-lab-v1'),
    immutable_snapshot_confirmed boolean NOT NULL CHECK (immutable_snapshot_confirmed),
    enabled boolean NOT NULL DEFAULT true,
    production_eligible boolean NOT NULL DEFAULT false CHECK (NOT production_eligible),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (source_mount_path <> restore_root_path)
);

CREATE TABLE single_tape_data_runs (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    profile_id uuid NOT NULL REFERENCES single_tape_data_profiles(id) ON DELETE RESTRICT,
    job_run_id uuid NOT NULL UNIQUE REFERENCES job_runs(id) ON DELETE RESTRICT,
    source_snapshot_id uuid NOT NULL REFERENCES snapshots(id) ON DELETE RESTRICT,
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
    source_mount_path text NOT NULL
        CHECK (source_mount_path LIKE '/%' AND source_mount_path !~ '[[:cntrl:]]'),
    restore_root_path text NOT NULL
        CHECK (restore_root_path LIKE '/%' AND restore_root_path !~ '[[:cntrl:]]'),
    logical_root text NOT NULL
        CHECK (logical_root LIKE '/%' AND logical_root !~ '[[:cntrl:]]'),
    engine_name text NOT NULL CHECK (engine_name = 'Bacula Community'),
    engine_version text NOT NULL CHECK (engine_version = '15.0.3'),
    adapter_version text NOT NULL CHECK (adapter_version = 'g0.2-lab-v1'),
    format_version text NOT NULL CHECK (format_version = 'bacula-community-15.0.3-lab-v1'),
    scratch_media_confirmed boolean NOT NULL CHECK (scratch_media_confirmed),
    destructive_overwrite_confirmed boolean NOT NULL CHECK (destructive_overwrite_confirmed),
    encryption_deviation_confirmed boolean NOT NULL CHECK (encryption_deviation_confirmed),
    media_write_started boolean NOT NULL DEFAULT false,
    media_write_started_at timestamptz,
    state text NOT NULL DEFAULT 'queued'
        CHECK (state IN ('queued', 'running', 'passed', 'failed', 'blocked', 'interrupted', 'cancelled')),
    requested_by_label text NOT NULL,
    correlation_id uuid NOT NULL,
    drive_lease_id uuid REFERENCES resource_leases(id) ON DELETE RESTRICT,
    drive_fencing_token bigint,
    media_lease_id uuid REFERENCES resource_leases(id) ON DELETE RESTRICT,
    media_fencing_token bigint,
    attempt integer NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    result jsonb NOT NULL DEFAULT '{}' CHECK (octet_length(result::text) <= 1048576),
    created_at timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz,
    finished_at timestamptz,
    CHECK ((drive_lease_id IS NULL) = (drive_fencing_token IS NULL)),
    CHECK ((media_lease_id IS NULL) = (media_fencing_token IS NULL)),
    CHECK (media_write_started = (media_write_started_at IS NOT NULL)),
    CHECK (
        (state = 'queued' AND started_at IS NULL AND finished_at IS NULL) OR
        (state = 'running' AND started_at IS NOT NULL AND finished_at IS NULL) OR
        (state IN ('passed', 'failed', 'blocked', 'interrupted', 'cancelled')
            AND started_at IS NOT NULL AND finished_at IS NOT NULL)
    )
);

CREATE UNIQUE INDEX single_tape_data_one_active_per_profile
    ON single_tape_data_runs(profile_id) WHERE state IN ('queued', 'running');

CREATE UNIQUE INDEX single_tape_data_one_active_per_drive
    ON single_tape_data_runs(drive_id) WHERE state IN ('queued', 'running');

CREATE UNIQUE INDEX single_tape_data_one_active_per_cartridge
    ON single_tape_data_runs(cartridge_id) WHERE state IN ('queued', 'running');

CREATE INDEX single_tape_data_runs_recent
    ON single_tape_data_runs(created_at DESC);

CREATE TABLE single_tape_data_artifacts (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    single_tape_data_run_id uuid NOT NULL
        REFERENCES single_tape_data_runs(id) ON DELETE RESTRICT,
    artifact_kind text NOT NULL CHECK (artifact_kind IN (
        'source_manifest', 'engine_write', 'restored_manifest',
        'comparison', 'evidence_report'
    )),
    storage_ref text NOT NULL
        CHECK (storage_ref <> '' AND storage_ref !~ '[[:cntrl:]]'),
    sha256 text NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    size_bytes bigint NOT NULL CHECK (size_bytes >= 0),
    metadata jsonb NOT NULL DEFAULT '{}'
        CHECK (octet_length(metadata::text) <= 1048576),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (single_tape_data_run_id, artifact_kind)
);

CREATE TABLE single_tape_data_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    single_tape_data_run_id uuid NOT NULL
        REFERENCES single_tape_data_runs(id) ON DELETE RESTRICT,
    event_type text NOT NULL CHECK (event_type IN (
        'queued', 'started', 'preparing', 'writing', 'written', 'verifying',
        'restoring', 'comparing',
        'media_write_started', 'engine_dispatched', 'fenced_out',
        'passed', 'failed', 'blocked', 'interrupted', 'cancelled'
    )),
    evidence jsonb NOT NULL DEFAULT '{}'
        CHECK (octet_length(evidence::text) <= 1048576),
    correlation_id uuid NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX single_tape_data_events_run_time
    ON single_tape_data_events(single_tape_data_run_id, occurred_at, id);

CREATE TRIGGER single_tape_data_artifacts_append_only
    BEFORE UPDATE OR DELETE ON single_tape_data_artifacts
    FOR EACH ROW EXECUTE FUNCTION tapescale_reject_history_mutation();

CREATE TRIGGER single_tape_data_events_append_only
    BEFORE UPDATE OR DELETE ON single_tape_data_events
    FOR EACH ROW EXECUTE FUNCTION tapescale_reject_history_mutation();
