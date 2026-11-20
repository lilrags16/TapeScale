use std::{borrow::Cow, time::Duration};

use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use url::Url;
use uuid::Uuid;

#[tokio::test]
async fn native_v2_profiles_upgrade_to_automatic_multivolume_without_touching_history() {
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
    let schema = format!("migration_0025_{}", Uuid::now_v7().simple());
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
    let through_0024 = Migrator {
        migrations: Cow::Owned(
            all.iter()
                .filter(|migration| migration.version <= 24)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    through_0024
        .run(&pool)
        .await
        .expect("schema should migrate through the pre-multivolume default");

    sqlx::raw_sql(
        r#"
        INSERT INTO truenas_instances
            (id, name, websocket_url, certificate_fingerprint, credential_secret_ref)
        VALUES
            ('00000000-0000-0000-0000-000000002501', '0025 fixture',
             'wss://fixture.invalid', 'fixture', 'fixture');
        INSERT INTO sources
            (id, truenas_instance_id, dataset_guid, dataset_name, source_type,
             logical_root, selection, scope_hash)
        VALUES
            ('00000000-0000-0000-0000-000000002502',
             '00000000-0000-0000-0000-000000002501', 'dataset-0025',
             'pool/fixture', 'filesystem', '/fixture', '{}', 'fixture-scope');
        INSERT INTO snapshots
            (id, source_id, snapshot_guid, snapshot_name, physical_path,
             logical_root, consistency, application_owned, created_at)
        VALUES
            ('00000000-0000-0000-0000-000000002503',
             '00000000-0000-0000-0000-000000002502', 'snapshot-0025',
             'fixture', '/source/fixture', '/fixture', 'crash_consistent', false, now());
        INSERT INTO protection_policies
            (id, name, source_id, representation, schedule_timezone,
             full_schedule, incremental_schedule, encryption_enabled, enabled,
             spanning_enabled)
        VALUES
            ('00000000-0000-0000-0000-000000002504', '0025 Native v2',
             '00000000-0000-0000-0000-000000002502', 'file', 'UTC',
             'manual', 'disabled', false, true, false),
            ('00000000-0000-0000-0000-000000002505', '0025 Native v1 history',
             '00000000-0000-0000-0000-000000002502', 'file', 'UTC',
             'manual', 'disabled', false, true, false);
        INSERT INTO jobs (id, external_id, kind, policy_id)
        VALUES
            ('00000000-0000-0000-0000-000000002506', '0025-v2', 'backup',
             '00000000-0000-0000-0000-000000002504'),
            ('00000000-0000-0000-0000-000000002507', '0025-v1', 'backup',
             '00000000-0000-0000-0000-000000002505');
        INSERT INTO single_tape_data_profiles
            (id, job_id, source_id, source_snapshot_id, name, source_subpath,
             restore_subpath, source_mount_path, restore_root_path, logical_root,
             engine_name, engine_version, adapter_version, format_version,
             archive_codec, immutable_snapshot_confirmed, workflow_kind, policy_id)
        VALUES
            ('00000000-0000-0000-0000-000000002508',
             '00000000-0000-0000-0000-000000002506',
             '00000000-0000-0000-0000-000000002502',
             '00000000-0000-0000-0000-000000002503', '0025 Native v2 profile',
             'fixture', 'fixture-restore', '/source/fixture', '/restore/fixture',
             '/fixture', 'TapeScale Native', '2', 'native-v2',
             'tapescale-native-v2', 'dar', true, 'backup',
             '00000000-0000-0000-0000-000000002504'),
            ('00000000-0000-0000-0000-000000002509',
             '00000000-0000-0000-0000-000000002507',
             '00000000-0000-0000-0000-000000002502',
             '00000000-0000-0000-0000-000000002503', '0025 Native v1 profile',
             'fixture', 'fixture-restore', '/source/fixture', '/restore/fixture',
             '/fixture', 'TapeScale Native', '1', 'native-v1',
             'tapescale-native-v1', 'dar', true, 'backup',
             '00000000-0000-0000-0000-000000002505');
        "#,
    )
    .execute(&pool)
    .await
    .expect("pre-upgrade Native profiles should persist");

    all.run(&pool)
        .await
        .expect("0025 should enable automatic multi-volume Native v2 backups");

    let policies = sqlx::query_as::<_, (String, bool)>(
        "SELECT name, spanning_enabled
           FROM protection_policies
          WHERE id IN (
              '00000000-0000-0000-0000-000000002504',
              '00000000-0000-0000-0000-000000002505'
          )
          ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .expect("upgraded policies should remain queryable");
    assert_eq!(
        policies,
        vec![
            ("0025 Native v1 history".to_owned(), false),
            ("0025 Native v2".to_owned(), true),
        ]
    );

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("isolated migration schema should be removed");
}
