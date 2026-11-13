-- Durable attended full-file backup workflow.
--
-- Existing G0.2 source bindings and fenced data runs remain the physical
-- execution boundary.  These columns distinguish recovery exercises from
-- operator-facing backup policies and allow a backup request/job-run identity
-- to exist in waiting_for_media before an exact drive and cartridge are bound.

ALTER TABLE single_tape_data_profiles
    ADD COLUMN workflow_kind text NOT NULL DEFAULT 'recovery_exercise'
        CHECK (workflow_kind IN ('recovery_exercise', 'backup')),
    ADD COLUMN policy_id uuid UNIQUE
        REFERENCES protection_policies(id) ON DELETE RESTRICT,
    ADD COLUMN media_pool_id uuid
        REFERENCES media_pools(id) ON DELETE RESTRICT,
    ADD COLUMN schedule_enabled boolean NOT NULL DEFAULT false,
    ADD COLUMN retention_days integer NOT NULL DEFAULT 28
        CHECK (retention_days > 0 AND retention_days <= 36500),
    ADD CONSTRAINT single_tape_backup_profile_contract CHECK (
        (workflow_kind = 'recovery_exercise'
            AND policy_id IS NULL
            AND media_pool_id IS NULL
            AND NOT schedule_enabled) OR
        (workflow_kind = 'backup' AND policy_id IS NOT NULL)
    );

ALTER TABLE single_tape_data_runs
    ADD COLUMN workflow_kind text NOT NULL DEFAULT 'recovery_exercise'
        CHECK (workflow_kind IN ('recovery_exercise', 'backup')),
    ADD COLUMN trigger_kind text NOT NULL DEFAULT 'attended'
        CHECK (trigger_kind IN ('attended', 'manual', 'scheduled')),
    ADD COLUMN requested_level text NOT NULL DEFAULT 'full'
        CHECK (requested_level = 'full'),
    ADD COLUMN scheduled_occurrence text
        CHECK (scheduled_occurrence IS NULL OR
               (scheduled_occurrence <> ''
                AND octet_length(scheduled_occurrence) <= 256
                AND scheduled_occurrence !~ '[[:cntrl:]]')),
    ADD COLUMN retention_days integer NOT NULL DEFAULT 28
        CHECK (retention_days > 0 AND retention_days <= 36500),
    ADD COLUMN cancel_requested_at timestamptz,
    ADD COLUMN cancel_requested_by_label text
        CHECK (cancel_requested_by_label IS NULL OR
               (cancel_requested_by_label <> ''
                AND octet_length(cancel_requested_by_label) <= 256
                AND cancel_requested_by_label !~ '[[:cntrl:]]')),
    ADD CONSTRAINT single_tape_data_run_trigger_contract CHECK (
        (workflow_kind = 'recovery_exercise'
            AND trigger_kind = 'attended'
            AND scheduled_occurrence IS NULL) OR
        (workflow_kind = 'backup'
            AND trigger_kind IN ('manual', 'scheduled')
            AND ((trigger_kind = 'manual' AND scheduled_occurrence IS NULL)
                 OR (trigger_kind = 'scheduled' AND scheduled_occurrence IS NOT NULL)))
    ),
    ADD CONSTRAINT single_tape_data_run_cancel_contract CHECK (
        (cancel_requested_at IS NULL) = (cancel_requested_by_label IS NULL)
    );

ALTER TABLE single_tape_data_events DROP CONSTRAINT single_tape_data_events_event_type_check;
ALTER TABLE single_tape_data_events ADD CONSTRAINT single_tape_data_events_event_type_check
    CHECK (event_type IN (
        'queued', 'started', 'preparing', 'writing', 'written', 'verifying',
        'restoring', 'comparing', 'media_write_started', 'media_prepared',
        'engine_dispatched', 'fenced_out', 'cancel_requested',
        'passed', 'failed', 'blocked', 'interrupted', 'cancelled'
    ));

CREATE TABLE backup_requests (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    profile_id uuid NOT NULL
        REFERENCES single_tape_data_profiles(id) ON DELETE RESTRICT,
    job_run_id uuid NOT NULL UNIQUE REFERENCES job_runs(id) ON DELETE RESTRICT,
    trigger_kind text NOT NULL CHECK (trigger_kind IN ('manual', 'scheduled')),
    requested_level text NOT NULL DEFAULT 'full' CHECK (requested_level = 'full'),
    scheduled_occurrence text
        CHECK (scheduled_occurrence IS NULL OR
               (scheduled_occurrence <> ''
                AND octet_length(scheduled_occurrence) <= 256
                AND scheduled_occurrence !~ '[[:cntrl:]]')),
    idempotency_key text NOT NULL
        CHECK (idempotency_key <> '' AND octet_length(idempotency_key) <= 256
               AND idempotency_key !~ '[[:cntrl:]]'),
    assigned_data_run_id uuid UNIQUE
        REFERENCES single_tape_data_runs(id) ON DELETE RESTRICT,
    requested_by_label text NOT NULL
        CHECK (requested_by_label <> ''
               AND octet_length(requested_by_label) <= 256
               AND requested_by_label !~ '[[:cntrl:]]'),
    correlation_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    cancelled_at timestamptz,
    cancelled_by_label text
        CHECK (cancelled_by_label IS NULL OR
               (cancelled_by_label <> ''
                AND octet_length(cancelled_by_label) <= 256
                AND cancelled_by_label !~ '[[:cntrl:]]')),

    UNIQUE (profile_id, idempotency_key),
    CHECK (
        (trigger_kind = 'manual' AND scheduled_occurrence IS NULL) OR
        (trigger_kind = 'scheduled' AND scheduled_occurrence IS NOT NULL)
    ),
    CHECK ((cancelled_at IS NULL) = (cancelled_by_label IS NULL))
);

CREATE UNIQUE INDEX backup_requests_scheduled_occurrence
    ON backup_requests(profile_id, scheduled_occurrence)
    WHERE scheduled_occurrence IS NOT NULL;

CREATE INDEX backup_requests_recent
    ON backup_requests(created_at DESC);

CREATE TABLE backup_request_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    backup_request_id uuid NOT NULL
        REFERENCES backup_requests(id) ON DELETE RESTRICT,
    event_type text NOT NULL CHECK (event_type IN (
        'requested', 'waiting_for_media', 'assignment_recorded', 'cancelled'
    )),
    evidence jsonb NOT NULL DEFAULT '{}'
        CHECK (jsonb_typeof(evidence) = 'object'
               AND octet_length(evidence::text) <= 1048576),
    actor_label text NOT NULL
        CHECK (actor_label <> '' AND octet_length(actor_label) <= 256
               AND actor_label !~ '[[:cntrl:]]'),
    correlation_id uuid NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX backup_request_events_request_time
    ON backup_request_events(backup_request_id, occurred_at, id);

CREATE TRIGGER backup_request_events_append_only
    BEFORE UPDATE OR DELETE ON backup_request_events
    FOR EACH ROW EXECUTE FUNCTION tapescale_reject_history_mutation();
