-- Durable, lab-only non-data tape motion requests.
--
-- These records deliberately exclude write, erase, format, label, seek, spacing,
-- and file-mark operations. Production data movement remains owned by a selected
-- and qualified tape engine.

CREATE SEQUENCE resource_fencing_token_seq AS bigint;

CREATE TABLE drive_operations (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    drive_id uuid NOT NULL REFERENCES drives(id) ON DELETE RESTRICT,
    expected_stable_id text NOT NULL,
    expected_path text NOT NULL,
    expected_vendor text,
    expected_model text,
    expected_serial text,
    expected_wwn text,
    expected_firmware text,
    action text NOT NULL CHECK (action IN ('rewind', 'unload')),
    scratch_media_confirmed boolean NOT NULL CHECK (scratch_media_confirmed),
    state text NOT NULL DEFAULT 'queued'
        CHECK (state IN ('queued', 'running', 'succeeded', 'failed', 'blocked', 'cancelled')),
    requested_by_label text NOT NULL,
    correlation_id uuid NOT NULL,
    lease_id uuid REFERENCES resource_leases(id) ON DELETE RESTRICT,
    fencing_token bigint,
    attempt integer NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    result jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz,
    finished_at timestamptz,
    CHECK ((lease_id IS NULL) = (fencing_token IS NULL)),
    CHECK (finished_at IS NULL OR started_at IS NOT NULL)
);

CREATE UNIQUE INDEX drive_operations_one_active_per_drive
    ON drive_operations(drive_id)
    WHERE state IN ('queued', 'running');

CREATE INDEX drive_operations_recent
    ON drive_operations(created_at DESC);
