use std::{borrow::Cow, time::Duration};

use sqlx::{migrate::Migrator, postgres::PgPoolOptions, Row};
use url::Url;
use uuid::Uuid;

#[tokio::test]
async fn multivolume_upgrade_handles_legacy_terminal_rows_with_and_without_writes() {
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
    let schema = format!("migration_0013_{}", Uuid::now_v7().simple());
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
    let through_0012 = Migrator {
        migrations: Cow::Owned(
            all.iter()
                .filter(|migration| migration.version <= 12)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    through_0012
        .run(&pool)
        .await
        .expect("legacy schema should migrate through 0012");

    sqlx::raw_sql(
        r#"
        INSERT INTO truenas_instances
            (id, name, websocket_url, certificate_fingerprint, credential_secret_ref)
        VALUES
            ('00000000-0000-0000-0000-000000000101', 'upgrade fixture',
             'wss://fixture.invalid', 'fixture', 'fixture');
        INSERT INTO sources
            (id, truenas_instance_id, dataset_guid, dataset_name, source_type,
             logical_root, selection, scope_hash)
        VALUES
            ('00000000-0000-0000-0000-000000000102',
             '00000000-0000-0000-0000-000000000101', 'dataset-fixture',
             'pool/fixture', 'filesystem', '/fixture', '{}', 'fixture-scope');
        INSERT INTO snapshots
            (id, source_id, snapshot_guid, snapshot_name, physical_path,
             logical_root, consistency, application_owned, created_at)
        VALUES
            ('00000000-0000-0000-0000-000000000103',
             '00000000-0000-0000-0000-000000000102', 'snapshot-fixture',
             'fixture', '/source/fixture', '/fixture', 'crash_consistent', false, now());
        INSERT INTO jobs (id, external_id, kind)
        VALUES ('00000000-0000-0000-0000-000000000104', 'upgrade-fixture-job', 'backup');
        INSERT INTO single_tape_data_profiles
            (id, job_id, source_id, source_snapshot_id, name, source_subpath,
             restore_subpath, source_mount_path, restore_root_path, logical_root,
             immutable_snapshot_confirmed)
        VALUES
            ('00000000-0000-0000-0000-000000000105',
             '00000000-0000-0000-0000-000000000104',
             '00000000-0000-0000-0000-000000000102',
             '00000000-0000-0000-0000-000000000103', 'upgrade fixture profile',
             'fixture', 'fixture-restore', '/source/fixture', '/restore/fixture',
             '/fixture', true);
        INSERT INTO drives
            (id, stable_id, generation, observed_path, expected_path, reconciled)
        VALUES
            ('00000000-0000-0000-0000-000000000106', 'upgrade-fixture-drive', 9,
             '/dev/nst0', '/dev/nst0', true);
        INSERT INTO cartridges
            (id, internal_label, media_generation, media_format)
        VALUES
            ('00000000-0000-0000-0000-000000000107', 'UPGRADE001', 9, 'LTO-9');
        INSERT INTO job_runs
            (id, job_id, attempt, state, correlation_id, source_snapshot_id,
             started_at, finished_at)
        VALUES
            ('00000000-0000-0000-0000-000000000111',
             '00000000-0000-0000-0000-000000000104', 1, 'completed',
             '00000000-0000-0000-0000-000000000211',
             '00000000-0000-0000-0000-000000000103', now(), now()),
            ('00000000-0000-0000-0000-000000000112',
             '00000000-0000-0000-0000-000000000104', 1, 'completed',
             '00000000-0000-0000-0000-000000000212',
             '00000000-0000-0000-0000-000000000103', now(), now()),
            ('00000000-0000-0000-0000-000000000113',
             '00000000-0000-0000-0000-000000000104', 1, 'completed',
             '00000000-0000-0000-0000-000000000213',
             '00000000-0000-0000-0000-000000000103', now(), now()),
            ('00000000-0000-0000-0000-000000000114',
             '00000000-0000-0000-0000-000000000104', 1, 'completed',
             '00000000-0000-0000-0000-000000000214',
             '00000000-0000-0000-0000-000000000103', now(), now());
        INSERT INTO single_tape_data_runs
            (id, profile_id, job_run_id, source_snapshot_id, drive_id, cartridge_id,
             expected_stable_id, expected_path, expected_cartridge_label,
             expected_media_generation, expected_media_format, source_mount_path,
             restore_root_path, logical_root, engine_name, engine_version,
             adapter_version, format_version, scratch_media_confirmed,
             destructive_overwrite_confirmed, encryption_deviation_confirmed,
             media_write_started, media_write_started_at, state,
             requested_by_label, correlation_id, attempt, result,
             started_at, finished_at)
        VALUES
            ('00000000-0000-0000-0000-000000000121',
             '00000000-0000-0000-0000-000000000105',
             '00000000-0000-0000-0000-000000000111',
             '00000000-0000-0000-0000-000000000103',
             '00000000-0000-0000-0000-000000000106',
             '00000000-0000-0000-0000-000000000107', 'upgrade-fixture-drive',
             '/dev/nst0', repeat('X', 129) || E'\n', 9, 'LTO-9', '/source/fixture',
             '/restore/fixture', '/fixture', 'Bacula Community', '15.0.3',
             'g0.2-lab-v1', 'bacula-community-15.0.3-lab-v1', true, true, true,
             false, NULL, 'passed', 'fixture',
             '00000000-0000-0000-0000-000000000221', 1, '{}', now(), now()),
            ('00000000-0000-0000-0000-000000000122',
             '00000000-0000-0000-0000-000000000105',
             '00000000-0000-0000-0000-000000000112',
             '00000000-0000-0000-0000-000000000103',
             '00000000-0000-0000-0000-000000000106',
             '00000000-0000-0000-0000-000000000107', 'upgrade-fixture-drive',
             '/dev/nst0', 'UPGRADE001', 9, 'LTO-9', '/source/fixture',
             '/restore/fixture', '/fixture', 'Bacula Community', '15.0.3',
             'g0.2-lab-v1', 'bacula-community-15.0.3-lab-v1', true, true, true,
             true, now(), 'passed', 'fixture',
             '00000000-0000-0000-0000-000000000222', 1,
             '{"engine_volume_name":"UPGRADE001"}', now(), now()),
            ('00000000-0000-0000-0000-000000000123',
             '00000000-0000-0000-0000-000000000105',
             '00000000-0000-0000-0000-000000000113',
             '00000000-0000-0000-0000-000000000103',
             '00000000-0000-0000-0000-000000000106',
             '00000000-0000-0000-0000-000000000107', 'upgrade-fixture-drive',
             '/dev/nst0', 'UPGRADE001', 9, 'LTO-9', '/source/fixture',
             '/restore/fixture', '/fixture', 'Bacula Community', '15.0.3',
             'g0.2-lab-v1', 'bacula-community-15.0.3-lab-v1', true, true, true,
             false, NULL, 'failed', 'fixture',
             '00000000-0000-0000-0000-000000000223', 1, '{}', now(), now()),
            ('00000000-0000-0000-0000-000000000124',
             '00000000-0000-0000-0000-000000000105',
             '00000000-0000-0000-0000-000000000114',
             '00000000-0000-0000-0000-000000000103',
             '00000000-0000-0000-0000-000000000106',
             '00000000-0000-0000-0000-000000000107', 'upgrade-fixture-drive',
             '/dev/nst0', 'UPGRADE001', 9, 'LTO-9', '/source/fixture',
             '/restore/fixture', '/fixture', 'Bacula Community', '15.0.3',
             'g0.2-lab-v1', 'bacula-community-15.0.3-lab-v1', true, true, true,
             true, now(), 'failed', 'fixture',
             '00000000-0000-0000-0000-000000000224', 1, '{}', now(), now());
        "#,
    )
    .execute(&pool)
    .await
    .expect("schema-valid legacy terminal fixtures should insert");

    all.run(&pool)
        .await
        .expect("0013 should upgrade every legacy write-boundary combination");
    let rows = sqlx::query(
        "SELECT run.media_write_started, run.state AS legacy_state,
                volume.state AS volume_state,
                volume.media_write_started_at IS NOT NULL AS has_write_time,
                volume.written_at IS NOT NULL AS has_written_time,
                volume.verified_at IS NOT NULL AS has_verified_time
           FROM single_tape_data_runs run
           JOIN single_tape_data_run_volumes volume
             ON volume.single_tape_data_run_id = run.id
          ORDER BY run.id",
    )
    .fetch_all(&pool)
    .await
    .expect("upgraded volume rows should be queryable");
    let states = rows
        .iter()
        .map(|row| {
            (
                row.get::<bool, _>("media_write_started"),
                row.get::<String, _>("legacy_state"),
                row.get::<String, _>("volume_state"),
                row.get::<bool, _>("has_write_time"),
                row.get::<bool, _>("has_written_time"),
                row.get::<bool, _>("has_verified_time"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        states,
        vec![
            (
                false,
                "passed".to_owned(),
                "authorized".to_owned(),
                false,
                false,
                false
            ),
            (
                true,
                "passed".to_owned(),
                "verified".to_owned(),
                true,
                true,
                true
            ),
            (
                false,
                "failed".to_owned(),
                "authorized".to_owned(),
                false,
                false,
                false
            ),
            (
                true,
                "failed".to_owned(),
                "write_unverified".to_owned(),
                true,
                false,
                false
            ),
        ]
    );

    let legacy_label = sqlx::query(
        "SELECT volume.expected_cartridge_label
           FROM single_tape_data_run_volumes volume
          WHERE volume.single_tape_data_run_id =
                '00000000-0000-0000-0000-000000000121'",
    )
    .fetch_one(&pool)
    .await
    .expect("schema-valid legacy labels should survive the ordered-volume backfill")
    .get::<String, _>("expected_cartridge_label");
    assert_eq!(legacy_label.len(), 130);
    assert!(legacy_label.ends_with('\n'));

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("isolated migration schema should be removed");
}
