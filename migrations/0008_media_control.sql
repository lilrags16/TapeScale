-- Durable media inventory, pool, hold, and reuse-decision operations.
--
-- This migration adds optimistic cartridge versions and append-only mutation
-- receipts. It deliberately adds no erase, relabel, recycle, or policy-override
-- primitive. Every existing cartridge update, including worker and custody
-- updates outside media_ops, advances state_version through the trigger below.

ALTER TABLE cartridges
    ADD COLUMN state_version bigint NOT NULL DEFAULT 0 CHECK (state_version >= 0);

CREATE OR REPLACE FUNCTION tapescale_increment_cartridge_state_version()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    NEW.state_version := OLD.state_version + 1;
    RETURN NEW;
END;
$$;

CREATE TRIGGER cartridges_advance_state_version
    BEFORE UPDATE ON cartridges
    FOR EACH ROW EXECUTE FUNCTION tapescale_increment_cartridge_state_version();

ALTER TABLE media_pools
    ADD COLUMN details jsonb NOT NULL DEFAULT '{}',
    ADD COLUMN updated_at timestamptz NOT NULL DEFAULT now(),
    ADD CONSTRAINT media_pools_details_object CHECK (
        jsonb_typeof(details) = 'object' AND octet_length(details::text) <= 1048576
    );

-- Pool names are operator identifiers.  Prevent case-only duplicates even
-- when two creates race past the application-level friendly-name check.
-- Existing installations may already contain names that differ only by
-- letter case because the original schema enforced case-sensitive uniqueness.
-- Stop with a precise remediation instead of letting CREATE INDEX fail with
-- an opaque duplicate-key error.
DO $$
DECLARE
    conflicting_names text;
BEGIN
    SELECT string_agg(quoted_name, ', ' ORDER BY quoted_name)
    INTO conflicting_names
    FROM (
        SELECT quote_literal(lower(name)) AS quoted_name
        FROM media_pools
        GROUP BY lower(name)
        HAVING count(*) > 1
    ) AS duplicates;

    IF conflicting_names IS NOT NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = '23505',
            MESSAGE = format(
                'media pool names conflict when compared case-insensitively: %s',
                conflicting_names
            ),
            HINT = 'Rename the conflicting media_pools.name values so every name is unique ignoring case, then restart TapeScale to retry migration 0008.';
    END IF;
END;
$$;

CREATE UNIQUE INDEX media_pools_name_casefold_unique
    ON media_pools(lower(name));

ALTER TABLE holds
    ADD COLUMN created_by_label text NOT NULL DEFAULT 'system',
    ADD COLUMN created_correlation_id uuid NOT NULL DEFAULT gen_random_uuid(),
    ADD COLUMN released_by uuid REFERENCES users(id) ON DELETE RESTRICT,
    ADD COLUMN released_by_label text,
    ADD COLUMN release_reason text,
    ADD COLUMN release_correlation_id uuid;

UPDATE holds
SET released_by_label = 'legacy/system',
    release_reason = 'released before media-control history was introduced',
    release_correlation_id = gen_random_uuid()
WHERE released_at IS NOT NULL;

ALTER TABLE holds
    ADD CONSTRAINT holds_actor_labels_bounded CHECK (
        created_by_label <> '' AND octet_length(created_by_label) <= 256
        AND (released_by_label IS NULL OR (
            released_by_label <> '' AND octet_length(released_by_label) <= 256
        ))
    ),
    ADD CONSTRAINT holds_release_shape CHECK (
        (released_at IS NULL
            AND released_by IS NULL
            AND released_by_label IS NULL
            AND release_reason IS NULL
            AND release_correlation_id IS NULL)
        OR
        (released_at IS NOT NULL
            AND released_by_label IS NOT NULL
            AND release_reason IS NOT NULL
            AND release_reason <> ''
            AND octet_length(release_reason) <= 4096
            AND release_correlation_id IS NOT NULL)
    );

CREATE INDEX holds_active_cartridge
    ON holds(cartridge_id, created_at DESC)
    WHERE released_at IS NULL;

CREATE TABLE media_operations (
    id uuid PRIMARY KEY,
    cartridge_id uuid NOT NULL REFERENCES cartridges(id) ON DELETE RESTRICT,
    operation_kind text NOT NULL CHECK (operation_kind IN (
        'pool_assigned', 'hold_created', 'hold_released'
    )),
    idempotency_key text NOT NULL UNIQUE CHECK (
        idempotency_key <> '' AND octet_length(idempotency_key) <= 256
    ),
    request_fingerprint text NOT NULL CHECK (request_fingerprint ~ '^[0-9a-f]{64}$'),
    expected_state_version bigint NOT NULL CHECK (expected_state_version >= 0),
    resulting_state_version bigint NOT NULL CHECK (resulting_state_version > expected_state_version),
    object_id uuid,
    actor_id uuid REFERENCES users(id) ON DELETE RESTRICT,
    actor_label text NOT NULL CHECK (
        actor_label <> '' AND octet_length(actor_label) <= 256
    ),
    correlation_id uuid NOT NULL,
    result jsonb NOT NULL DEFAULT '{}' CHECK (
        jsonb_typeof(result) = 'object' AND octet_length(result::text) <= 1048576
    ),
    occurred_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX media_operations_cartridge_time
    ON media_operations(cartridge_id, occurred_at, id);

CREATE TRIGGER media_operations_append_only
    BEFORE UPDATE OR DELETE ON media_operations
    FOR EACH ROW EXECUTE FUNCTION tapescale_reject_history_mutation();

-- Media Control Center read paths are graph- and history-heavy.  These match
-- the recursive dependency walks, cartridge dossiers, and pool counts without
-- changing any existing uniqueness semantics.
CREATE INDEX cartridges_pool_id
    ON cartridges(pool_id);

CREATE INDEX volume_records_cartridge_recovery
    ON volume_records(cartridge_id, recovery_point_id);

CREATE INDEX recovery_dependencies_base_dependent
    ON recovery_dependencies(depends_on_id, recovery_point_id);

CREATE INDEX holds_active_recovery_point
    ON holds(recovery_point_id, created_at DESC)
    WHERE released_at IS NULL;

CREATE INDEX custody_events_cartridge_time
    ON custody_events(cartridge_id, occurred_at DESC, id);

CREATE INDEX verification_records_recovery_time
    ON verification_records(recovery_point_id, started_at DESC, id);

CREATE INDEX verification_records_media_ids
    ON verification_records USING gin(media_ids);
