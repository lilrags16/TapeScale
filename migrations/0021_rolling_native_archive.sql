-- TapeScale Native v2: bounded rolling-spool archive packs and an explicit
-- final archive-set commitment. Existing Native v1 rows retain layout version
-- one and remain readable without being reinterpreted as rolling archives.

-- Operational backup configuration is mutable and should create v2 work
-- after upgrade. G0.2 recovery exercises retain the Native v1 writer because
-- the v2 path does not implement their restore-and-compare phase.
ALTER TABLE single_tape_data_profiles
    ALTER COLUMN engine_version SET DEFAULT '2',
    ALTER COLUMN adapter_version SET DEFAULT 'native-v2',
    ALTER COLUMN format_version SET DEFAULT 'tapescale-native-v2';

UPDATE single_tape_data_profiles
   SET engine_name = 'TapeScale Native',
       engine_version = '2',
       adapter_version = 'native-v2',
       format_version = 'tapescale-native-v2',
       archive_codec = 'dar',
       updated_at = now()
 WHERE archive_codec = 'dar'
   AND workflow_kind = 'backup';

ALTER TABLE single_tape_data_runs
    ADD COLUMN native_layout_version smallint NOT NULL DEFAULT 1
        CHECK (native_layout_version IN (1, 2)),
    ADD COLUMN native_producer_state text NOT NULL DEFAULT 'not_started'
        CHECK (native_producer_state IN (
            'not_started', 'running', 'paused', 'complete', 'failed'
        )),
    ADD COLUMN native_spool_low_water_bytes bigint
        CHECK (native_spool_low_water_bytes IS NULL OR native_spool_low_water_bytes > 0),
    ADD COLUMN native_spool_high_water_bytes bigint
        CHECK (native_spool_high_water_bytes IS NULL OR native_spool_high_water_bytes > 0),
    ADD COLUMN native_produced_pack_count bigint
        CHECK (native_produced_pack_count IS NULL OR native_produced_pack_count > 0),
    ADD COLUMN native_produced_encoded_bytes bigint
        CHECK (native_produced_encoded_bytes IS NULL OR native_produced_encoded_bytes > 0),
    ADD COLUMN native_producer_root_sha256 text
        CHECK (native_producer_root_sha256 IS NULL OR
               native_producer_root_sha256 ~ '^[0-9a-f]{64}$'),
    ADD COLUMN native_producer_completed_at timestamptz,
    ADD COLUMN native_archive_seal_sha256 text
        CHECK (native_archive_seal_sha256 IS NULL OR
               native_archive_seal_sha256 ~ '^[0-9a-f]{64}$'),
    ADD COLUMN native_archive_sealed_at timestamptz,
    ADD CONSTRAINT single_tape_data_runs_native_spool_window_check CHECK (
        (native_spool_low_water_bytes IS NULL AND native_spool_high_water_bytes IS NULL)
        OR
        (native_spool_low_water_bytes IS NOT NULL AND
         native_spool_high_water_bytes IS NOT NULL AND
         native_spool_low_water_bytes < native_spool_high_water_bytes)
    ),
    ADD CONSTRAINT single_tape_data_runs_native_producer_commit_check CHECK (
        native_producer_state <> 'complete'
        OR
        (native_produced_pack_count IS NOT NULL AND
         native_produced_encoded_bytes IS NOT NULL AND
         native_producer_root_sha256 IS NOT NULL AND
         native_producer_completed_at IS NOT NULL)
    ),
    ADD CONSTRAINT single_tape_data_runs_native_seal_pair_check CHECK (
        (native_archive_seal_sha256 IS NULL) = (native_archive_sealed_at IS NULL)
    );

-- Only requests that have never crossed a media-write boundary can safely be
-- rebound. Every running or terminal row remains immutable Native v1 history.
UPDATE single_tape_data_runs
   SET engine_name = 'TapeScale Native',
       engine_version = '2',
       adapter_version = 'native-v2',
       format_version = 'tapescale-native-v2',
       archive_codec = 'dar',
       native_layout_version = 2
 WHERE state = 'queued'
   AND media_write_started = false
   AND archive_codec = 'dar'
   AND workflow_kind = 'backup';

ALTER TABLE single_tape_data_runs
    ALTER COLUMN engine_version SET DEFAULT '2',
    ALTER COLUMN adapter_version SET DEFAULT 'native-v2',
    ALTER COLUMN format_version SET DEFAULT 'tapescale-native-v2',
    ALTER COLUMN native_layout_version SET DEFAULT 2;

ALTER TABLE single_tape_data_run_volumes
    ADD COLUMN native_layout_version smallint NOT NULL DEFAULT 1
        CHECK (native_layout_version IN (1, 2)),
    ADD COLUMN previous_archive_volume_id uuid,
    ADD COLUMN previous_index_sha256 text
        CHECK (previous_index_sha256 IS NULL OR
               previous_index_sha256 ~ '^[0-9a-f]{64}$'),
    ADD COLUMN archive_final boolean NOT NULL DEFAULT false,
    ADD COLUMN archive_seal_sha256 text
        CHECK (archive_seal_sha256 IS NULL OR
               archive_seal_sha256 ~ '^[0-9a-f]{64}$'),
    ADD CONSTRAINT single_tape_data_run_volumes_v2_chain_check CHECK (
        native_layout_version = 1
        OR
        (
            (volume_order = 1 AND previous_archive_volume_id IS NULL AND
             previous_index_sha256 IS NULL)
            OR
            (volume_order > 1 AND previous_archive_volume_id IS NOT NULL AND
             previous_index_sha256 IS NOT NULL)
        )
    ),
    ADD CONSTRAINT single_tape_data_run_volumes_v2_seal_check CHECK (
        native_layout_version = 1
        OR
        ((archive_seal_sha256 IS NOT NULL) = archive_final)
    );

-- The first volume assignment is created with the run. Move only assignments
-- belonging to the untouched queued runs upgraded above; media-touched and
-- historical volume rows remain immutable v1 evidence.
UPDATE single_tape_data_run_volumes volume
   SET native_layout_version = 2
  FROM single_tape_data_runs data_run
 WHERE volume.single_tape_data_run_id = data_run.id
   AND data_run.native_layout_version = 2
   AND data_run.state = 'queued'
   AND data_run.media_write_started = false
   AND volume.media_write_started_at IS NULL;

ALTER TABLE single_tape_data_run_volumes
    ALTER COLUMN native_layout_version SET DEFAULT 2;

ALTER TABLE single_tape_data_run_segments
    DROP CONSTRAINT single_tape_data_run_segments_segment_kind_check;
ALTER TABLE single_tape_data_run_segments
    ADD CONSTRAINT single_tape_data_run_segments_segment_kind_check CHECK (
        segment_kind IN (
            'dar_slice', 'dar_pack', 'native_file_chunk', 'zfs_send_chunk',
            'recovery_catalog', 'manifest'
        )
    ),
    ADD COLUMN native_layout_version smallint NOT NULL DEFAULT 1
        CHECK (native_layout_version IN (1, 2)),
    ADD COLUMN pack_id uuid,
    ADD COLUMN pack_kind text
        CHECK (pack_kind IS NULL OR pack_kind IN ('dar_archive', 'native_file_chunk')),
    ADD COLUMN catalog_sha256 text
        CHECK (catalog_sha256 IS NULL OR catalog_sha256 ~ '^[0-9a-f]{64}$'),
    ADD COLUMN manifest_entry_start bigint
        CHECK (manifest_entry_start IS NULL OR manifest_entry_start > 0),
    ADD COLUMN manifest_entry_count bigint
        CHECK (manifest_entry_count IS NULL OR manifest_entry_count > 0),
    ADD COLUMN spool_state text NOT NULL DEFAULT 'present'
        CHECK (spool_state IN ('present', 'reclaiming', 'reclaimed', 'missing')),
    ADD COLUMN spool_reclaimed_at timestamptz,
    ADD CONSTRAINT single_tape_data_run_segments_pack_unique
        UNIQUE (single_tape_data_run_id, pack_id),
    ADD CONSTRAINT single_tape_data_run_segments_spool_reclaim_check CHECK (
        native_layout_version = 1
        OR
        (
            (spool_state = 'reclaimed' AND storage_ref IS NULL AND
             spool_reclaimed_at IS NOT NULL)
            OR
            (spool_state <> 'reclaimed' AND spool_reclaimed_at IS NULL)
        )
    ),
    ADD CONSTRAINT single_tape_data_run_segments_v2_pack_check CHECK (
        native_layout_version = 1
        OR
        (pack_id IS NOT NULL AND pack_kind IS NOT NULL AND
         manifest_entry_start IS NOT NULL AND manifest_entry_count IS NOT NULL AND
         ((pack_kind = 'dar_archive' AND catalog_sha256 IS NOT NULL AND
           segment_kind = 'dar_pack')
          OR
          (pack_kind = 'native_file_chunk' AND catalog_sha256 IS NULL AND
           segment_kind = 'native_file_chunk')))
    );

ALTER TABLE single_tape_data_run_segments
    ALTER COLUMN native_layout_version SET DEFAULT 2;

CREATE INDEX single_tape_data_run_segments_spool_queue
    ON single_tape_data_run_segments
        (single_tape_data_run_id, spool_state, state, segment_order)
    WHERE native_layout_version = 2;

-- The final seal is append-only evidence. It binds the exact volume and pack
-- roots after the producer is complete and before a recovery point may exist.
CREATE TABLE native_archive_set_seals (
    single_tape_data_run_id uuid PRIMARY KEY
        REFERENCES single_tape_data_runs(id) ON DELETE RESTRICT,
    archive_id uuid NOT NULL UNIQUE,
    format_version smallint NOT NULL CHECK (format_version = 2),
    final_archive_volume_id uuid NOT NULL,
    source_manifest_sha256 text NOT NULL
        CHECK (source_manifest_sha256 ~ '^[0-9a-f]{64}$'),
    pack_catalog_root_sha256 text NOT NULL
        CHECK (pack_catalog_root_sha256 ~ '^[0-9a-f]{64}$'),
    volume_count integer NOT NULL CHECK (volume_count > 0),
    pack_count bigint NOT NULL CHECK (pack_count > 0),
    total_encoded_bytes bigint NOT NULL CHECK (total_encoded_bytes > 0),
    total_logical_bytes bigint NOT NULL CHECK (total_logical_bytes >= 0),
    total_entries bigint NOT NULL CHECK (total_entries > 0),
    ordered_volume_index_sha256 text NOT NULL
        CHECK (ordered_volume_index_sha256 ~ '^[0-9a-f]{64}$'),
    ordered_pack_sha256 text NOT NULL
        CHECK (ordered_pack_sha256 ~ '^[0-9a-f]{64}$'),
    archive_set_sha256 text NOT NULL UNIQUE
        CHECK (archive_set_sha256 ~ '^[0-9a-f]{64}$'),
    evidence jsonb NOT NULL DEFAULT '{}'
        CHECK (jsonb_typeof(evidence) = 'object' AND
               octet_length(evidence::text) <= 1048576),
    completed_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TRIGGER native_archive_set_seals_append_only
    BEFORE UPDATE OR DELETE ON native_archive_set_seals
    FOR EACH ROW EXECUTE FUNCTION tapescale_reject_history_mutation();

ALTER TABLE single_tape_data_events
    DROP CONSTRAINT single_tape_data_events_event_type_check;
ALTER TABLE single_tape_data_events
    ADD CONSTRAINT single_tape_data_events_event_type_check CHECK (event_type IN (
        'queued', 'started', 'preparing', 'inventorying', 'writing', 'written', 'verifying',
        'restoring', 'comparing', 'archive_staged', 'pack_staged', 'pack_assigned',
        'pack_written', 'spool_reclaimed', 'producer_paused', 'producer_resumed',
        'producer_complete', 'archive_sealed', 'media_write_started', 'media_prepared',
        'media_required', 'media_authorized', 'media_loaded', 'media_full',
        'media_identified', 'media_read', 'media_verified', 'engine_dispatched', 'fenced_out',
        'cancel_requested',
        'passed', 'failed', 'blocked', 'interrupted', 'cancelled'
    ));
