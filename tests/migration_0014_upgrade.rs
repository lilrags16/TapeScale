use std::{borrow::Cow, time::Duration};

use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use url::Url;
use uuid::Uuid;

#[tokio::test]
async fn multivolume_restore_upgrade_adds_ordered_ledger_and_waiting_state() {
    let Ok(database_url) = std::env::var("TAPESCALE_TEST_DATABASE_URL") else {
        eprintln!("TAPESCALE_TEST_DATABASE_URL is not set; skipping PostgreSQL migration test");
        return;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&database_url)
        .await
        .expect("migration test database should connect");
    let schema = format!("migration_0014_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("isolated migration schema should be created");
    let mut isolated_url = Url::parse(&database_url).expect("database URL should parse");
    isolated_url
        .query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema},public"));
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(10))
        .connect(isolated_url.as_str())
        .await
        .expect("isolated migration database should connect");

    let all = sqlx::migrate!("./migrations");
    let through_0013 = Migrator {
        migrations: Cow::Owned(
            all.iter()
                .filter(|migration| migration.version <= 13)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    through_0013
        .run(&pool)
        .await
        .expect("legacy schema should migrate through 0013");

    sqlx::raw_sql(
        r#"
        INSERT INTO truenas_instances
            (id, name, websocket_url, certificate_fingerprint, credential_secret_ref)
        VALUES
            ('00000000-0000-0000-0000-000000000101', '0014 fixture',
             'wss://fixture.invalid', 'fixture', 'fixture');

        INSERT INTO sources
            (id, truenas_instance_id, dataset_guid, dataset_name, source_type,
             logical_root, selection, scope_hash)
        VALUES
            ('00000000-0000-0000-0000-000000000102',
             '00000000-0000-0000-0000-000000000101',
             'dataset-0014', 'pool/fixture', 'filesystem',
             '/fixture', '{}', 'fixture-scope');

        INSERT INTO snapshots
            (id, source_id, snapshot_guid, snapshot_name, physical_path,
             logical_root, consistency, application_owned, created_at)
        VALUES
            ('00000000-0000-0000-0000-000000000103',
             '00000000-0000-0000-0000-000000000102',
             'snapshot-0014', 'fixture', '/source/fixture',
             '/fixture', 'crash_consistent', false, now());

        INSERT INTO jobs (id, external_id, kind)
        VALUES
            ('00000000-0000-0000-0000-000000000104', '0014-backup', 'backup'),
            ('00000000-0000-0000-0000-000000000105', '0014-running-restore', 'restore'),
            ('00000000-0000-0000-0000-000000000106', '0014-empty-restore', 'restore');

        INSERT INTO drives
            (id, stable_id, generation, observed_path, expected_path, reconciled)
        VALUES
            ('00000000-0000-0000-0000-000000000110',
             '0014-drive', 9, '/dev/nst0', '/dev/nst0', true);

        INSERT INTO cartridges
            (id, internal_label, media_generation, media_format, on_tape_identity)
        VALUES
            ('00000000-0000-0000-0000-000000000111',
             repeat('é', 129) || E'\n', 9, 'LTO-9', repeat('Ω', 80) || E'\t'),
            ('00000000-0000-0000-0000-000000000112',
             '', 9, 'LTO-9', 'EMPTYVOL');

        INSERT INTO job_runs
            (id, job_id, state, correlation_id, source_snapshot_id,
             started_at, finished_at)
        VALUES
            ('00000000-0000-0000-0000-000000000121',
             '00000000-0000-0000-0000-000000000104', 'completed',
             '00000000-0000-0000-0000-000000000221',
             '00000000-0000-0000-0000-000000000103', now(), now()),
            ('00000000-0000-0000-0000-000000000122',
             '00000000-0000-0000-0000-000000000105', 'restoring',
             '00000000-0000-0000-0000-000000000222',
             '00000000-0000-0000-0000-000000000103', now(), NULL),
            ('00000000-0000-0000-0000-000000000123',
             '00000000-0000-0000-0000-000000000106', 'completed',
             '00000000-0000-0000-0000-000000000223',
             '00000000-0000-0000-0000-000000000103', now(), now());

        INSERT INTO single_tape_data_profiles
            (id, job_id, source_id, source_snapshot_id, name,
             source_subpath, restore_subpath, source_mount_path,
             restore_root_path, logical_root, immutable_snapshot_confirmed)
        VALUES
            ('00000000-0000-0000-0000-000000000130',
             '00000000-0000-0000-0000-000000000104',
             '00000000-0000-0000-0000-000000000102',
             '00000000-0000-0000-0000-000000000103',
             '0014 fixture profile', 'fixture', 'fixture-restore',
             '/source/fixture', '/restore/fixture', '/fixture', true);

        INSERT INTO single_tape_data_runs
            (id, profile_id, job_run_id, source_snapshot_id, drive_id, cartridge_id,
             expected_stable_id, expected_path, expected_cartridge_label,
             expected_media_generation, expected_media_format,
             source_mount_path, restore_root_path, logical_root,
             engine_name, engine_version, adapter_version, format_version,
             scratch_media_confirmed, destructive_overwrite_confirmed,
             encryption_deviation_confirmed, state, requested_by_label,
             correlation_id, started_at, finished_at)
        VALUES
            ('00000000-0000-0000-0000-000000000131',
             '00000000-0000-0000-0000-000000000130',
             '00000000-0000-0000-0000-000000000121',
             '00000000-0000-0000-0000-000000000103',
             '00000000-0000-0000-0000-000000000110',
             '00000000-0000-0000-0000-000000000111',
             '0014-drive', '/dev/nst0', repeat('é', 129) || E'\n',
             9, 'LTO-9', '/source/fixture', '/restore/fixture', '/fixture',
             'Bacula Community', '15.0.3', 'g0.2-lab-v1',
             'bacula-community-15.0.3-lab-v1',
             true, true, true, 'passed', 'fixture',
             '00000000-0000-0000-0000-000000000231', now(), now());

        INSERT INTO single_tape_data_artifacts
            (single_tape_data_run_id, artifact_kind, storage_ref,
             sha256, size_bytes, metadata)
        VALUES
            ('00000000-0000-0000-0000-000000000131',
             'source_manifest', '/fixture/manifest.json',
             repeat('a', 64), 1, '{}');

        INSERT INTO recovery_points
            (id, source_id, run_id, representation, level, source_scope_hash,
             format_version, status, recovery_time, coverage, manifest_digest)
        VALUES
            ('00000000-0000-0000-0000-000000000132',
             '00000000-0000-0000-0000-000000000102',
             '00000000-0000-0000-0000-000000000121',
             'file', 'full', 'fixture-scope',
             'bacula-community-15.0.3-lab-v1', 'verified',
             now(), '{}', repeat('b', 64));

        INSERT INTO volume_records
            (id, recovery_point_id, cartridge_id, volume_order,
             first_record, last_record, encoded_bytes, finalized, engine_record_id)
        VALUES
            ('00000000-0000-0000-0000-000000000133',
             '00000000-0000-0000-0000-000000000132',
             '00000000-0000-0000-0000-000000000111',
             1, '', NULL, 1, true, 'legacy-record');

        INSERT INTO restore_plans
            (id, selection, dependency_chain, required_media_ids,
             key_lineage_ids, destination, blockers, estimates,
             preview_hash, expires_at)
        VALUES
            ('00000000-0000-0000-0000-000000000141',
             '{}', ARRAY['00000000-0000-0000-0000-000000000132'::uuid],
             ARRAY['00000000-0000-0000-0000-000000000111'::uuid],
             '{}', '{}', '[]', '{}', repeat('c', 64), now() + interval '1 day'),
            ('00000000-0000-0000-0000-000000000142',
             '{}', ARRAY['00000000-0000-0000-0000-000000000132'::uuid],
             ARRAY['00000000-0000-0000-0000-000000000112'::uuid],
             '{}', '{}', '[]', '{}', repeat('d', 64), now() + interval '1 day');

        INSERT INTO restore_rehearsals
            (id, restore_plan_id, job_id, job_run_id, recovery_point_id,
             original_data_run_id, drive_id, cartridge_id,
             expected_stable_id, expected_path,
             expected_volume_label, expected_media_generation,
             expected_media_format, expected_on_tape_identity,
             backup_job_id, expected_engine_media_id, expected_engine_record_id,
             source_manifest_artifact_id, source_manifest_storage_ref,
             source_manifest_sha256, source_manifest_digest,
             source_manifest_size_bytes, source_mount_path, logical_root,
             destination_subpath, estimated_required_bytes, available_bytes,
             required_free_reserve_bytes, preview_hash, state, phase,
             requested_by_label, correlation_id, started_at, finished_at)
        VALUES
            ('00000000-0000-0000-0000-000000000151',
             '00000000-0000-0000-0000-000000000141',
             '00000000-0000-0000-0000-000000000105',
             '00000000-0000-0000-0000-000000000122',
             '00000000-0000-0000-0000-000000000132',
             '00000000-0000-0000-0000-000000000131',
             '00000000-0000-0000-0000-000000000110',
             '00000000-0000-0000-0000-000000000111',
             '0014-drive', '/dev/nst0',
             repeat('V', 140) || E'\n', 9, 'LTO-9',
             repeat('Ω', 80) || E'\t', 1, 1, 'legacy-record',
             (SELECT id FROM single_tape_data_artifacts
               WHERE single_tape_data_run_id =
                     '00000000-0000-0000-0000-000000000131'
                 AND artifact_kind = 'source_manifest'),
             '/fixture/manifest.json', repeat('a', 64), repeat('b', 64),
             1, '/source/fixture', '/fixture', 'running-restore',
             0, 1, 0, repeat('c', 64), 'running', 'restoring',
             'fixture', '00000000-0000-0000-0000-000000000251',
             now(), NULL),
            ('00000000-0000-0000-0000-000000000152',
             '00000000-0000-0000-0000-000000000142',
             '00000000-0000-0000-0000-000000000106',
             '00000000-0000-0000-0000-000000000123',
             '00000000-0000-0000-0000-000000000132',
             '00000000-0000-0000-0000-000000000131',
             '00000000-0000-0000-0000-000000000110',
             '00000000-0000-0000-0000-000000000112',
             '0014-drive', '/dev/nst0', 'EMPTYVOL', 9, 'LTO-9',
             'EMPTYVOL', 1, 1, 'legacy-record',
             (SELECT id FROM single_tape_data_artifacts
               WHERE single_tape_data_run_id =
                     '00000000-0000-0000-0000-000000000131'
                 AND artifact_kind = 'source_manifest'),
             '/fixture/manifest.json', repeat('a', 64), repeat('b', 64),
             1, '/source/fixture', '/fixture', 'passed-restore',
             0, 1, 0, repeat('d', 64), 'passed', 'passed',
             'fixture', '00000000-0000-0000-0000-000000000252',
             now(), now());
        "#,
    )
    .execute(&pool)
    .await
    .expect("pre-0014 schema-valid legacy restore rows should be inserted");

    all.run(&pool)
        .await
        .expect("0014 should add the attended multi-volume restore contract");

    for table in [
        "restore_rehearsal_volumes",
        "restore_rehearsal_media_requests",
    ] {
        let exists = sqlx::query_scalar::<_, bool>("SELECT to_regclass($1) IS NOT NULL")
            .bind(table)
            .fetch_one(&pool)
            .await
            .expect("restore contract table lookup should succeed");
        assert!(exists, "{table} should exist after migration 0014");
    }
    let run_columns = sqlx::query_scalar::<_, String>(
        "SELECT column_name FROM information_schema.columns
         WHERE table_schema = current_schema()
           AND table_name = 'restore_rehearsals'
           AND column_name IN ('workflow_kind', 'volume_count')
         ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await
    .expect("new restore run columns should be queryable");
    assert_eq!(run_columns, vec!["volume_count", "workflow_kind"]);

    let phase_constraint = sqlx::query_scalar::<_, String>(
        "SELECT pg_get_constraintdef(oid)
         FROM pg_constraint
         WHERE conrelid = 'restore_rehearsals'::regclass
           AND conname = 'restore_rehearsals_phase_check'",
    )
    .fetch_one(&pool)
    .await
    .expect("restore phase constraint should exist");
    assert!(phase_constraint.contains("waiting_for_media"));
    let open_request_index = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM pg_indexes
             WHERE schemaname = current_schema()
               AND indexname = 'restore_rehearsal_one_open_media_request'
               AND indexdef LIKE '%pending%authorized%loaded%')",
    )
    .fetch_one(&pool)
    .await
    .expect("open-request index should be inspectable");
    assert!(open_request_index);

    let running_legacy = sqlx::query_as::<_, (String, String, String, String, bool, bool, bool)>(
        "SELECT expected_cartridge_label, expected_volume_label,
                expected_on_tape_identity, expected_first_record,
                active, media_lease_id IS NULL, loaded_at IS NULL
         FROM restore_rehearsal_volumes
         WHERE restore_rehearsal_id = '00000000-0000-0000-0000-000000000151'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy running restore should have a backfilled ledger row");
    assert_eq!(running_legacy.0, format!("{}\n", "é".repeat(129)));
    assert_eq!(running_legacy.1, format!("{}\n", "V".repeat(140)));
    assert_eq!(running_legacy.2, format!("{}\t", "Ω".repeat(80)));
    assert_eq!(running_legacy.3, "");
    assert!(running_legacy.4);
    assert!(running_legacy.5);
    assert!(running_legacy.6);
    let running_state = sqlx::query_scalar::<_, String>(
        "SELECT state FROM restore_rehearsal_volumes
         WHERE restore_rehearsal_id = '00000000-0000-0000-0000-000000000151'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy running restore state should be queryable");
    assert_eq!(running_state, "reserved");

    let empty_label_legacy = sqlx::query_as::<_, (String, String, bool)>(
        "SELECT expected_cartridge_label, state, active
         FROM restore_rehearsal_volumes
         WHERE restore_rehearsal_id = '00000000-0000-0000-0000-000000000152'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy empty cartridge label should survive ledger backfill");
    assert_eq!(empty_label_legacy.0, "");
    assert_eq!(empty_label_legacy.1, "completed");
    assert!(!empty_label_legacy.2);

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("isolated migration schema should be removed");
    admin.close().await;
}
