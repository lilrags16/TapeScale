use std::{borrow::Cow, time::Duration};

use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use url::Url;
use uuid::Uuid;

#[tokio::test]
async fn overlap_reconciliation_rebinds_only_untouched_native_v1_work() {
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
    let schema = format!("migration_0022_{}", Uuid::now_v7().simple());
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
    let through_0021 = Migrator {
        migrations: Cow::Owned(
            all.iter()
                .filter(|migration| migration.version <= 21)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    through_0021
        .run(&pool)
        .await
        .expect("schema should migrate through rolling Native v2");

    sqlx::raw_sql(
        r#"
        INSERT INTO truenas_instances
            (id, name, websocket_url, certificate_fingerprint, credential_secret_ref)
        VALUES
            ('00000000-0000-0000-0000-000000002201', '0022 fixture',
             'wss://fixture.invalid', 'fixture', 'fixture');
        INSERT INTO sources
            (id, truenas_instance_id, dataset_guid, dataset_name, source_type,
             logical_root, selection, scope_hash)
        VALUES
            ('00000000-0000-0000-0000-000000002202',
             '00000000-0000-0000-0000-000000002201', 'dataset-0022',
             'pool/fixture', 'filesystem', '/fixture', '{}', 'fixture-scope');
        INSERT INTO snapshots
            (id, source_id, snapshot_guid, snapshot_name, physical_path,
             logical_root, consistency, application_owned, created_at)
        VALUES
            ('00000000-0000-0000-0000-000000002203',
             '00000000-0000-0000-0000-000000002202', 'snapshot-0022',
             'fixture', '/source/fixture', '/fixture', 'crash_consistent', false, now());

        INSERT INTO protection_policies
            (id, name, source_id, representation, schedule_timezone,
             full_schedule, incremental_schedule, encryption_enabled, enabled)
        VALUES
            ('00000000-0000-0000-0000-000000002210', '0022 untouched policy',
             '00000000-0000-0000-0000-000000002202', 'file', 'UTC',
             'manual', 'disabled', false, true),
            ('00000000-0000-0000-0000-000000002211', '0022 running policy',
             '00000000-0000-0000-0000-000000002202', 'file', 'UTC',
             'manual', 'disabled', false, true),
            ('00000000-0000-0000-0000-000000002212', '0022 terminal policy',
             '00000000-0000-0000-0000-000000002202', 'file', 'UTC',
             'manual', 'disabled', false, true),
            ('00000000-0000-0000-0000-000000002213', '0022 touched policy',
             '00000000-0000-0000-0000-000000002202', 'file', 'UTC',
             'manual', 'disabled', false, true);
        INSERT INTO jobs (id, external_id, kind, policy_id)
        VALUES
            ('00000000-0000-0000-0000-000000002220', '0022-untouched', 'backup',
             '00000000-0000-0000-0000-000000002210'),
            ('00000000-0000-0000-0000-000000002221', '0022-running', 'backup',
             '00000000-0000-0000-0000-000000002211'),
            ('00000000-0000-0000-0000-000000002222', '0022-terminal', 'backup',
             '00000000-0000-0000-0000-000000002212'),
            ('00000000-0000-0000-0000-000000002223', '0022-touched', 'backup',
             '00000000-0000-0000-0000-000000002213'),
            ('00000000-0000-0000-0000-000000002224', '0022-g02', 'backup', NULL);
        INSERT INTO single_tape_data_profiles
            (id, job_id, source_id, source_snapshot_id, name, source_subpath,
             restore_subpath, source_mount_path, restore_root_path, logical_root,
             engine_name, engine_version, adapter_version, format_version,
             archive_codec, immutable_snapshot_confirmed, workflow_kind, policy_id)
        VALUES
            ('00000000-0000-0000-0000-000000002230',
             '00000000-0000-0000-0000-000000002220',
             '00000000-0000-0000-0000-000000002202',
             '00000000-0000-0000-0000-000000002203', '0022 untouched profile',
             'untouched', 'restore-untouched', '/source/fixture', '/restore/fixture',
             '/fixture', 'TapeScale Native', '1', 'native-v1',
             'tapescale-native-v1', 'dar', true, 'backup',
             '00000000-0000-0000-0000-000000002210'),
            ('00000000-0000-0000-0000-000000002231',
             '00000000-0000-0000-0000-000000002221',
             '00000000-0000-0000-0000-000000002202',
             '00000000-0000-0000-0000-000000002203', '0022 running profile',
             'running', 'restore-running', '/source/fixture', '/restore/fixture',
             '/fixture', 'TapeScale Native', '1', 'native-v1',
             'tapescale-native-v1', 'dar', true, 'backup',
             '00000000-0000-0000-0000-000000002211'),
            ('00000000-0000-0000-0000-000000002232',
             '00000000-0000-0000-0000-000000002222',
             '00000000-0000-0000-0000-000000002202',
             '00000000-0000-0000-0000-000000002203', '0022 terminal profile',
             'terminal', 'restore-terminal', '/source/fixture', '/restore/fixture',
             '/fixture', 'TapeScale Native', '1', 'native-v1',
             'tapescale-native-v1', 'dar', true, 'backup',
             '00000000-0000-0000-0000-000000002212'),
            ('00000000-0000-0000-0000-000000002233',
             '00000000-0000-0000-0000-000000002223',
             '00000000-0000-0000-0000-000000002202',
             '00000000-0000-0000-0000-000000002203', '0022 touched profile',
             'touched', 'restore-touched', '/source/fixture', '/restore/fixture',
             '/fixture', 'TapeScale Native', '1', 'native-v1',
             'tapescale-native-v1', 'dar', true, 'backup',
             '00000000-0000-0000-0000-000000002213'),
            ('00000000-0000-0000-0000-000000002234',
             '00000000-0000-0000-0000-000000002224',
             '00000000-0000-0000-0000-000000002202',
             '00000000-0000-0000-0000-000000002203', '0022 G0.2 profile',
             'g02', 'restore-g02', '/source/fixture', '/restore/fixture',
             '/fixture', 'TapeScale Native', '1', 'native-v1',
             'tapescale-native-v1', 'dar', true, 'recovery_exercise', NULL);

        INSERT INTO drives
            (id, stable_id, generation, observed_path, expected_path, reconciled)
        VALUES
            ('00000000-0000-0000-0000-000000002240', '0022-drive-0', 9,
             '/dev/nst0', '/dev/nst0', true),
            ('00000000-0000-0000-0000-000000002241', '0022-drive-1', 9,
             '/dev/nst1', '/dev/nst1', true),
            ('00000000-0000-0000-0000-000000002242', '0022-drive-2', 9,
             '/dev/nst2', '/dev/nst2', true),
            ('00000000-0000-0000-0000-000000002243', '0022-drive-3', 9,
             '/dev/nst3', '/dev/nst3', true);
        INSERT INTO cartridges (id, internal_label, media_generation, media_format)
        VALUES
            ('00000000-0000-0000-0000-000000002250', 'OVERLAP220', 9, 'LTO-9'),
            ('00000000-0000-0000-0000-000000002251', 'OVERLAP221', 9, 'LTO-9'),
            ('00000000-0000-0000-0000-000000002252', 'OVERLAP222', 9, 'LTO-9'),
            ('00000000-0000-0000-0000-000000002253', 'OVERLAP223', 9, 'LTO-9');
        INSERT INTO job_runs
            (id, job_id, state, correlation_id, source_snapshot_id, started_at, finished_at)
        VALUES
            ('00000000-0000-0000-0000-000000002260',
             '00000000-0000-0000-0000-000000002220', 'queued',
             '00000000-0000-0000-0000-000000002270',
             '00000000-0000-0000-0000-000000002203', NULL, NULL),
            ('00000000-0000-0000-0000-000000002261',
             '00000000-0000-0000-0000-000000002221', 'writing',
             '00000000-0000-0000-0000-000000002271',
             '00000000-0000-0000-0000-000000002203', now(), NULL),
            ('00000000-0000-0000-0000-000000002262',
             '00000000-0000-0000-0000-000000002222', 'blocked',
             '00000000-0000-0000-0000-000000002272',
             '00000000-0000-0000-0000-000000002203', now(), now()),
            ('00000000-0000-0000-0000-000000002263',
             '00000000-0000-0000-0000-000000002223', 'queued',
             '00000000-0000-0000-0000-000000002273',
             '00000000-0000-0000-0000-000000002203', NULL, NULL);
        INSERT INTO single_tape_data_runs
            (id, profile_id, job_run_id, source_snapshot_id, drive_id, cartridge_id,
             expected_stable_id, expected_path, expected_cartridge_label,
             expected_media_generation, expected_media_format, source_mount_path,
             restore_root_path, logical_root, engine_name, engine_version,
             adapter_version, format_version, archive_codec,
             scratch_media_confirmed, destructive_overwrite_confirmed,
             encryption_deviation_confirmed, workflow_kind, trigger_kind, state,
             requested_by_label, correlation_id, native_layout_version,
             media_write_started, media_write_started_at, started_at, finished_at)
        VALUES
            ('00000000-0000-0000-0000-000000002280',
             '00000000-0000-0000-0000-000000002230',
             '00000000-0000-0000-0000-000000002260',
             '00000000-0000-0000-0000-000000002203',
             '00000000-0000-0000-0000-000000002240',
             '00000000-0000-0000-0000-000000002250', '0022-drive-0', '/dev/nst0',
             'OVERLAP220', 9, 'LTO-9', '/source/fixture', '/restore/fixture',
             '/fixture', 'TapeScale Native', '1', 'native-v1',
             'tapescale-native-v1', 'dar', true, true, true, 'backup', 'manual',
             'queued', 'untouched overlap',
             '00000000-0000-0000-0000-000000002270', 1, false, NULL, NULL, NULL),
            ('00000000-0000-0000-0000-000000002281',
             '00000000-0000-0000-0000-000000002231',
             '00000000-0000-0000-0000-000000002261',
             '00000000-0000-0000-0000-000000002203',
             '00000000-0000-0000-0000-000000002241',
             '00000000-0000-0000-0000-000000002251', '0022-drive-1', '/dev/nst1',
             'OVERLAP221', 9, 'LTO-9', '/source/fixture', '/restore/fixture',
             '/fixture', 'TapeScale Native', '1', 'native-v1',
             'tapescale-native-v1', 'dar', true, true, true, 'backup', 'manual',
             'running', 'running overlap',
             '00000000-0000-0000-0000-000000002271', 1, false, NULL, now(), NULL),
            ('00000000-0000-0000-0000-000000002282',
             '00000000-0000-0000-0000-000000002232',
             '00000000-0000-0000-0000-000000002262',
             '00000000-0000-0000-0000-000000002203',
             '00000000-0000-0000-0000-000000002242',
             '00000000-0000-0000-0000-000000002252', '0022-drive-2', '/dev/nst2',
             'OVERLAP222', 9, 'LTO-9', '/source/fixture', '/restore/fixture',
             '/fixture', 'TapeScale Native', '1', 'native-v1',
             'tapescale-native-v1', 'dar', true, true, true, 'backup', 'manual',
             'blocked', 'terminal overlap',
             '00000000-0000-0000-0000-000000002272', 1, false, NULL, now(), now()),
            ('00000000-0000-0000-0000-000000002283',
             '00000000-0000-0000-0000-000000002233',
             '00000000-0000-0000-0000-000000002263',
             '00000000-0000-0000-0000-000000002203',
             '00000000-0000-0000-0000-000000002243',
             '00000000-0000-0000-0000-000000002253', '0022-drive-3', '/dev/nst3',
             'OVERLAP223', 9, 'LTO-9', '/source/fixture', '/restore/fixture',
             '/fixture', 'TapeScale Native', '1', 'native-v1',
             'tapescale-native-v1', 'dar', true, true, true, 'backup', 'manual',
             'queued', 'media-touched overlap',
             '00000000-0000-0000-0000-000000002273', 1, true, now(), NULL, NULL);
        INSERT INTO single_tape_data_run_volumes
            (single_tape_data_run_id, volume_order, cartridge_id,
             expected_cartridge_label, expected_media_generation,
             expected_media_format, state, native_layout_version,
             media_write_started_at)
        VALUES
            ('00000000-0000-0000-0000-000000002280', 1,
             '00000000-0000-0000-0000-000000002250', 'OVERLAP220', 9,
             'LTO-9', 'authorized', 1, NULL),
            ('00000000-0000-0000-0000-000000002281', 1,
             '00000000-0000-0000-0000-000000002251', 'OVERLAP221', 9,
             'LTO-9', 'authorized', 1, NULL),
            ('00000000-0000-0000-0000-000000002282', 1,
             '00000000-0000-0000-0000-000000002252', 'OVERLAP222', 9,
             'LTO-9', 'authorized', 1, NULL),
            ('00000000-0000-0000-0000-000000002283', 1,
             '00000000-0000-0000-0000-000000002253', 'OVERLAP223', 9,
             'LTO-9', 'write_started', 1, now());
        "#,
    )
    .execute(&pool)
    .await
    .expect("post-0021 overlap fixtures should persist");

    all.run(&pool)
        .await
        .expect("0022 should reconcile only safe Native v1 overlap rows");

    let backup_profiles = sqlx::query_as::<_, (String, String, String)>(
        "SELECT engine_version, adapter_version, format_version
           FROM single_tape_data_profiles
          WHERE workflow_kind = 'backup'
          ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("operational profiles should remain queryable");
    assert_eq!(backup_profiles.len(), 4);
    assert!(backup_profiles.iter().all(|binding| binding
        == &(
            "2".to_owned(),
            "native-v2".to_owned(),
            "tapescale-native-v2".to_owned()
        )));
    let recovery_binding = sqlx::query_as::<_, (String, String, String)>(
        "SELECT engine_version, adapter_version, format_version
           FROM single_tape_data_profiles
          WHERE id = '00000000-0000-0000-0000-000000002234'",
    )
    .fetch_one(&pool)
    .await
    .expect("recovery exercise profile should remain queryable");
    assert_eq!(
        recovery_binding,
        (
            "1".to_owned(),
            "native-v1".to_owned(),
            "tapescale-native-v1".to_owned()
        )
    );

    let run_bindings = sqlx::query_as::<_, (String, String, i16)>(
        "SELECT requested_by_label, format_version, native_layout_version
           FROM single_tape_data_runs
          WHERE id IN (
              '00000000-0000-0000-0000-000000002280',
              '00000000-0000-0000-0000-000000002281',
              '00000000-0000-0000-0000-000000002282',
              '00000000-0000-0000-0000-000000002283'
          )
          ORDER BY requested_by_label",
    )
    .fetch_all(&pool)
    .await
    .expect("overlap runs should remain queryable");
    assert_eq!(
        run_bindings,
        vec![
            (
                "media-touched overlap".to_owned(),
                "tapescale-native-v1".to_owned(),
                1
            ),
            (
                "running overlap".to_owned(),
                "tapescale-native-v1".to_owned(),
                1
            ),
            (
                "terminal overlap".to_owned(),
                "tapescale-native-v1".to_owned(),
                1
            ),
            (
                "untouched overlap".to_owned(),
                "tapescale-native-v2".to_owned(),
                2
            )
        ]
    );
    let volume_layouts = sqlx::query_as::<_, (Uuid, i16)>(
        "SELECT single_tape_data_run_id, native_layout_version
           FROM single_tape_data_run_volumes
          WHERE single_tape_data_run_id IN (
              '00000000-0000-0000-0000-000000002280',
              '00000000-0000-0000-0000-000000002281',
              '00000000-0000-0000-0000-000000002282',
              '00000000-0000-0000-0000-000000002283'
          )
          ORDER BY single_tape_data_run_id",
    )
    .fetch_all(&pool)
    .await
    .expect("overlap volume layouts should remain queryable");
    assert_eq!(volume_layouts[0].1, 2);
    assert!(volume_layouts[1..].iter().all(|(_, layout)| *layout == 1));

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("isolated migration schema should be removed");
    admin.close().await;
}
