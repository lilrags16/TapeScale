-- Durable, read-only replay of one previously passed G0.2 recovery point.
--
-- This remains a laboratory path.  It is deliberately restricted to the
-- pinned single-volume, unencrypted, full-file Bacula format already produced
-- by a passed G0.2 run.  A rehearsal may read that exact cartridge and write
-- only into an isolated restore destination; it never creates a recovery
-- point and it grants no broader production or hardware qualification.

CREATE TABLE restore_rehearsals (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    restore_plan_id uuid NOT NULL REFERENCES restore_plans(id) ON DELETE RESTRICT,
    job_id uuid NOT NULL UNIQUE REFERENCES jobs(id) ON DELETE RESTRICT,
    job_run_id uuid NOT NULL UNIQUE REFERENCES job_runs(id) ON DELETE RESTRICT,
    recovery_point_id uuid NOT NULL REFERENCES recovery_points(id) ON DELETE RESTRICT,
    original_data_run_id uuid NOT NULL REFERENCES single_tape_data_runs(id) ON DELETE RESTRICT,
    drive_id uuid NOT NULL REFERENCES drives(id) ON DELETE RESTRICT,
    cartridge_id uuid NOT NULL REFERENCES cartridges(id) ON DELETE RESTRICT,

    expected_stable_id text NOT NULL,
    expected_path text NOT NULL CHECK (expected_path ~ '^/dev/nst[0-9]+$'),
    expected_vendor text,
    expected_model text,
    expected_serial text,
    expected_wwn text,
    expected_firmware text,
    expected_volume_label text NOT NULL CHECK (expected_volume_label <> ''),
    expected_media_generation smallint CHECK (expected_media_generation BETWEEN 1 AND 10),
    expected_media_format text,
    expected_on_tape_identity text NOT NULL CHECK (expected_on_tape_identity <> ''),

    backup_job_id bigint NOT NULL CHECK (backup_job_id > 0),
    expected_engine_media_id bigint NOT NULL CHECK (expected_engine_media_id > 0),
    expected_engine_record_id text NOT NULL CHECK (expected_engine_record_id <> ''),
    source_manifest_artifact_id bigint NOT NULL
        REFERENCES single_tape_data_artifacts(id) ON DELETE RESTRICT,
    source_manifest_storage_ref text NOT NULL
        CHECK (source_manifest_storage_ref <> '' AND source_manifest_storage_ref !~ '[[:cntrl:]]'),
    source_manifest_sha256 text NOT NULL CHECK (source_manifest_sha256 ~ '^[0-9a-f]{64}$'),
    source_manifest_digest text NOT NULL CHECK (source_manifest_digest ~ '^[0-9a-f]{64}$'),
    source_manifest_size_bytes bigint NOT NULL CHECK (source_manifest_size_bytes > 0),
    source_mount_path text NOT NULL
        CHECK (source_mount_path LIKE '/%' AND source_mount_path !~ '[[:cntrl:]]'),
    logical_root text NOT NULL
        CHECK (logical_root LIKE '/%' AND logical_root !~ '[[:cntrl:]]'),
    destination_subpath text NOT NULL
        CHECK (destination_subpath <> '' AND octet_length(destination_subpath) <= 512
               AND destination_subpath !~ '(^|/)\.\.?(/|$)'
               AND destination_subpath !~ '^/' AND destination_subpath !~ '/$'
               AND destination_subpath !~ '//' AND destination_subpath !~ '[[:cntrl:]]'),
    estimated_required_bytes bigint NOT NULL CHECK (estimated_required_bytes >= 0),
    available_bytes bigint NOT NULL CHECK (available_bytes >= 0),
    required_free_reserve_bytes bigint NOT NULL CHECK (required_free_reserve_bytes >= 0),

    engine_name text NOT NULL DEFAULT 'Bacula Community'
        CHECK (engine_name = 'Bacula Community'),
    engine_version text NOT NULL DEFAULT '15.0.3'
        CHECK (engine_version = '15.0.3'),
    adapter_version text NOT NULL DEFAULT 'g0.2-lab-v1'
        CHECK (adapter_version = 'g0.2-lab-v1'),
    format_version text NOT NULL DEFAULT 'bacula-community-15.0.3-lab-v1'
        CHECK (format_version = 'bacula-community-15.0.3-lab-v1'),
    preview_hash text NOT NULL CHECK (preview_hash ~ '^[0-9a-f]{64}$'),
    production_eligible boolean NOT NULL DEFAULT false CHECK (NOT production_eligible),

    state text NOT NULL DEFAULT 'queued'
        CHECK (state IN (
            'queued', 'running', 'passed', 'failed', 'blocked',
            'interrupted', 'cancelled'
        )),
    phase text NOT NULL DEFAULT 'queued'
        CHECK (phase IN (
            'queued', 'preparing', 'restoring', 'comparing',
            'passed', 'failed', 'blocked', 'interrupted', 'cancelled'
        )),
    requested_by_label text NOT NULL
        CHECK (requested_by_label <> '' AND octet_length(requested_by_label) <= 256
               AND requested_by_label !~ '[[:cntrl:]]'),
    correlation_id uuid NOT NULL,
    cancel_requested_at timestamptz,
    cancel_requested_by_label text,
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
    CHECK ((cancel_requested_at IS NULL) = (cancel_requested_by_label IS NULL)),
    CHECK (cancel_requested_by_label IS NULL OR
           (cancel_requested_by_label <> ''
            AND octet_length(cancel_requested_by_label) <= 256
            AND cancel_requested_by_label !~ '[[:cntrl:]]')),
    CHECK (available_bytes - estimated_required_bytes >= required_free_reserve_bytes),
    CHECK (
        (state = 'queued' AND phase = 'queued' AND started_at IS NULL AND finished_at IS NULL) OR
        (state = 'running' AND phase IN ('preparing', 'restoring', 'comparing')
            AND started_at IS NOT NULL AND finished_at IS NULL) OR
        (state IN ('passed', 'failed', 'blocked', 'interrupted', 'cancelled')
            AND phase = state AND started_at IS NOT NULL AND finished_at IS NOT NULL)
    )
);

CREATE UNIQUE INDEX restore_rehearsals_one_active_per_plan
    ON restore_rehearsals(restore_plan_id) WHERE state IN ('queued', 'running');

CREATE UNIQUE INDEX restore_rehearsals_one_active_per_drive
    ON restore_rehearsals(drive_id) WHERE state IN ('queued', 'running');

CREATE UNIQUE INDEX restore_rehearsals_one_active_per_cartridge
    ON restore_rehearsals(cartridge_id) WHERE state IN ('queued', 'running');

CREATE INDEX restore_rehearsals_recent
    ON restore_rehearsals(created_at DESC);

CREATE TABLE restore_rehearsal_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    restore_rehearsal_id uuid NOT NULL REFERENCES restore_rehearsals(id) ON DELETE RESTRICT,
    event_type text NOT NULL CHECK (event_type IN (
        'queued', 'started', 'media_observed', 'restoring',
        'engine_dispatched', 'engine_completed', 'comparing',
        'cancel_requested', 'passed', 'failed', 'blocked',
        'interrupted', 'cancelled', 'fenced_out'
    )),
    evidence jsonb NOT NULL DEFAULT '{}' CHECK (octet_length(evidence::text) <= 1048576),
    correlation_id uuid NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX restore_rehearsal_events_run_time
    ON restore_rehearsal_events(restore_rehearsal_id, occurred_at, id);

CREATE TABLE restore_rehearsal_artifacts (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    restore_rehearsal_id uuid NOT NULL REFERENCES restore_rehearsals(id) ON DELETE RESTRICT,
    artifact_kind text NOT NULL CHECK (artifact_kind IN (
        'engine_restore', 'restored_manifest', 'comparison', 'evidence_report'
    )),
    storage_ref text NOT NULL
        CHECK (storage_ref <> '' AND storage_ref !~ '[[:cntrl:]]'),
    sha256 text NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    size_bytes bigint NOT NULL CHECK (size_bytes >= 0),
    metadata jsonb NOT NULL DEFAULT '{}' CHECK (octet_length(metadata::text) <= 1048576),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (restore_rehearsal_id, artifact_kind)
);

CREATE INDEX restore_rehearsal_artifacts_run_time
    ON restore_rehearsal_artifacts(restore_rehearsal_id, created_at, id);

CREATE TRIGGER restore_rehearsal_events_append_only
    BEFORE UPDATE OR DELETE ON restore_rehearsal_events
    FOR EACH ROW EXECUTE FUNCTION tapescale_reject_history_mutation();

CREATE TRIGGER restore_rehearsal_artifacts_append_only
    BEFORE UPDATE OR DELETE ON restore_rehearsal_artifacts
    FOR EACH ROW EXECUTE FUNCTION tapescale_reject_history_mutation();
