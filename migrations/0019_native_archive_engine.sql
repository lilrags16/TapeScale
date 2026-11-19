-- TapeScale-owned archive metadata and engine-neutral persistence.
--
-- Earlier releases pinned every physical record to Bacula Community 15.0.3.
-- Preserve those rows as legacy audit evidence, but allow new runs to use the
-- TapeScale native media header with either DAR file slices or ZFS-send chunks.

ALTER TABLE qualification_runs
    DROP CONSTRAINT IF EXISTS qualification_runs_engine_name_check,
    DROP CONSTRAINT IF EXISTS qualification_runs_engine_version_check,
    DROP CONSTRAINT IF EXISTS qualification_runs_test_kind_check;

ALTER TABLE qualification_runs
    ALTER COLUMN engine_name SET DEFAULT 'TapeScale Native',
    ALTER COLUMN engine_version SET DEFAULT '1',
    ALTER COLUMN test_kind SET DEFAULT 'native_tape_test',
    ADD CONSTRAINT qualification_runs_engine_name_check
        CHECK (engine_name <> '' AND octet_length(engine_name) <= 128
               AND engine_name !~ '[[:cntrl:]]'),
    ADD CONSTRAINT qualification_runs_engine_version_check
        CHECK (engine_version <> '' AND octet_length(engine_version) <= 64
               AND engine_version !~ '[[:cntrl:]]'),
    ADD CONSTRAINT qualification_runs_test_kind_check
        CHECK (test_kind IN ('btape_test', 'native_tape_test'));

ALTER TABLE single_tape_data_profiles
    DROP CONSTRAINT IF EXISTS single_tape_data_profiles_engine_name_check,
    DROP CONSTRAINT IF EXISTS single_tape_data_profiles_engine_version_check,
    DROP CONSTRAINT IF EXISTS single_tape_data_profiles_adapter_version_check,
    DROP CONSTRAINT IF EXISTS single_tape_data_profiles_format_version_check;

ALTER TABLE single_tape_data_profiles
    ALTER COLUMN engine_name SET DEFAULT 'TapeScale Native',
    ALTER COLUMN engine_version SET DEFAULT '1',
    ALTER COLUMN adapter_version SET DEFAULT 'native-v1',
    ALTER COLUMN format_version SET DEFAULT 'tapescale-native-v1',
    ADD CONSTRAINT single_tape_data_profiles_engine_name_check
        CHECK (engine_name <> '' AND octet_length(engine_name) <= 128
               AND engine_name !~ '[[:cntrl:]]'),
    ADD CONSTRAINT single_tape_data_profiles_engine_version_check
        CHECK (engine_version <> '' AND octet_length(engine_version) <= 64
               AND engine_version !~ '[[:cntrl:]]'),
    ADD CONSTRAINT single_tape_data_profiles_adapter_version_check
        CHECK (adapter_version <> '' AND octet_length(adapter_version) <= 64
               AND adapter_version !~ '[[:cntrl:]]'),
    ADD CONSTRAINT single_tape_data_profiles_format_version_check
        CHECK (format_version <> '' AND octet_length(format_version) <= 128
               AND format_version !~ '[[:cntrl:]]');

ALTER TABLE single_tape_data_profiles
    ADD COLUMN archive_codec text;
UPDATE single_tape_data_profiles
   SET archive_codec = CASE
       WHEN engine_name = 'Bacula Community' THEN 'legacy_bacula'
       ELSE 'dar'
   END;
ALTER TABLE single_tape_data_profiles
    ALTER COLUMN archive_codec SET NOT NULL,
    ALTER COLUMN archive_codec SET DEFAULT 'dar',
    ADD CONSTRAINT single_tape_data_profiles_archive_codec_check
        CHECK (archive_codec IN ('legacy_bacula', 'dar', 'zfs_send'));

-- Profiles are mutable protection configuration, not historical evidence.
-- Move every saved profile to the native writer so an upgraded installation
-- cannot silently launch another Bacula backup. Existing run rows below retain
-- their original engine/codec as immutable audit history.
UPDATE single_tape_data_profiles
   SET engine_name = 'TapeScale Native',
       engine_version = '1',
       adapter_version = 'native-v1',
       format_version = 'tapescale-native-v1',
       archive_codec = 'dar',
       updated_at = now();

ALTER TABLE protection_policies
    DROP CONSTRAINT IF EXISTS protection_policies_compression_mode_check;
ALTER TABLE protection_policies
    -- 0013 deliberately admitted pre-constraint policy rows. Keep that
    -- upgrade guarantee while enforcing the expanded set for new writes.
    ADD CONSTRAINT protection_policies_compression_mode_check
        CHECK (compression_mode IN ('auto', 'none', 'gzip', 'lzo', 'lz4', 'zstd'))
        NOT VALID;

ALTER TABLE single_tape_data_runs
    DROP CONSTRAINT IF EXISTS single_tape_data_runs_engine_name_check,
    DROP CONSTRAINT IF EXISTS single_tape_data_runs_engine_version_check,
    DROP CONSTRAINT IF EXISTS single_tape_data_runs_adapter_version_check,
    DROP CONSTRAINT IF EXISTS single_tape_data_runs_format_version_check,
    DROP CONSTRAINT IF EXISTS single_tape_data_runs_compression_mode_check,
    DROP CONSTRAINT IF EXISTS single_tape_data_runs_resolved_compression_check;

ALTER TABLE single_tape_data_runs
    ADD CONSTRAINT single_tape_data_runs_engine_name_check
        CHECK (engine_name <> '' AND octet_length(engine_name) <= 128
               AND engine_name !~ '[[:cntrl:]]'),
    ADD CONSTRAINT single_tape_data_runs_engine_version_check
        CHECK (engine_version <> '' AND octet_length(engine_version) <= 64
               AND engine_version !~ '[[:cntrl:]]'),
    ADD CONSTRAINT single_tape_data_runs_adapter_version_check
        CHECK (adapter_version <> '' AND octet_length(adapter_version) <= 64
               AND adapter_version !~ '[[:cntrl:]]'),
    ADD CONSTRAINT single_tape_data_runs_format_version_check
        CHECK (format_version <> '' AND octet_length(format_version) <= 128
               AND format_version !~ '[[:cntrl:]]'),
    ADD CONSTRAINT single_tape_data_runs_compression_mode_check
        CHECK (compression_mode IN ('auto', 'none', 'gzip', 'lzo', 'lz4', 'zstd')),
    ADD CONSTRAINT single_tape_data_runs_resolved_compression_check
        CHECK (resolved_compression IN ('none', 'gzip', 'lzo', 'lz4', 'zstd')),
    ADD COLUMN archive_id uuid NOT NULL DEFAULT gen_random_uuid(),
    ADD COLUMN archive_codec text;

UPDATE single_tape_data_runs
   SET archive_codec = CASE
       WHEN engine_name = 'Bacula Community' THEN 'legacy_bacula'
       ELSE 'dar'
   END;
ALTER TABLE single_tape_data_runs
    ALTER COLUMN archive_codec SET NOT NULL,
    ALTER COLUMN archive_codec SET DEFAULT 'dar',
    ADD CONSTRAINT single_tape_data_runs_archive_codec_check
        CHECK (archive_codec IN ('legacy_bacula', 'dar', 'zfs_send')),
    ADD CONSTRAINT single_tape_data_runs_archive_id_unique UNIQUE (archive_id);

-- A queued, untouched request has no physical-format history to preserve.
-- Rebind it with its upgraded profile; failed/interrupted/touched rows remain
-- exactly as recorded for audit.
UPDATE single_tape_data_runs
   SET engine_name = 'TapeScale Native',
       engine_version = '1',
       adapter_version = 'native-v1',
       format_version = 'tapescale-native-v1',
       archive_codec = 'dar',
       -- The native writer always completes the archive on disk before tape
       -- motion.  Preserve only the operator's requested codec; translate the
       -- legacy LZO resolution to the native LZ4 implementation.
       data_delivery = 'disk_spool',
       resolved_compression = CASE compression_mode
           WHEN 'auto' THEN 'zstd'
           WHEN 'zstd' THEN 'zstd'
           WHEN 'lzo' THEN 'lz4'
           WHEN 'lz4' THEN 'lz4'
           WHEN 'gzip' THEN 'gzip'
           ELSE 'none'
       END
 WHERE state = 'queued' AND media_write_started = false;

-- The permanent UUID is read from tape file zero.  The hashes bind the exact
-- header and volume-local index that were physically read back.
ALTER TABLE single_tape_data_run_volumes
    ADD COLUMN archive_volume_id uuid,
    ADD COLUMN header_sha256 text
        CHECK (header_sha256 IS NULL OR header_sha256 ~ '^[0-9a-f]{64}$'),
    ADD COLUMN index_sha256 text
        CHECK (index_sha256 IS NULL OR index_sha256 ~ '^[0-9a-f]{64}$'),
    ADD COLUMN tape_file_count integer
        CHECK (tape_file_count IS NULL OR tape_file_count > 0),
    ADD CONSTRAINT single_tape_data_run_volumes_archive_volume_unique
        UNIQUE (archive_volume_id);

-- Native archive payload is split into complete, independently hashed
-- segments.  A segment is assigned to at most one physical tape file and can
-- remain staged until its physical readback succeeds.
CREATE TABLE single_tape_data_run_segments (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    single_tape_data_run_id uuid NOT NULL
        REFERENCES single_tape_data_runs(id) ON DELETE RESTRICT,
    segment_order integer NOT NULL CHECK (segment_order > 0),
    segment_kind text NOT NULL
        CHECK (segment_kind IN (
            'dar_slice', 'zfs_send_chunk', 'recovery_catalog', 'manifest'
        )),
    content_sha256 text NOT NULL CHECK (content_sha256 ~ '^[0-9a-f]{64}$'),
    logical_bytes bigint CHECK (logical_bytes IS NULL OR logical_bytes >= 0),
    encoded_bytes bigint NOT NULL CHECK (encoded_bytes >= 0),
    storage_ref text
        CHECK (storage_ref IS NULL OR
               (storage_ref <> '' AND storage_ref !~ '[[:cntrl:]]')),
    state text NOT NULL DEFAULT 'staged'
        CHECK (state IN (
            'staged', 'assigned', 'write_started', 'written', 'verified', 'failed'
        )),
    volume_order integer,
    tape_file_number integer
        CHECK (tape_file_number IS NULL OR tape_file_number > 0),
    written_at timestamptz,
    verified_at timestamptz,
    metadata jsonb NOT NULL DEFAULT '{}'
        CHECK (jsonb_typeof(metadata) = 'object'
               AND octet_length(metadata::text) <= 1048576),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (single_tape_data_run_id, segment_order),
    UNIQUE (single_tape_data_run_id, volume_order, tape_file_number),
    FOREIGN KEY (single_tape_data_run_id, volume_order)
        REFERENCES single_tape_data_run_volumes(single_tape_data_run_id, volume_order)
        ON DELETE RESTRICT,
    CHECK ((volume_order IS NULL) = (tape_file_number IS NULL)),
    CHECK (state IN ('staged', 'failed') OR volume_order IS NOT NULL),
    CHECK (written_at IS NULL OR state IN ('written', 'verified')),
    CHECK ((verified_at IS NOT NULL) = (state = 'verified'))
);

CREATE INDEX single_tape_data_run_segments_run_state
    ON single_tape_data_run_segments(single_tape_data_run_id, state, segment_order);

ALTER TABLE single_tape_data_events
    DROP CONSTRAINT single_tape_data_events_event_type_check;
ALTER TABLE single_tape_data_events
    ADD CONSTRAINT single_tape_data_events_event_type_check CHECK (event_type IN (
        'queued', 'started', 'preparing', 'inventorying', 'writing', 'written', 'verifying',
        'restoring', 'comparing', 'archive_staged', 'media_write_started', 'media_prepared',
        'media_required', 'media_authorized', 'media_loaded', 'media_full',
        'media_identified', 'media_read', 'media_verified', 'engine_dispatched', 'fenced_out',
        'cancel_requested',
        'passed', 'failed', 'blocked', 'interrupted', 'cancelled'
    ));

ALTER TABLE recovery_points
    ADD COLUMN archive_id uuid,
    ADD COLUMN archive_codec text;

UPDATE recovery_points recovery
   SET archive_id = data_run.archive_id,
       archive_codec = data_run.archive_codec
  FROM single_tape_data_runs data_run
 WHERE data_run.job_run_id = recovery.run_id;
UPDATE recovery_points
   SET archive_id = gen_random_uuid()
 WHERE archive_id IS NULL;
UPDATE recovery_points
   SET archive_codec = CASE
       WHEN format_version = 'bacula-community-15.0.3-lab-v1' THEN 'legacy_bacula'
       WHEN representation = 'native_zfs' THEN 'zfs_send'
       ELSE 'dar'
   END
 WHERE archive_codec IS NULL;

-- This release is an intentional clean break because the deployment has no
-- valid Bacula recovery media.  Preserve the old point, volume, and
-- verification rows as audit evidence, but never continue presenting them as
-- recoverable or allowing them to protect cartridges from reuse.  A queued
-- legacy restore will consequently fail its normal verified-origin recheck
-- before any tape read or unavailable Bacula process can be dispatched.
UPDATE recovery_points
   SET status = 'failed',
       coverage = coverage || jsonb_build_object(
           'legacy_format_retired', true,
           'legacy_format_retired_reason',
           'Bacula media declared non-recoverable during TapeScale Native v1 migration'
       )
 WHERE archive_codec = 'legacy_bacula';

ALTER TABLE recovery_points
    ALTER COLUMN archive_id SET NOT NULL,
    ALTER COLUMN archive_id SET DEFAULT gen_random_uuid(),
    ALTER COLUMN archive_codec SET NOT NULL,
    ALTER COLUMN archive_codec SET DEFAULT 'dar',
    ADD CONSTRAINT recovery_points_archive_id_unique UNIQUE (archive_id),
    ADD CONSTRAINT recovery_points_archive_codec_check
        CHECK (archive_codec IN ('legacy_bacula', 'dar', 'zfs_send'));

ALTER TABLE volume_records
    ADD COLUMN archive_volume_id uuid,
    ADD COLUMN header_sha256 text
        CHECK (header_sha256 IS NULL OR header_sha256 ~ '^[0-9a-f]{64}$'),
    ADD COLUMN index_sha256 text
        CHECK (index_sha256 IS NULL OR index_sha256 ~ '^[0-9a-f]{64}$'),
    ADD COLUMN tape_file_count integer
        CHECK (tape_file_count IS NULL OR tape_file_count > 0),
    ADD CONSTRAINT volume_records_archive_volume_unique UNIQUE (archive_volume_id);

-- Native restore rows use UUID/text archive identities.  Keep the numeric
-- Bacula columns nullable so existing history remains readable without making
-- them a requirement for a new archive.
ALTER TABLE restore_rehearsals
    DROP CONSTRAINT IF EXISTS restore_rehearsals_engine_name_check,
    DROP CONSTRAINT IF EXISTS restore_rehearsals_engine_version_check,
    DROP CONSTRAINT IF EXISTS restore_rehearsals_adapter_version_check,
    DROP CONSTRAINT IF EXISTS restore_rehearsals_format_version_check,
    ALTER COLUMN backup_job_id DROP NOT NULL,
    ALTER COLUMN expected_engine_media_id DROP NOT NULL,
    ADD COLUMN archive_id uuid REFERENCES recovery_points(archive_id) ON DELETE RESTRICT,
    ADD COLUMN engine_operation_id text,
    ADD COLUMN expected_archive_volume_id uuid;

UPDATE restore_rehearsals rehearsal
   SET archive_id = recovery.archive_id,
       engine_operation_id = 'bacula-job:' || rehearsal.backup_job_id::text
  FROM recovery_points recovery
 WHERE recovery.id = rehearsal.recovery_point_id;

ALTER TABLE restore_rehearsals
    ALTER COLUMN archive_id SET NOT NULL,
    ADD CONSTRAINT restore_rehearsals_engine_name_check
        CHECK (engine_name <> '' AND octet_length(engine_name) <= 128
               AND engine_name !~ '[[:cntrl:]]'),
    ADD CONSTRAINT restore_rehearsals_engine_version_check
        CHECK (engine_version <> '' AND octet_length(engine_version) <= 64
               AND engine_version !~ '[[:cntrl:]]'),
    ADD CONSTRAINT restore_rehearsals_adapter_version_check
        CHECK (adapter_version <> '' AND octet_length(adapter_version) <= 64
               AND adapter_version !~ '[[:cntrl:]]'),
    ADD CONSTRAINT restore_rehearsals_format_version_check
        CHECK (format_version <> '' AND octet_length(format_version) <= 128
               AND format_version !~ '[[:cntrl:]]'),
    ADD CONSTRAINT restore_rehearsals_engine_operation_check
        CHECK (engine_operation_id IS NULL OR
               (engine_operation_id <> '' AND octet_length(engine_operation_id) <= 256
                AND engine_operation_id !~ '[[:cntrl:]]'));

ALTER TABLE restore_rehearsal_volumes
    ALTER COLUMN expected_engine_media_id DROP NOT NULL,
    ADD COLUMN expected_archive_volume_id uuid,
    ADD COLUMN first_segment integer CHECK (first_segment IS NULL OR first_segment > 0),
    ADD COLUMN last_segment integer CHECK (last_segment IS NULL OR last_segment > 0),
    ADD CONSTRAINT restore_rehearsal_volumes_segment_range_check
        CHECK (last_segment IS NULL OR
               (first_segment IS NOT NULL AND last_segment >= first_segment));
