-- Durable off-site media custody operations.
--
-- Keep the original movement_tasks.required_media_ids and custody_events shapes
-- intact for existing readers.  movement_task_media is the normalized mutable
-- projection used for exact, per-cartridge progress; custody_events remains the
-- append-only history of what an operator physically confirmed.

ALTER TABLE movement_tasks
    ADD COLUMN transit_location_id uuid REFERENCES locations(id) ON DELETE RESTRICT,
    ADD COLUMN idempotency_key text,
    ADD COLUMN request_fingerprint text,
    ADD COLUMN confirmation_version bigint NOT NULL DEFAULT 0 CHECK (confirmation_version >= 0),
    ADD COLUMN updated_at timestamptz NOT NULL DEFAULT now();

ALTER TABLE movement_tasks
    ADD CONSTRAINT movement_tasks_idempotency_shape CHECK (
        (idempotency_key IS NULL AND request_fingerprint IS NULL) OR
        (idempotency_key IS NOT NULL AND idempotency_key <> ''
            AND octet_length(idempotency_key) <= 256
            AND request_fingerprint ~ '^[0-9a-f]{64}$')
    );

CREATE UNIQUE INDEX movement_tasks_idempotency_key_unique
    ON movement_tasks(idempotency_key) WHERE idempotency_key IS NOT NULL;

CREATE TABLE movement_task_media (
    movement_task_id uuid NOT NULL REFERENCES movement_tasks(id) ON DELETE RESTRICT,
    cartridge_id uuid NOT NULL REFERENCES cartridges(id) ON DELETE RESTRICT,
    expected_source_location_id uuid REFERENCES locations(id) ON DELETE RESTRICT,
    destination_location_id uuid NOT NULL REFERENCES locations(id) ON DELETE RESTRICT,
    reservation_owner text NOT NULL CHECK (
        reservation_owner <> '' AND octet_length(reservation_owner) <= 128
    ),
    stage text NOT NULL DEFAULT 'planned' CHECK (stage IN (
        'planned', 'departed', 'arrived', 'direct_confirmed', 'cancelled', 'exception'
    )),
    latest_custody_event_id uuid REFERENCES custody_events(id) ON DELETE RESTRICT,
    last_confirmation_version bigint NOT NULL DEFAULT 0 CHECK (last_confirmation_version >= 0),
    departed_at timestamptz,
    arrived_at timestamptz,
    cancelled_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (movement_task_id, cartridge_id)
);

CREATE INDEX movement_task_media_cartridge_stage
    ON movement_task_media(cartridge_id, stage);

CREATE INDEX movement_task_media_task_stage
    ON movement_task_media(movement_task_id, stage);

-- Backfill legacy task arrays so new detail/list readers do not hide existing
-- movement tasks.  Legacy rows are not retroactively granted reservations.
INSERT INTO movement_task_media (
    movement_task_id,
    cartridge_id,
    expected_source_location_id,
    destination_location_id,
    reservation_owner,
    stage,
    departed_at,
    arrived_at,
    cancelled_at,
    updated_at
)
SELECT
    task.id,
    media.cartridge_id,
    task.source_location_id,
    task.destination_location_id,
    'movement:' || task.id::text,
    CASE
        WHEN task.state = 'confirmed' THEN 'arrived'
        WHEN task.state = 'cancelled' THEN 'cancelled'
        WHEN task.state = 'departed' THEN 'departed'
        ELSE 'planned'
    END,
    CASE WHEN task.state = 'departed' THEN task.created_at END,
    CASE WHEN task.state = 'confirmed' THEN task.created_at END,
    CASE WHEN task.state = 'cancelled' THEN task.created_at END,
    task.created_at
FROM movement_tasks AS task
CROSS JOIN LATERAL unnest(task.required_media_ids) AS media(cartridge_id)
ON CONFLICT (movement_task_id, cartridge_id) DO NOTHING;

CREATE TABLE movement_task_operations (
    id uuid PRIMARY KEY,
    movement_task_id uuid NOT NULL REFERENCES movement_tasks(id) ON DELETE RESTRICT,
    idempotency_key text NOT NULL CHECK (
        idempotency_key <> '' AND octet_length(idempotency_key) <= 256
    ),
    operation_kind text NOT NULL CHECK (operation_kind IN (
        'departure_confirmed', 'arrival_confirmed', 'direct_move_confirmed',
        'cancelled', 'custody_corrected'
    )),
    request_fingerprint text NOT NULL CHECK (request_fingerprint ~ '^[0-9a-f]{64}$'),
    expected_confirmation_version bigint NOT NULL CHECK (expected_confirmation_version >= 0),
    resulting_confirmation_version bigint NOT NULL CHECK (resulting_confirmation_version > 0),
    resulting_task_state text NOT NULL CHECK (resulting_task_state IN (
        'planned', 'ready', 'departed', 'partially_confirmed', 'confirmed',
        'overdue', 'blocked', 'cancelled'
    )),
    affected_media_ids uuid[] NOT NULL CHECK (cardinality(affected_media_ids) > 0),
    actor_id uuid REFERENCES users(id) ON DELETE RESTRICT,
    actor_label text NOT NULL CHECK (actor_label <> '' AND octet_length(actor_label) <= 256),
    correlation_id uuid NOT NULL,
    result jsonb NOT NULL DEFAULT '{}' CHECK (octet_length(result::text) <= 1048576),
    occurred_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (movement_task_id, idempotency_key)
);

CREATE INDEX movement_task_operations_task_time
    ON movement_task_operations(movement_task_id, occurred_at, id);

CREATE TRIGGER movement_task_operations_append_only
    BEFORE UPDATE OR DELETE ON movement_task_operations
    FOR EACH ROW EXECUTE FUNCTION tapescale_reject_history_mutation();
