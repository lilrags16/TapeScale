-- Durable attended multi-volume full-file restores.
--
-- Restore media handoffs are deliberately separate from backup continuation
-- requests: every required archive volume is snapshotted when the restore is
-- queued, and only the exact next read-only volume may be authorized.

ALTER TABLE restore_rehearsals
    ADD COLUMN workflow_kind text NOT NULL DEFAULT 'recovery_exercise'
        CHECK (workflow_kind IN ('recovery_exercise', 'backup')),
    ADD COLUMN volume_count integer NOT NULL DEFAULT 1
        CHECK (volume_count > 0);

ALTER TABLE restore_rehearsals
    DROP CONSTRAINT restore_rehearsals_phase_check;
ALTER TABLE restore_rehearsals
    ADD CONSTRAINT restore_rehearsals_phase_check CHECK (phase IN (
        'queued', 'preparing', 'waiting_for_media', 'restoring', 'comparing',
        'passed', 'failed', 'blocked', 'interrupted', 'cancelled'
    ));

DO $$
DECLARE
    constraint_name text;
BEGIN
    SELECT conname INTO constraint_name
      FROM pg_constraint
     WHERE conrelid = 'restore_rehearsals'::regclass
       AND contype = 'c'
       AND pg_get_constraintdef(oid) LIKE '%state = ''running''%'
       AND pg_get_constraintdef(oid) LIKE '%phase = state%'
     LIMIT 1;
    IF constraint_name IS NULL THEN
        RAISE EXCEPTION 'restore rehearsal state/phase constraint was not found';
    END IF;
    EXECUTE format(
        'ALTER TABLE restore_rehearsals DROP CONSTRAINT %I',
        constraint_name
    );
END $$;

ALTER TABLE restore_rehearsals
    ADD CONSTRAINT restore_rehearsals_state_phase_check CHECK (
        (state = 'queued' AND phase = 'queued'
            AND started_at IS NULL AND finished_at IS NULL) OR
        (state = 'running'
            AND phase IN ('preparing', 'waiting_for_media', 'restoring', 'comparing')
            AND started_at IS NOT NULL AND finished_at IS NULL) OR
        (state IN ('passed', 'failed', 'blocked', 'interrupted', 'cancelled')
            AND phase = state AND started_at IS NOT NULL AND finished_at IS NOT NULL)
    );

UPDATE restore_rehearsals rehearsal
   SET workflow_kind = data_run.workflow_kind,
       volume_count = (
           SELECT GREATEST(1, count(*)::integer)
             FROM volume_records volume
            WHERE volume.recovery_point_id = rehearsal.recovery_point_id
       )
  FROM single_tape_data_runs data_run
 WHERE data_run.id = rehearsal.original_data_run_id;

CREATE TABLE restore_rehearsal_volumes (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    restore_rehearsal_id uuid NOT NULL
        REFERENCES restore_rehearsals(id) ON DELETE RESTRICT,
    volume_order integer NOT NULL CHECK (volume_order > 0),
    cartridge_id uuid NOT NULL REFERENCES cartridges(id) ON DELETE RESTRICT,
    -- These three identity snapshots deliberately retain the permissive
    -- pre-0014 scalar contracts so every schema-valid legacy rehearsal can be
    -- backfilled. New queue/handoff paths enforce their tighter byte and
    -- character alphabets before insert.
    expected_cartridge_label text NOT NULL,
    expected_media_generation smallint
        CHECK (expected_media_generation BETWEEN 1 AND 10),
    expected_media_format text,
    expected_volume_label text NOT NULL,
    expected_on_tape_identity text NOT NULL,
    expected_engine_media_id bigint NOT NULL CHECK (expected_engine_media_id > 0),
    expected_engine_record_id text NOT NULL CHECK (expected_engine_record_id <> ''),
    expected_first_record text NOT NULL,
    expected_last_record text,
    expected_encoded_bytes bigint CHECK (expected_encoded_bytes IS NULL OR expected_encoded_bytes >= 0),
    state text NOT NULL DEFAULT 'reserved'
        CHECK (state IN ('reserved', 'loaded', 'completed', 'cancelled')),
    active boolean NOT NULL DEFAULT true,
    media_lease_id uuid REFERENCES resource_leases(id) ON DELETE RESTRICT,
    media_fencing_token bigint,
    loaded_at timestamptz,
    completed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (restore_rehearsal_id, volume_order),
    UNIQUE (restore_rehearsal_id, cartridge_id),
    UNIQUE (restore_rehearsal_id, expected_volume_label),
    CHECK ((media_lease_id IS NULL) = (media_fencing_token IS NULL)),
    CHECK (state <> 'loaded' OR media_lease_id IS NOT NULL),
    CHECK (state NOT IN ('loaded', 'completed') OR loaded_at IS NOT NULL),
    CHECK ((state = 'completed') = (completed_at IS NOT NULL)),
    CHECK (state <> 'cancelled' OR NOT active)
);

CREATE UNIQUE INDEX restore_rehearsal_volumes_one_active_cartridge
    ON restore_rehearsal_volumes(cartridge_id) WHERE active;

CREATE UNIQUE INDEX restore_rehearsal_volumes_one_loaded_per_run
    ON restore_rehearsal_volumes(restore_rehearsal_id)
    WHERE active AND state = 'loaded';

CREATE INDEX restore_rehearsal_volumes_run_order
    ON restore_rehearsal_volumes(restore_rehearsal_id, volume_order);

-- Backfill the scalar single-volume contract. The scalar columns remain the
-- legacy/first-volume compatibility binding; this ledger is authoritative for
-- all new queue, handoff, and finalization decisions.
INSERT INTO restore_rehearsal_volumes
    (restore_rehearsal_id, volume_order, cartridge_id,
     expected_cartridge_label, expected_media_generation, expected_media_format,
     expected_volume_label, expected_on_tape_identity,
     expected_engine_media_id, expected_engine_record_id,
     expected_first_record, expected_last_record, expected_encoded_bytes,
     state, active, media_lease_id, media_fencing_token,
     loaded_at, completed_at, created_at, updated_at)
SELECT rehearsal.id, 1, rehearsal.cartridge_id,
       cartridge.internal_label, rehearsal.expected_media_generation,
       rehearsal.expected_media_format, rehearsal.expected_volume_label,
       rehearsal.expected_on_tape_identity, rehearsal.expected_engine_media_id,
       rehearsal.expected_engine_record_id, volume.first_record,
       volume.last_record, volume.encoded_bytes,
       CASE
           WHEN rehearsal.state = 'passed' THEN 'completed'
           WHEN rehearsal.state IN ('failed', 'blocked', 'interrupted', 'cancelled') THEN 'cancelled'
           WHEN rehearsal.state = 'running'
                AND rehearsal.media_lease_id IS NOT NULL
                AND rehearsal.media_fencing_token IS NOT NULL THEN 'loaded'
           ELSE 'reserved'
       END,
       rehearsal.state IN ('queued', 'running'),
       CASE WHEN rehearsal.state = 'running'
                  AND rehearsal.media_lease_id IS NOT NULL
                  AND rehearsal.media_fencing_token IS NOT NULL
            THEN rehearsal.media_lease_id END,
       CASE WHEN rehearsal.state = 'running'
                  AND rehearsal.media_lease_id IS NOT NULL
                  AND rehearsal.media_fencing_token IS NOT NULL
            THEN rehearsal.media_fencing_token END,
       CASE WHEN rehearsal.state = 'passed'
                  OR (rehearsal.state = 'running'
                      AND rehearsal.media_lease_id IS NOT NULL
                      AND rehearsal.media_fencing_token IS NOT NULL)
            THEN rehearsal.started_at END,
       CASE WHEN rehearsal.state = 'passed' THEN rehearsal.finished_at END,
       rehearsal.created_at, COALESCE(rehearsal.finished_at, rehearsal.created_at)
  FROM restore_rehearsals rehearsal
  JOIN cartridges cartridge ON cartridge.id = rehearsal.cartridge_id
  JOIN volume_records volume
    ON volume.recovery_point_id = rehearsal.recovery_point_id
   AND volume.volume_order = 1;

CREATE TABLE restore_rehearsal_media_requests (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    restore_rehearsal_id uuid NOT NULL
        REFERENCES restore_rehearsals(id) ON DELETE RESTRICT,
    request_sequence integer NOT NULL CHECK (request_sequence > 0),
    volume_order integer NOT NULL CHECK (volume_order > 1),
    expected_cartridge_id uuid NOT NULL REFERENCES cartridges(id) ON DELETE RESTRICT,
    expected_cartridge_label text NOT NULL
        CHECK (expected_cartridge_label <> ''
               AND octet_length(expected_cartridge_label) <= 128
               AND expected_cartridge_label !~ '[[:cntrl:]]'),
    expected_engine_volume_name text NOT NULL
        CHECK (expected_engine_volume_name <> ''
               AND octet_length(expected_engine_volume_name) <= 127
               AND expected_engine_volume_name ~ '^[A-Za-z0-9_.-]+$'),
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
    UNIQUE (restore_rehearsal_id, request_sequence),
    UNIQUE (restore_rehearsal_id, volume_order),
    FOREIGN KEY (restore_rehearsal_id, volume_order)
        REFERENCES restore_rehearsal_volumes(restore_rehearsal_id, volume_order)
        ON DELETE RESTRICT,
    CHECK ((state = 'pending' AND authorized_at IS NULL
            AND authorized_cartridge_id IS NULL) OR
           (state IN ('authorized', 'loaded', 'completed')
            AND authorized_at IS NOT NULL
            AND authorized_cartridge_id IS NOT NULL) OR
           state = 'cancelled'),
    CHECK (loaded_at IS NULL OR authorized_at IS NOT NULL),
    CHECK (completed_at IS NULL OR loaded_at IS NOT NULL)
);

CREATE UNIQUE INDEX restore_rehearsal_one_open_media_request
    ON restore_rehearsal_media_requests(restore_rehearsal_id)
    WHERE state IN ('pending', 'authorized', 'loaded');

ALTER TABLE restore_rehearsal_events
    DROP CONSTRAINT restore_rehearsal_events_event_type_check;
ALTER TABLE restore_rehearsal_events
    ADD CONSTRAINT restore_rehearsal_events_event_type_check CHECK (event_type IN (
        'queued', 'started', 'media_observed', 'media_required',
        'media_authorized', 'media_loaded', 'media_read', 'restoring',
        'engine_dispatched', 'engine_completed', 'comparing',
        'cancel_requested', 'passed', 'failed', 'blocked',
        'interrupted', 'cancelled', 'fenced_out'
    ));
