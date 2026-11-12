-- TapeScale durable control-plane schema.
-- Engine-owned catalog tables are intentionally absent: integrations use supported engine APIs.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE installations (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name text NOT NULL,
    site_timezone text NOT NULL DEFAULT 'UTC',
    mode text NOT NULL DEFAULT 'production' CHECK (mode IN ('production', 'simulation')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE users (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    username text NOT NULL UNIQUE,
    display_name text NOT NULL,
    password_hash text NOT NULL,
    mfa_required boolean NOT NULL DEFAULT true,
    disabled_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE roles (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name text NOT NULL UNIQUE CHECK (name IN ('administrator', 'operator', 'viewer')),
    permissions text[] NOT NULL DEFAULT '{}'
);

CREATE TABLE user_roles (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role_id uuid NOT NULL REFERENCES roles(id) ON DELETE RESTRICT,
    PRIMARY KEY (user_id, role_id)
);

CREATE TABLE sessions (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_digest bytea NOT NULL UNIQUE,
    reauthenticated_at timestamptz,
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE truenas_instances (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name text NOT NULL,
    websocket_url text NOT NULL,
    certificate_fingerprint text NOT NULL,
    api_version text,
    software_version text,
    credential_secret_ref text NOT NULL,
    last_seen_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE sources (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    truenas_instance_id uuid NOT NULL REFERENCES truenas_instances(id) ON DELETE RESTRICT,
    dataset_guid text NOT NULL,
    dataset_name text NOT NULL,
    source_type text NOT NULL CHECK (source_type IN ('filesystem', 'zvol')),
    logical_root text NOT NULL,
    selection jsonb NOT NULL,
    scope_hash text NOT NULL,
    enabled boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (truenas_instance_id, dataset_guid)
);

CREATE TABLE snapshots (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id uuid NOT NULL REFERENCES sources(id) ON DELETE RESTRICT,
    snapshot_guid text NOT NULL,
    snapshot_name text NOT NULL,
    physical_path text,
    logical_root text NOT NULL,
    consistency text NOT NULL CHECK (consistency IN ('crash_consistent', 'application_consistent')),
    application_owned boolean NOT NULL,
    hold_tag text,
    created_at timestamptz NOT NULL,
    released_at timestamptz,
    UNIQUE (source_id, snapshot_guid)
);

CREATE TABLE media_pools (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name text NOT NULL UNIQUE,
    purpose text NOT NULL CHECK (purpose IN ('routine', 'archive', 'scratch', 'cleaning')),
    indefinite_retention boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE locations (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name text NOT NULL UNIQUE,
    kind text NOT NULL CHECK (kind IN ('onsite', 'offsite', 'in_transit', 'unknown', 'device')),
    details jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE changers (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    stable_id text NOT NULL UNIQUE,
    vendor text,
    model text,
    serial text,
    wwn text,
    firmware text,
    observed_path text,
    reconciled boolean NOT NULL DEFAULT false,
    last_observed_at timestamptz,
    capabilities jsonb NOT NULL DEFAULT '{}'
);

CREATE TABLE drives (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    changer_id uuid REFERENCES changers(id) ON DELETE SET NULL,
    stable_id text NOT NULL UNIQUE,
    vendor text,
    model text,
    serial text,
    wwn text,
    firmware text,
    generation smallint NOT NULL CHECK (generation BETWEEN 1 AND 10),
    observed_path text,
    expected_path text,
    reconciled boolean NOT NULL DEFAULT false,
    qualification_status text NOT NULL DEFAULT 'detected_unqualified'
        CHECK (qualification_status IN ('simulated', 'detected_unqualified', 'physically_qualified')),
    health jsonb NOT NULL DEFAULT '{}',
    last_observed_at timestamptz
);

CREATE TABLE slots (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    changer_id uuid NOT NULL REFERENCES changers(id) ON DELETE CASCADE,
    element_address integer NOT NULL,
    kind text NOT NULL CHECK (kind IN ('storage', 'drive', 'import_export')),
    observed_barcode text,
    observed_at timestamptz,
    UNIQUE (changer_id, element_address)
);

CREATE TABLE cartridges (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    internal_label text NOT NULL UNIQUE,
    barcode text,
    media_generation smallint CHECK (media_generation BETWEEN 1 AND 10),
    media_format text,
    on_tape_identity text,
    identity_status text NOT NULL DEFAULT 'unknown'
        CHECK (identity_status IN ('unknown', 'foreign', 'reconciled', 'mismatch')),
    pool_id uuid REFERENCES media_pools(id) ON DELETE RESTRICT,
    confirmed_location_id uuid REFERENCES locations(id) ON DELETE RESTRICT,
    observed_location jsonb NOT NULL DEFAULT '{}',
    health_status text NOT NULL DEFAULT 'unknown',
    worm boolean NOT NULL DEFAULT false,
    reserved_by text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX cartridges_barcode_unique ON cartridges(barcode) WHERE barcode IS NOT NULL;

CREATE TABLE protection_policies (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name text NOT NULL UNIQUE,
    source_id uuid NOT NULL REFERENCES sources(id) ON DELETE RESTRICT,
    representation text NOT NULL CHECK (representation IN ('file', 'native_zfs', 'both')),
    schedule_timezone text NOT NULL,
    full_schedule text NOT NULL,
    incremental_schedule text NOT NULL,
    retention_days integer NOT NULL DEFAULT 28 CHECK (retention_days > 0),
    encryption_enabled boolean NOT NULL DEFAULT true,
    key_lineage_id text,
    compression_mode text NOT NULL DEFAULT 'auto',
    verification_mode text NOT NULL DEFAULT 'full_readback',
    config jsonb NOT NULL DEFAULT '{}',
    enabled boolean NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE jobs (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    external_id text NOT NULL UNIQUE,
    policy_id uuid REFERENCES protection_policies(id) ON DELETE RESTRICT,
    kind text NOT NULL CHECK (kind IN ('backup', 'verify', 'restore', 'inventory', 'rotation', 'recovery_export')),
    priority integer NOT NULL DEFAULT 100,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE job_runs (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    job_id uuid NOT NULL REFERENCES jobs(id) ON DELETE RESTRICT,
    scheduled_occurrence text,
    attempt integer NOT NULL DEFAULT 1 CHECK (attempt > 0),
    state text NOT NULL CHECK (state IN (
        'queued', 'preparing', 'waiting_for_media', 'writing', 'written', 'verifying', 'verified',
        'blocked', 'interrupted', 'failed', 'cancelled', 'completed'
    )),
    correlation_id uuid NOT NULL,
    source_snapshot_id uuid REFERENCES snapshots(id) ON DELETE RESTRICT,
    engine_operation_id text,
    failure_class text,
    failure_detail jsonb,
    started_at timestamptz,
    finished_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (job_id, scheduled_occurrence)
);

CREATE INDEX job_runs_state_priority ON job_runs(state, created_at) WHERE finished_at IS NULL;

CREATE TABLE job_transitions (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    run_id uuid NOT NULL REFERENCES job_runs(id) ON DELETE RESTRICT,
    from_state text,
    to_state text NOT NULL,
    reason text NOT NULL,
    evidence jsonb NOT NULL DEFAULT '{}',
    actor_id uuid REFERENCES users(id) ON DELETE RESTRICT,
    correlation_id uuid NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX job_transitions_run_time ON job_transitions(run_id, occurred_at);

CREATE TABLE recovery_points (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id uuid NOT NULL REFERENCES sources(id) ON DELETE RESTRICT,
    run_id uuid NOT NULL UNIQUE REFERENCES job_runs(id) ON DELETE RESTRICT,
    representation text NOT NULL CHECK (representation IN ('file', 'native_zfs')),
    level text NOT NULL CHECK (level IN ('full', 'incremental')),
    source_scope_hash text NOT NULL,
    format_version text NOT NULL,
    key_lineage_id text,
    status text NOT NULL CHECK (status IN ('incomplete', 'written', 'verified', 'failed')),
    recovery_time timestamptz NOT NULL,
    expires_at timestamptz,
    indefinite boolean NOT NULL DEFAULT false,
    coverage jsonb NOT NULL,
    manifest_digest text,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE recovery_dependencies (
    recovery_point_id uuid NOT NULL REFERENCES recovery_points(id) ON DELETE RESTRICT,
    depends_on_id uuid NOT NULL REFERENCES recovery_points(id) ON DELETE RESTRICT,
    dependency_kind text NOT NULL,
    PRIMARY KEY (recovery_point_id, depends_on_id),
    CHECK (recovery_point_id <> depends_on_id)
);

CREATE TABLE volume_records (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    recovery_point_id uuid NOT NULL REFERENCES recovery_points(id) ON DELETE RESTRICT,
    cartridge_id uuid NOT NULL REFERENCES cartridges(id) ON DELETE RESTRICT,
    volume_order integer NOT NULL CHECK (volume_order > 0),
    first_record text NOT NULL,
    last_record text,
    encoded_bytes bigint CHECK (encoded_bytes >= 0),
    finalized boolean NOT NULL DEFAULT false,
    engine_record_id text NOT NULL,
    UNIQUE (recovery_point_id, volume_order),
    UNIQUE (engine_record_id)
);

CREATE TABLE holds (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    cartridge_id uuid REFERENCES cartridges(id) ON DELETE RESTRICT,
    recovery_point_id uuid REFERENCES recovery_points(id) ON DELETE RESTRICT,
    kind text NOT NULL CHECK (kind IN ('dependency', 'archive', 'legal', 'manual', 'active_job', 'unknown_identity')),
    reason text NOT NULL,
    expires_at timestamptz,
    released_at timestamptz,
    created_by uuid REFERENCES users(id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (cartridge_id IS NOT NULL OR recovery_point_id IS NOT NULL)
);

CREATE TABLE verification_records (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    recovery_point_id uuid NOT NULL REFERENCES recovery_points(id) ON DELETE RESTRICT,
    kind text NOT NULL CHECK (kind IN ('write_complete', 'full_readback', 'sample', 'restore_test')),
    scope jsonb NOT NULL,
    outcome text NOT NULL CHECK (outcome IN ('running', 'passed', 'failed', 'interrupted')),
    drive_id uuid REFERENCES drives(id) ON DELETE RESTRICT,
    media_ids uuid[] NOT NULL DEFAULT '{}',
    evidence jsonb NOT NULL DEFAULT '{}',
    correlation_id uuid NOT NULL,
    started_at timestamptz NOT NULL,
    finished_at timestamptz
);

CREATE TABLE movement_tasks (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    external_id text NOT NULL UNIQUE,
    kind text NOT NULL CHECK (kind IN ('take_offsite', 'return', 'swap', 'recall')),
    source_location_id uuid REFERENCES locations(id) ON DELETE RESTRICT,
    destination_location_id uuid NOT NULL REFERENCES locations(id) ON DELETE RESTRICT,
    required_media_ids uuid[] NOT NULL,
    state text NOT NULL CHECK (state IN ('planned', 'ready', 'departed', 'partially_confirmed', 'confirmed', 'overdue', 'blocked', 'cancelled')),
    readiness jsonb NOT NULL DEFAULT '{}',
    assigned_user_id uuid REFERENCES users(id) ON DELETE RESTRICT,
    due_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE custody_events (
    id uuid PRIMARY KEY,
    movement_task_id uuid REFERENCES movement_tasks(id) ON DELETE RESTRICT,
    movement_task_external_id text,
    cartridge_id uuid REFERENCES cartridges(id) ON DELETE RESTRICT,
    actor_id uuid REFERENCES users(id) ON DELETE RESTRICT,
    actor_label text NOT NULL,
    event_type text NOT NULL DEFAULT 'arrival_confirmed',
    source_location_id uuid REFERENCES locations(id) ON DELETE RESTRICT,
    destination_location_id uuid REFERENCES locations(id) ON DELETE RESTRICT,
    destination_label text,
    tape_ids text[] NOT NULL DEFAULT '{}',
    note text NOT NULL DEFAULT '',
    corrects_event_id uuid REFERENCES custody_events(id) ON DELETE RESTRICT,
    evidence jsonb NOT NULL DEFAULT '{}',
    occurred_at timestamptz NOT NULL,
    correlation_id uuid NOT NULL
);

CREATE TABLE restore_plans (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    requested_by uuid REFERENCES users(id) ON DELETE RESTRICT,
    selection jsonb NOT NULL,
    dependency_chain uuid[] NOT NULL,
    required_media_ids uuid[] NOT NULL,
    key_lineage_ids text[] NOT NULL,
    destination jsonb NOT NULL,
    conflict_policy text NOT NULL DEFAULT 'skip' CHECK (conflict_policy IN ('skip', 'rename', 'replace')),
    blockers jsonb NOT NULL DEFAULT '[]',
    estimates jsonb NOT NULL DEFAULT '{}',
    preview_hash text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL
);

CREATE TABLE resource_leases (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    resource_kind text NOT NULL CHECK (resource_kind IN ('drive', 'changer', 'media', 'source', 'spool')),
    resource_id text NOT NULL,
    owner_id uuid NOT NULL,
    fencing_token bigint NOT NULL,
    acquired_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    released_at timestamptz
);

CREATE UNIQUE INDEX resource_leases_one_active
    ON resource_leases(resource_kind, resource_id) WHERE released_at IS NULL;

CREATE TABLE audit_events (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    actor_id uuid REFERENCES users(id) ON DELETE RESTRICT,
    actor_label text NOT NULL,
    action text NOT NULL,
    object_type text NOT NULL,
    object_id text NOT NULL,
    requested jsonb NOT NULL DEFAULT '{}',
    result text NOT NULL,
    correlation_id uuid NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX audit_events_object_time ON audit_events(object_type, object_id, occurred_at);

CREATE TABLE notification_outbox (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    event_key text NOT NULL UNIQUE,
    channel text NOT NULL CHECK (channel IN ('in_app', 'email', 'webhook')),
    payload jsonb NOT NULL,
    available_at timestamptz NOT NULL DEFAULT now(),
    attempts integer NOT NULL DEFAULT 0,
    delivered_at timestamptz,
    last_error text
);

CREATE TABLE component_versions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    component text NOT NULL,
    version text NOT NULL,
    implementation text NOT NULL,
    license text,
    qualification_status text NOT NULL,
    evidence jsonb NOT NULL DEFAULT '{}',
    observed_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (component, version)
);

CREATE TABLE preflight_runs (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    status text NOT NULL CHECK (status IN ('passed', 'attention', 'blocked')),
    checks jsonb NOT NULL,
    correlation_id uuid NOT NULL,
    started_at timestamptz NOT NULL,
    finished_at timestamptz NOT NULL
);

CREATE TABLE recovery_bundles (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    format_version text NOT NULL,
    catalog_export_digest text NOT NULL,
    key_package_reference text NOT NULL,
    media_map_digest text NOT NULL,
    independently_verified_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE OR REPLACE FUNCTION tapescale_reject_history_mutation()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION '% is append-only', TG_TABLE_NAME;
END;
$$;

CREATE TRIGGER job_transitions_append_only
    BEFORE UPDATE OR DELETE ON job_transitions
    FOR EACH ROW EXECUTE FUNCTION tapescale_reject_history_mutation();

CREATE TRIGGER custody_events_append_only
    BEFORE UPDATE OR DELETE ON custody_events
    FOR EACH ROW EXECUTE FUNCTION tapescale_reject_history_mutation();

CREATE TRIGGER verification_records_append_only
    BEFORE UPDATE OR DELETE ON verification_records
    FOR EACH ROW EXECUTE FUNCTION tapescale_reject_history_mutation();

CREATE TRIGGER audit_events_append_only
    BEFORE UPDATE OR DELETE ON audit_events
    FOR EACH ROW EXECUTE FUNCTION tapescale_reject_history_mutation();

INSERT INTO roles (name, permissions) VALUES
    ('administrator', ARRAY['*']),
    ('operator', ARRAY['jobs:run','jobs:cancel','media:handle','movement:confirm','restore:approved_destination']),
    ('viewer', ARRAY['inventory:read','status:read','verification:read']);
