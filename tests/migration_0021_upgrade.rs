use std::{borrow::Cow, time::Duration};

use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use url::Url;
use uuid::Uuid;

#[tokio::test]
async fn rolling_native_upgrade_rebinds_only_untouched_work_and_preserves_v1_history() {
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
    let schema = format!("migration_0021_{}", Uuid::now_v7().simple());
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
    let through_0020 = Migrator {
        migrations: Cow::Owned(
            all.iter()
                .filter(|migration| migration.version <= 20)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    through_0020
        .run(&pool)
        .await
        .expect("Native v1 schema should migrate through 0020");

    sqlx::raw_sql(
        r#"
        INSERT INTO truenas_instances
            (id, name, websocket_url, certificate_fingerprint, credential_secret_ref)
        VALUES
            ('00000000-0000-0000-0000-000000002101', '0021 fixture',
             'wss://fixture.invalid', 'fixture', 'fixture');
        INSERT INTO sources
            (id, truenas_instance_id, dataset_guid, dataset_name, source_type,
             logical_root, selection, scope_hash)
        VALUES
            ('00000000-0000-0000-0000-000000002102',
             '00000000-0000-0000-0000-000000002101', 'dataset-0021',
             'pool/fixture', 'filesystem', '/fixture', '{}', 'fixture-scope');
        INSERT INTO snapshots
            (id, source_id, snapshot_guid, snapshot_name, physical_path,
             logical_root, consistency, application_owned, created_at)
        VALUES
            ('00000000-0000-0000-0000-000000002103',
             '00000000-0000-0000-0000-000000002102', 'snapshot-0021',
             'fixture', '/source/fixture', '/fixture', 'crash_consistent', false, now());
        INSERT INTO protection_policies
            (id, name, source_id, representation, schedule_timezone,
             full_schedule, incremental_schedule, encryption_enabled, enabled)
        VALUES
            ('00000000-0000-0000-0000-000000002114', '0021 backup policy',
             '00000000-0000-0000-0000-000000002102', 'file', 'UTC',
             'manual', 'disabled', false, true);
        INSERT INTO jobs (id, external_id, kind, policy_id)
        VALUES
            ('00000000-0000-0000-0000-000000002104', '0021-native', 'backup',
             '00000000-0000-0000-0000-000000002114'),
            ('00000000-0000-0000-0000-000000002115', '0021-g02', 'backup', NULL);
        INSERT INTO single_tape_data_profiles
            (id, job_id, source_id, source_snapshot_id, name, source_subpath,
             restore_subpath, source_mount_path, restore_root_path, logical_root,
             immutable_snapshot_confirmed, workflow_kind, policy_id)
        VALUES
            ('00000000-0000-0000-0000-000000002105',
             '00000000-0000-0000-0000-000000002104',
             '00000000-0000-0000-0000-000000002102',
             '00000000-0000-0000-0000-000000002103', '0021 native profile',
             'fixture', 'fixture-restore', '/source/fixture', '/restore/fixture',
             '/fixture', true, 'backup',
             '00000000-0000-0000-0000-000000002114'),
            ('00000000-0000-0000-0000-000000002116',
             '00000000-0000-0000-0000-000000002115',
             '00000000-0000-0000-0000-000000002102',
             '00000000-0000-0000-0000-000000002103', '0021 G0.2 profile',
             'fixture', 'fixture-restore', '/source/fixture', '/restore/fixture',
             '/fixture', true, 'recovery_exercise', NULL);
        INSERT INTO drives
            (id, stable_id, generation, observed_path, expected_path, reconciled)
        VALUES
            ('00000000-0000-0000-0000-000000002106', '0021-drive', 9,
             '/dev/nst0', '/dev/nst0', true);
        INSERT INTO cartridges
            (id, internal_label, media_generation, media_format)
        VALUES
            ('00000000-0000-0000-0000-000000002107', 'NATIVE021', 9, 'LTO-9');
        INSERT INTO job_runs
            (id, job_id, state, correlation_id, source_snapshot_id, finished_at)
        VALUES
            ('00000000-0000-0000-0000-000000002108',
             '00000000-0000-0000-0000-000000002104', 'queued',
             '00000000-0000-0000-0000-000000002118',
             '00000000-0000-0000-0000-000000002103', NULL),
            ('00000000-0000-0000-0000-000000002109',
             '00000000-0000-0000-0000-000000002104', 'failed',
             '00000000-0000-0000-0000-000000002119',
             '00000000-0000-0000-0000-000000002103', now());
        INSERT INTO single_tape_data_runs
            (id, profile_id, job_run_id, source_snapshot_id, drive_id, cartridge_id,
             expected_stable_id, expected_path, expected_cartridge_label,
             expected_media_generation, expected_media_format, source_mount_path,
             restore_root_path, logical_root, engine_name, engine_version,
             adapter_version, format_version, scratch_media_confirmed,
             destructive_overwrite_confirmed, encryption_deviation_confirmed,
             workflow_kind, trigger_kind, state, requested_by_label,
             correlation_id, media_write_started,
             media_write_started_at, started_at, finished_at)
        VALUES
            ('00000000-0000-0000-0000-000000002110',
             '00000000-0000-0000-0000-000000002105',
             '00000000-0000-0000-0000-000000002108',
             '00000000-0000-0000-0000-000000002103',
             '00000000-0000-0000-0000-000000002106',
             '00000000-0000-0000-0000-000000002107', '0021-drive', '/dev/nst0',
             'NATIVE021', 9, 'LTO-9', '/source/fixture', '/restore/fixture',
             '/fixture', 'TapeScale Native', '1', 'native-v1',
             'tapescale-native-v1', true, true, true, 'backup', 'manual',
             'queued', 'untouched',
             '00000000-0000-0000-0000-000000002120', false, NULL, NULL, NULL),
            ('00000000-0000-0000-0000-000000002111',
             '00000000-0000-0000-0000-000000002105',
             '00000000-0000-0000-0000-000000002109',
             '00000000-0000-0000-0000-000000002103',
             '00000000-0000-0000-0000-000000002106',
             '00000000-0000-0000-0000-000000002107', '0021-drive', '/dev/nst0',
             'NATIVE021', 9, 'LTO-9', '/source/fixture', '/restore/fixture',
             '/fixture', 'TapeScale Native', '1', 'native-v1',
             'tapescale-native-v1', true, true, true, 'backup', 'manual',
             'failed', 'touched',
             '00000000-0000-0000-0000-000000002121', true, now(), now(), now());
        INSERT INTO single_tape_data_run_segments
            (single_tape_data_run_id, segment_order, segment_kind,
             content_sha256, encoded_bytes, storage_ref)
        VALUES
            ('00000000-0000-0000-0000-000000002111', 1, 'dar_slice',
             repeat('a', 64), 1024, '/spool/v1/archive.000001.dar');
        INSERT INTO single_tape_data_run_volumes
            (single_tape_data_run_id, volume_order, cartridge_id,
             expected_cartridge_label, expected_media_generation,
             expected_media_format, state, media_write_started_at)
        VALUES
            ('00000000-0000-0000-0000-000000002110', 1,
             '00000000-0000-0000-0000-000000002107', 'NATIVE021', 9,
             'LTO-9', 'authorized', NULL),
            ('00000000-0000-0000-0000-000000002111', 1,
             '00000000-0000-0000-0000-000000002107', 'NATIVE021', 9,
             'LTO-9', 'write_unverified', now());
        "#,
    )
    .execute(&pool)
    .await
    .expect("Native v1 queued and media-touched fixtures should persist");

    all.run(&pool)
        .await
        .expect("0021 should add bounded rolling-spool persistence");

    let profile = sqlx::query_as::<_, (String, String, String)>(
        "SELECT engine_version, adapter_version, format_version
           FROM single_tape_data_profiles
          WHERE id = '00000000-0000-0000-0000-000000002105'",
    )
    .fetch_one(&pool)
    .await
    .expect("saved profile should remain queryable");
    assert_eq!(
        profile,
        (
            "2".to_owned(),
            "native-v2".to_owned(),
            "tapescale-native-v2".to_owned()
        )
    );

    let recovery_profile = sqlx::query_as::<_, (String, String, String)>(
        "SELECT engine_version, adapter_version, format_version
           FROM single_tape_data_profiles
          WHERE id = '00000000-0000-0000-0000-000000002116'",
    )
    .fetch_one(&pool)
    .await
    .expect("G0.2 profile should remain queryable");
    assert_eq!(
        recovery_profile,
        (
            "1".to_owned(),
            "native-v1".to_owned(),
            "tapescale-native-v1".to_owned()
        ),
        "the v2 backup-only executor must not be assigned to recovery exercises"
    );

    let runs = sqlx::query_as::<_, (String, String, String, i16)>(
        "SELECT requested_by_label, adapter_version, format_version,
                native_layout_version
           FROM single_tape_data_runs
          WHERE id IN (
              '00000000-0000-0000-0000-000000002110',
              '00000000-0000-0000-0000-000000002111'
          )
          ORDER BY requested_by_label",
    )
    .fetch_all(&pool)
    .await
    .expect("upgraded and preserved runs should remain queryable");
    assert_eq!(
        runs,
        vec![
            (
                "touched".to_owned(),
                "native-v1".to_owned(),
                "tapescale-native-v1".to_owned(),
                1
            ),
            (
                "untouched".to_owned(),
                "native-v2".to_owned(),
                "tapescale-native-v2".to_owned(),
                2
            )
        ]
    );

    let volumes = sqlx::query_as::<_, (Uuid, i16)>(
        "SELECT single_tape_data_run_id, native_layout_version
           FROM single_tape_data_run_volumes
          WHERE single_tape_data_run_id IN (
              '00000000-0000-0000-0000-000000002110',
              '00000000-0000-0000-0000-000000002111'
          )
          ORDER BY single_tape_data_run_id",
    )
    .fetch_all(&pool)
    .await
    .expect("volume layout versions should remain aligned with their runs");
    assert_eq!(
        volumes,
        vec![
            (
                Uuid::parse_str("00000000-0000-0000-0000-000000002110").unwrap(),
                2
            ),
            (
                Uuid::parse_str("00000000-0000-0000-0000-000000002111").unwrap(),
                1
            )
        ]
    );

    let legacy_segment = sqlx::query_as::<_, (i16, String, Option<String>)>(
        "SELECT native_layout_version, spool_state, storage_ref
           FROM single_tape_data_run_segments
          WHERE single_tape_data_run_id = '00000000-0000-0000-0000-000000002111'",
    )
    .fetch_one(&pool)
    .await
    .expect("existing Native v1 segment should remain available");
    assert_eq!(
        legacy_segment,
        (
            1,
            "present".to_owned(),
            Some("/spool/v1/archive.000001.dar".to_owned())
        )
    );

    sqlx::query(
        "INSERT INTO single_tape_data_run_segments
            (single_tape_data_run_id, segment_order, segment_kind,
             content_sha256, encoded_bytes, storage_ref, pack_id, pack_kind,
             catalog_sha256, manifest_entry_start, manifest_entry_count)
         VALUES
            ('00000000-0000-0000-0000-000000002110', 1, 'dar_pack',
             repeat('b', 64), 4096, '/spool/v2/pack-1',
             '00000000-0000-0000-0000-000000002112', 'dar_archive',
             repeat('c', 64), 1, 25)",
    )
    .execute(&pool)
    .await
    .expect("new segments should default to the Native v2 pack contract");

    sqlx::query(
        "UPDATE single_tape_data_run_segments
            SET storage_ref = NULL, spool_state = 'reclaimed', spool_reclaimed_at = now()
          WHERE single_tape_data_run_id = '00000000-0000-0000-0000-000000002110'",
    )
    .execute(&pool)
    .await
    .expect("verified pack bookkeeping should support bounded spool reclamation");

    sqlx::query(
        "UPDATE single_tape_data_runs
            SET native_producer_state = 'complete',
                native_spool_low_water_bytes = 268435456,
                native_spool_high_water_bytes = 805306368,
                native_produced_pack_count = 1,
                native_produced_encoded_bytes = 4096,
                native_producer_root_sha256 = repeat('d', 64),
                native_producer_completed_at = now()
          WHERE id = '00000000-0000-0000-0000-000000002110'",
    )
    .execute(&pool)
    .await
    .expect("producer completion should be retained atomically");

    sqlx::query(
        "INSERT INTO native_archive_set_seals
            (single_tape_data_run_id, archive_id, format_version,
             final_archive_volume_id, source_manifest_sha256,
             pack_catalog_root_sha256, volume_count, pack_count,
             total_encoded_bytes, total_logical_bytes, total_entries,
             ordered_volume_index_sha256, ordered_pack_sha256,
             archive_set_sha256, completed_at)
         SELECT id, archive_id, 2,
                '00000000-0000-0000-0000-000000002113', repeat('e', 64),
                repeat('f', 64), 1, 1, 4096, 8192, 25,
                repeat('1', 64), repeat('2', 64), repeat('3', 64), now()
           FROM single_tape_data_runs
          WHERE id = '00000000-0000-0000-0000-000000002110'",
    )
    .execute(&pool)
    .await
    .expect("final archive-set seal should persist");

    let seal_mutation = sqlx::query(
        "UPDATE native_archive_set_seals SET pack_count = 2
          WHERE single_tape_data_run_id = '00000000-0000-0000-0000-000000002110'",
    )
    .execute(&pool)
    .await;
    assert!(
        seal_mutation.is_err(),
        "archive-set seals must be append-only"
    );

    let invalid_window = sqlx::query(
        "UPDATE single_tape_data_runs
            SET native_spool_low_water_bytes = 1000,
                native_spool_high_water_bytes = 500
          WHERE id = '00000000-0000-0000-0000-000000002110'",
    )
    .execute(&pool)
    .await;
    assert!(invalid_window.is_err(), "spool watermarks must be ordered");

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("isolated migration schema should be removed");
    admin.close().await;
}
