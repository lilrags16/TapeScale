use std::{borrow::Cow, time::Duration};

use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use url::Url;
use uuid::Uuid;

#[tokio::test]
async fn native_archive_upgrade_preserves_legacy_rows_and_accepts_native_segments() {
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
    let schema = format!("migration_0019_{}", Uuid::now_v7().simple());
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
    let through_0018 = Migrator {
        migrations: Cow::Owned(
            all.iter()
                .filter(|migration| migration.version <= 18)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    through_0018
        .run(&pool)
        .await
        .expect("legacy schema should migrate through 0018");

    let through_0019 = Migrator {
        migrations: Cow::Owned(
            all.iter()
                .filter(|migration| migration.version <= 19)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };

    sqlx::raw_sql(
        r#"
        INSERT INTO truenas_instances
            (id, name, websocket_url, certificate_fingerprint, credential_secret_ref)
        VALUES
            ('00000000-0000-0000-0000-000000001901', '0019 fixture',
             'wss://fixture.invalid', 'fixture', 'fixture');
        INSERT INTO sources
            (id, truenas_instance_id, dataset_guid, dataset_name, source_type,
             logical_root, selection, scope_hash)
        VALUES
            ('00000000-0000-0000-0000-000000001902',
             '00000000-0000-0000-0000-000000001901', 'dataset-0019',
             'pool/fixture', 'filesystem', '/fixture', '{}', 'fixture-scope');
        ALTER TABLE protection_policies
            DROP CONSTRAINT protection_policies_compression_mode_check;
        INSERT INTO protection_policies
            (id, name, source_id, representation, schedule_timezone,
             full_schedule, incremental_schedule, compression_mode)
        VALUES
            ('00000000-0000-0000-0000-000000001900', '0019 legacy codec policy',
             '00000000-0000-0000-0000-000000001902', 'file', 'UTC',
             '0 2 * * 6', '', 'legacy-experimental-codec');
        ALTER TABLE protection_policies
            ADD CONSTRAINT protection_policies_compression_mode_check
            CHECK (compression_mode IN ('auto', 'none', 'gzip', 'lzo')) NOT VALID;
        INSERT INTO snapshots
            (id, source_id, snapshot_guid, snapshot_name, physical_path,
             logical_root, consistency, application_owned, created_at)
        VALUES
            ('00000000-0000-0000-0000-000000001903',
             '00000000-0000-0000-0000-000000001902', 'snapshot-0019',
             'fixture', '/source/fixture', '/fixture', 'crash_consistent', false, now());
        INSERT INTO jobs (id, external_id, kind)
        VALUES ('00000000-0000-0000-0000-000000001904', '0019-legacy', 'backup');
        INSERT INTO single_tape_data_profiles
            (id, job_id, source_id, source_snapshot_id, name, source_subpath,
             restore_subpath, source_mount_path, restore_root_path, logical_root,
             immutable_snapshot_confirmed)
        VALUES
            ('00000000-0000-0000-0000-000000001905',
             '00000000-0000-0000-0000-000000001904',
             '00000000-0000-0000-0000-000000001902',
             '00000000-0000-0000-0000-000000001903', '0019 legacy profile',
             'fixture', 'fixture-restore', '/source/fixture', '/restore/fixture',
             '/fixture', true);
        INSERT INTO drives
            (id, stable_id, generation, observed_path, expected_path, reconciled)
        VALUES
            ('00000000-0000-0000-0000-000000001906', '0019-drive', 9,
             '/dev/nst0', '/dev/nst0', true);
        INSERT INTO cartridges
            (id, internal_label, media_generation, media_format)
        VALUES
            ('00000000-0000-0000-0000-000000001907', 'NATIVE001', 9, 'LTO-9');
        INSERT INTO job_runs
            (id, job_id, state, correlation_id, source_snapshot_id,
             started_at, finished_at)
        VALUES
            ('00000000-0000-0000-0000-000000001908',
             '00000000-0000-0000-0000-000000001904', 'completed',
             '00000000-0000-0000-0000-000000001909',
             '00000000-0000-0000-0000-000000001903', now(), now());
        INSERT INTO single_tape_data_runs
            (id, profile_id, job_run_id, source_snapshot_id, drive_id, cartridge_id,
             expected_stable_id, expected_path, expected_cartridge_label,
             expected_media_generation, expected_media_format, source_mount_path,
             restore_root_path, logical_root, engine_name, engine_version,
             adapter_version, format_version, scratch_media_confirmed,
             destructive_overwrite_confirmed, encryption_deviation_confirmed,
             state, requested_by_label, correlation_id, started_at, finished_at)
        VALUES
            ('00000000-0000-0000-0000-000000001910',
             '00000000-0000-0000-0000-000000001905',
             '00000000-0000-0000-0000-000000001908',
             '00000000-0000-0000-0000-000000001903',
             '00000000-0000-0000-0000-000000001906',
             '00000000-0000-0000-0000-000000001907', '0019-drive', '/dev/nst0',
             'NATIVE001', 9, 'LTO-9', '/source/fixture', '/restore/fixture',
             '/fixture', 'Bacula Community', '15.0.3', 'g0.2-lab-v1',
             'bacula-community-15.0.3-lab-v1', true, true, true,
             'failed', 'fixture', '00000000-0000-0000-0000-000000001911',
             now(), now());
        INSERT INTO recovery_points
            (id, source_id, run_id, representation, level, source_scope_hash,
             format_version, status, recovery_time, indefinite, coverage,
             manifest_digest)
        VALUES
            ('00000000-0000-0000-0000-000000001918',
             '00000000-0000-0000-0000-000000001902',
             '00000000-0000-0000-0000-000000001908', 'file', 'full',
             'fixture-scope', 'bacula-community-15.0.3-lab-v1', 'verified',
             now(), true, '{"legacy_fixture":true}', repeat('b', 64));
        INSERT INTO volume_records
            (id, recovery_point_id, cartridge_id, volume_order, first_record,
             last_record, encoded_bytes, finalized, engine_record_id)
        VALUES
            ('00000000-0000-0000-0000-000000001919',
             '00000000-0000-0000-0000-000000001918',
             '00000000-0000-0000-0000-000000001907', 1, 'bacula-file-0',
             'bacula-file-1', 1024, true, 'bacula:volume:fixture');
        INSERT INTO job_runs
            (id, job_id, state, correlation_id, source_snapshot_id)
        VALUES
            ('00000000-0000-0000-0000-000000001914',
             '00000000-0000-0000-0000-000000001904', 'queued',
             '00000000-0000-0000-0000-000000001915',
             '00000000-0000-0000-0000-000000001903');
        INSERT INTO single_tape_data_runs
            (id, profile_id, job_run_id, source_snapshot_id, drive_id, cartridge_id,
             expected_stable_id, expected_path, expected_cartridge_label,
             expected_media_generation, expected_media_format, source_mount_path,
             restore_root_path, logical_root, engine_name, engine_version,
             adapter_version, format_version, scratch_media_confirmed,
             destructive_overwrite_confirmed, encryption_deviation_confirmed,
             compression_mode, resolved_compression, data_delivery, state,
             requested_by_label, correlation_id)
        VALUES
            ('00000000-0000-0000-0000-000000001916',
             '00000000-0000-0000-0000-000000001905',
             '00000000-0000-0000-0000-000000001914',
             '00000000-0000-0000-0000-000000001903',
             '00000000-0000-0000-0000-000000001906',
             '00000000-0000-0000-0000-000000001907', '0019-drive', '/dev/nst0',
             'NATIVE001', 9, 'LTO-9', '/source/fixture', '/restore/fixture',
             '/fixture', 'Bacula Community', '15.0.3', 'g0.2-lab-v1',
             'bacula-community-15.0.3-lab-v1', true, true, true,
             'lzo', 'lzo', 'direct', 'queued', 'fixture',
             '00000000-0000-0000-0000-000000001917');
        INSERT INTO qualification_runs
            (id, drive_id, cartridge_id, expected_stable_id, expected_path,
             expected_cartridge_label, expected_media_generation,
             expected_media_format, scratch_media_confirmed,
             destructive_overwrite_confirmed, state, requested_by_label,
             correlation_id, started_at, finished_at)
        VALUES
            ('00000000-0000-0000-0000-000000001912',
             '00000000-0000-0000-0000-000000001906',
             '00000000-0000-0000-0000-000000001907', '0019-drive', '/dev/nst0',
             'NATIVE001', 9, 'LTO-9', true, true, 'failed', 'fixture',
             '00000000-0000-0000-0000-000000001913', now(), now());
        "#,
    )
    .execute(&pool)
    .await
    .expect("legacy fixtures should persist before the native upgrade");

    through_0019
        .run(&pool)
        .await
        .expect("0019 should enable TapeScale native archive metadata");

    let legacy = sqlx::query_as::<_, (String, String, String, String)>(
        "SELECT profile.engine_name, profile.archive_codec,
                run.engine_name, run.archive_codec
           FROM single_tape_data_profiles profile
           JOIN single_tape_data_runs run ON run.profile_id = profile.id
          WHERE run.id = '00000000-0000-0000-0000-000000001910'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy engine evidence should remain queryable");
    assert_eq!(
        legacy,
        (
            "TapeScale Native".to_owned(),
            "dar".to_owned(),
            "Bacula Community".to_owned(),
            "legacy_bacula".to_owned(),
        )
    );

    let retired_point = sqlx::query_as::<_, (String, String, bool, String)>(
        "SELECT archive_codec, status,
                coverage->>'legacy_format_retired' = 'true',
                coverage->>'legacy_format_retired_reason'
           FROM recovery_points
          WHERE id = '00000000-0000-0000-0000-000000001918'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy recovery point should remain as retired audit evidence");
    assert_eq!(
        retired_point,
        (
            "legacy_bacula".to_owned(),
            "failed".to_owned(),
            true,
            "Bacula media declared non-recoverable during TapeScale Native v1 migration".to_owned(),
        )
    );

    let retained_legacy_volume = sqlx::query_scalar::<_, bool>(
        "SELECT finalized FROM volume_records
          WHERE id = '00000000-0000-0000-0000-000000001919'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy volume history should remain queryable");
    assert!(retained_legacy_volume);

    let queued = sqlx::query_as::<_, (String, String, String, String, String)>(
        "SELECT engine_name, archive_codec, data_delivery,
                compression_mode, resolved_compression
           FROM single_tape_data_runs
          WHERE id = '00000000-0000-0000-0000-000000001916'",
    )
    .fetch_one(&pool)
    .await
    .expect("untouched queued run should remain queryable");
    assert_eq!(
        queued,
        (
            "TapeScale Native".to_owned(),
            "dar".to_owned(),
            "disk_spool".to_owned(),
            "lzo".to_owned(),
            "lz4".to_owned(),
        )
    );

    let legacy_policy = sqlx::query_as::<_, (String, bool)>(
        "SELECT policy.compression_mode, constraint_row.convalidated
           FROM protection_policies policy
           CROSS JOIN pg_constraint constraint_row
          WHERE policy.id = '00000000-0000-0000-0000-000000001900'
            AND constraint_row.conrelid = 'protection_policies'::regclass
            AND constraint_row.conname = 'protection_policies_compression_mode_check'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy unsupported policy should survive under a not-valid constraint");
    assert_eq!(
        legacy_policy,
        ("legacy-experimental-codec".to_owned(), false)
    );

    let qualification = sqlx::query_as::<_, (String, String, String)>(
        "INSERT INTO qualification_runs
             (drive_id, cartridge_id, expected_stable_id, expected_path,
              expected_cartridge_label, expected_media_generation,
              expected_media_format, scratch_media_confirmed,
              destructive_overwrite_confirmed, requested_by_label, correlation_id)
         VALUES
             ('00000000-0000-0000-0000-000000001906',
              '00000000-0000-0000-0000-000000001907', '0019-drive', '/dev/nst0',
              'NATIVE001', 9, 'LTO-9', true, true, 'fixture', $1)
         RETURNING engine_name, engine_version, test_kind",
    )
    .bind(Uuid::now_v7())
    .fetch_one(&pool)
    .await
    .expect("native qualification defaults should insert");
    assert_eq!(
        qualification,
        (
            "TapeScale Native".to_owned(),
            "1".to_owned(),
            "native_tape_test".to_owned()
        )
    );

    sqlx::raw_sql(
        r#"
        INSERT INTO jobs (id, external_id, kind)
        VALUES ('00000000-0000-0000-0000-000000001920', '0019-native', 'backup');
        INSERT INTO single_tape_data_profiles
            (id, job_id, source_id, source_snapshot_id, name, source_subpath,
             restore_subpath, source_mount_path, restore_root_path, logical_root,
             immutable_snapshot_confirmed)
        VALUES
            ('00000000-0000-0000-0000-000000001921',
             '00000000-0000-0000-0000-000000001920',
             '00000000-0000-0000-0000-000000001902',
             '00000000-0000-0000-0000-000000001903', '0019 native profile',
             'fixture', 'fixture-native-restore', '/source/fixture', '/restore/fixture',
             '/fixture', true);
        INSERT INTO job_runs
            (id, job_id, state, correlation_id, source_snapshot_id)
        VALUES
            ('00000000-0000-0000-0000-000000001922',
             '00000000-0000-0000-0000-000000001920', 'queued',
             '00000000-0000-0000-0000-000000001923',
             '00000000-0000-0000-0000-000000001903');
        INSERT INTO single_tape_data_runs
            (id, profile_id, job_run_id, source_snapshot_id, drive_id, cartridge_id,
             expected_stable_id, expected_path, expected_cartridge_label,
             expected_media_generation, expected_media_format, source_mount_path,
             restore_root_path, logical_root, engine_name, engine_version,
             adapter_version, format_version, scratch_media_confirmed,
             destructive_overwrite_confirmed, encryption_deviation_confirmed,
             state, requested_by_label, correlation_id)
        VALUES
            ('00000000-0000-0000-0000-000000001924',
             '00000000-0000-0000-0000-000000001921',
             '00000000-0000-0000-0000-000000001922',
             '00000000-0000-0000-0000-000000001903',
             '00000000-0000-0000-0000-000000001906',
             '00000000-0000-0000-0000-000000001907', '0019-drive', '/dev/nst0',
             'NATIVE001', 9, 'LTO-9', '/source/fixture', '/restore/fixture',
             '/fixture', 'TapeScale Native', '1', 'native-v1',
             'tapescale-native-v1', true, true, true, 'queued', 'fixture',
             '00000000-0000-0000-0000-000000001925');
        INSERT INTO single_tape_data_run_volumes
            (single_tape_data_run_id, volume_order, cartridge_id,
             expected_cartridge_label, expected_media_generation,
             expected_media_format, archive_volume_id)
        VALUES
            ('00000000-0000-0000-0000-000000001924', 1,
             '00000000-0000-0000-0000-000000001907', 'NATIVE001', 9, 'LTO-9',
             '00000000-0000-0000-0000-000000001926');
        INSERT INTO single_tape_data_run_segments
            (single_tape_data_run_id, segment_order, segment_kind,
             content_sha256, encoded_bytes, state, volume_order, tape_file_number)
        VALUES
            ('00000000-0000-0000-0000-000000001924', 1, 'dar_slice',
             repeat('a', 64), 67108864, 'assigned', 1, 3);
        INSERT INTO single_tape_data_events
            (single_tape_data_run_id, event_type, evidence, correlation_id)
        VALUES
            ('00000000-0000-0000-0000-000000001924', 'archive_staged',
             '{"archive_codec":"dar"}',
             '00000000-0000-0000-0000-000000001925'),
            ('00000000-0000-0000-0000-000000001924', 'media_identified',
             '{"on_tape_identity":"TS-LTO-0019"}',
             '00000000-0000-0000-0000-000000001925');
        "#,
    )
    .execute(&pool)
    .await
    .expect("native archive run and assigned segment should persist");

    let native = sqlx::query_as::<_, (String, String, String, i64)>(
        "SELECT profile.engine_name, profile.archive_codec, segment.segment_kind,
                segment.encoded_bytes
           FROM single_tape_data_profiles profile
           JOIN single_tape_data_runs run ON run.profile_id = profile.id
           JOIN single_tape_data_run_segments segment
             ON segment.single_tape_data_run_id = run.id
          WHERE run.id = '00000000-0000-0000-0000-000000001924'",
    )
    .fetch_one(&pool)
    .await
    .expect("native archive metadata should reload");
    assert_eq!(
        native,
        (
            "TapeScale Native".to_owned(),
            "dar".to_owned(),
            "dar_slice".to_owned(),
            67_108_864,
        )
    );

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("isolated migration schema should be removed");
    admin.close().await;
}
