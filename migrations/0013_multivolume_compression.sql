-- Operational compression and manual multi-volume continuation.
--
-- The original physical-data tables retain their names for migration
-- compatibility, but an operational backup may now own an ordered set of
-- cartridges.  Recovery exercises remain single-volume until their separate
-- restore-comparison gate is expanded.

ALTER TABLE protection_policies
    -- Preserve the pre-upgrade single-volume behavior for existing policies.
    -- New TapeScale profiles explicitly persist the operator's (default-on)
    -- spanning choice rather than depending on this compatibility default.
    ADD COLUMN spanning_enabled boolean NOT NULL DEFAULT false,
    -- Old previews did not constrain this column.  NOT VALID keeps an upgrade
    -- from failing on a legacy unsupported value; PostgreSQL still enforces
    -- the constraint for every new or changed row, and the executor rejects
    -- unsupported legacy values before tape motion.
    ADD CONSTRAINT protection_policies_compression_mode_check
        CHECK (compression_mode IN ('auto', 'none', 'gzip', 'lzo')) NOT VALID;

ALTER TABLE single_tape_data_runs
    ADD COLUMN compression_mode text NOT NULL DEFAULT 'none'
        CHECK (compression_mode IN ('auto', 'none', 'gzip', 'lzo')),
    ADD COLUMN resolved_compression text NOT NULL DEFAULT 'none'
        CHECK (resolved_compression IN ('none', 'gzip', 'lzo')),
    ADD COLUMN spanning_enabled boolean NOT NULL DEFAULT false;

CREATE TABLE single_tape_data_run_volumes (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    single_tape_data_run_id uuid NOT NULL
        REFERENCES single_tape_data_runs(id) ON DELETE RESTRICT,
    volume_order integer NOT NULL CHECK (volume_order > 0),
    cartridge_id uuid NOT NULL REFERENCES cartridges(id) ON DELETE RESTRICT,
    -- Match the legacy scalar's non-empty contract so every schema-valid
    -- pre-0013 row can be backfilled. New operator input is bounded and
    -- control-character checked by the API/store path before it reaches SQL.
    expected_cartridge_label text NOT NULL CHECK (expected_cartridge_label <> ''),
    expected_media_generation smallint CHECK (expected_media_generation BETWEEN 1 AND 10),
    expected_media_format text,
    engine_volume_name text
        CHECK (engine_volume_name IS NULL OR
               (engine_volume_name <> '' AND octet_length(engine_volume_name) <= 127
                AND engine_volume_name ~ '^[A-Za-z0-9_.-]+$')),
    engine_media_id bigint CHECK (engine_media_id IS NULL OR engine_media_id > 0),
    state text NOT NULL DEFAULT 'authorized'
        CHECK (state IN (
            'authorized', 'write_started', 'written', 'verified',
            'write_unverified', 'failed'
        )),
    media_lease_id uuid REFERENCES resource_leases(id) ON DELETE RESTRICT,
    media_fencing_token bigint,
    media_write_started_at timestamptz,
    written_at timestamptz,
    verified_at timestamptz,
    encoded_bytes bigint CHECK (encoded_bytes IS NULL OR encoded_bytes >= 0),
    first_record text,
    last_record text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (single_tape_data_run_id, volume_order),
    UNIQUE (single_tape_data_run_id, cartridge_id),
    UNIQUE (single_tape_data_run_id, engine_volume_name),
    CHECK ((media_lease_id IS NULL) = (media_fencing_token IS NULL)),
    CHECK ((state = 'authorized' AND media_write_started_at IS NULL) OR
           (state <> 'authorized' AND media_write_started_at IS NOT NULL)),
    CHECK (written_at IS NULL OR media_write_started_at IS NOT NULL),
    CHECK (verified_at IS NULL OR written_at IS NOT NULL)
);

CREATE INDEX single_tape_data_run_volumes_cartridge_active
    ON single_tape_data_run_volumes(cartridge_id, single_tape_data_run_id);

-- Seed the legacy scalar assignment as volume one.  The scalar columns remain
-- a compatibility mirror for old clients and the single-volume recovery
-- exercise; ordered rows are authoritative for operational backups.
-- Legacy schema-valid terminal rows can be `passed` without ever crossing the
-- durable media-write boundary.  They remain an `authorized` historical
-- assignment here; only a passed row with that boundary may be backfilled as
-- a verified written volume.
INSERT INTO single_tape_data_run_volumes
    (single_tape_data_run_id, volume_order, cartridge_id,
     expected_cartridge_label, expected_media_generation,
     expected_media_format, engine_volume_name,
     media_lease_id, media_fencing_token, media_write_started_at,
     written_at, verified_at, state, created_at, updated_at)
SELECT id, 1, cartridge_id, expected_cartridge_label,
       expected_media_generation, expected_media_format,
       CASE
           WHEN state = 'passed' AND media_write_started
            AND octet_length(result->>'engine_volume_name') BETWEEN 1 AND 127
            AND result->>'engine_volume_name' ~ '^[A-Za-z0-9_.-]+$'
           THEN result->>'engine_volume_name'
       END,
       media_lease_id, media_fencing_token, media_write_started_at,
       CASE WHEN state = 'passed' AND media_write_started THEN finished_at END,
       CASE WHEN state = 'passed' AND media_write_started THEN finished_at END,
       CASE
           WHEN state = 'passed' AND media_write_started THEN 'verified'
           WHEN media_write_started THEN 'write_unverified'
           ELSE 'authorized'
       END,
       created_at, COALESCE(finished_at, created_at)
  FROM single_tape_data_runs;

CREATE TABLE single_tape_data_media_requests (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    single_tape_data_run_id uuid NOT NULL
        REFERENCES single_tape_data_runs(id) ON DELETE RESTRICT,
    request_sequence integer NOT NULL CHECK (request_sequence > 0),
    phase text NOT NULL CHECK (phase IN ('backup_write', 'full_readback', 'restore')),
    volume_order integer NOT NULL CHECK (volume_order > 0),
    expected_cartridge_id uuid REFERENCES cartridges(id) ON DELETE RESTRICT,
    expected_engine_volume_name text,
    state text NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'authorized', 'loaded', 'completed', 'cancelled')),
    authorized_cartridge_id uuid REFERENCES cartridges(id) ON DELETE RESTRICT,
    requested_at timestamptz NOT NULL DEFAULT now(),
    authorized_at timestamptz,
    loaded_at timestamptz,
    completed_at timestamptz,
    confirmation_evidence jsonb NOT NULL DEFAULT '{}'
        CHECK (jsonb_typeof(confirmation_evidence) = 'object'
               AND octet_length(confirmation_evidence::text) <= 1048576),
    UNIQUE (single_tape_data_run_id, request_sequence),
    CHECK (expected_engine_volume_name IS NULL OR
           (expected_engine_volume_name <> ''
            AND octet_length(expected_engine_volume_name) <= 127
            AND expected_engine_volume_name ~ '^[A-Za-z0-9_.-]+$')),
    CHECK ((state = 'pending' AND authorized_at IS NULL
            AND authorized_cartridge_id IS NULL) OR
           (state <> 'pending' AND state <> 'cancelled'
            AND authorized_at IS NOT NULL
            AND authorized_cartridge_id IS NOT NULL) OR
           state = 'cancelled'),
    CHECK (loaded_at IS NULL OR authorized_at IS NOT NULL),
    CHECK (completed_at IS NULL OR loaded_at IS NOT NULL)
);

CREATE UNIQUE INDEX single_tape_data_one_open_media_request
    ON single_tape_data_media_requests(single_tape_data_run_id)
    WHERE state IN ('pending', 'authorized', 'loaded');

ALTER TABLE single_tape_data_events
    DROP CONSTRAINT single_tape_data_events_event_type_check;
ALTER TABLE single_tape_data_events
    ADD CONSTRAINT single_tape_data_events_event_type_check CHECK (event_type IN (
        'queued', 'started', 'preparing', 'writing', 'written', 'verifying',
        'restoring', 'comparing', 'media_write_started', 'media_prepared',
        'media_required', 'media_authorized', 'media_loaded', 'media_full',
        'media_read', 'media_verified', 'engine_dispatched', 'fenced_out', 'cancel_requested',
        'passed', 'failed', 'blocked', 'interrupted', 'cancelled'
    ));
