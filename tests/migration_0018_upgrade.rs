use std::{borrow::Cow, time::Duration};

use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use url::Url;
use uuid::Uuid;

#[tokio::test]
async fn whole_archive_staging_upgrade_moves_saved_policies_to_the_safe_default() {
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
    let schema = format!("migration_0018_{}", Uuid::now_v7().simple());
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
    let through_0017 = Migrator {
        migrations: Cow::Owned(
            all.iter()
                .filter(|migration| migration.version <= 17)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    through_0017
        .run(&pool)
        .await
        .expect("legacy schema should migrate through 0017");

    let instance_id = Uuid::now_v7();
    let source_id = Uuid::now_v7();
    let policy_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO truenas_instances
             (id, name, websocket_url, certificate_fingerprint, credential_secret_ref)
         VALUES ($1, 'migration host', 'wss://migration.invalid', 'fixture', 'fixture')",
    )
    .bind(instance_id)
    .execute(&pool)
    .await
    .expect("migration TrueNAS fixture should persist");
    sqlx::query(
        "INSERT INTO sources
             (id, truenas_instance_id, dataset_guid, dataset_name, source_type,
              logical_root, selection, scope_hash)
         VALUES ($1, $2, 'migration-guid', 'pool/migration', 'filesystem',
                 '/source/pool/migration', '{}'::jsonb, 'migration-scope')",
    )
    .bind(source_id)
    .bind(instance_id)
    .execute(&pool)
    .await
    .expect("migration source fixture should persist");
    sqlx::query(
        "INSERT INTO protection_policies
             (id, name, source_id, representation, schedule_timezone, full_schedule,
              incremental_schedule, retention_days, encryption_enabled, compression_mode,
              verification_mode, config, enabled, data_delivery)
         VALUES ($1, 'migration policy', $2, 'file', 'UTC', '0 2 * * 6', '', 28,
                 false, 'auto', 'full_readback', '{}'::jsonb, false, 'direct')",
    )
    .bind(policy_id)
    .bind(source_id)
    .execute(&pool)
    .await
    .expect("legacy direct policy should persist");

    all.run(&pool)
        .await
        .expect("0018 should enable whole-archive staging safety");

    let migrated = sqlx::query_scalar::<_, String>(
        "SELECT data_delivery FROM protection_policies WHERE id = $1",
    )
    .bind(policy_id)
    .fetch_one(&pool)
    .await
    .expect("migrated policy should be queryable");
    assert_eq!(migrated, "disk_spool");

    let defaults = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT table_name, column_default
           FROM information_schema.columns
          WHERE table_schema = current_schema()
            AND column_name = 'data_delivery'
            AND table_name IN ('protection_policies', 'single_tape_data_runs')
          ORDER BY table_name",
    )
    .fetch_all(&pool)
    .await
    .expect("delivery defaults should be queryable");
    assert_eq!(defaults.len(), 2);
    assert!(defaults.iter().all(|(_, value)| value
        .as_deref()
        .is_some_and(|default| default.contains("disk_spool"))));

    sqlx::query("UPDATE protection_policies SET data_delivery = 'direct' WHERE id = $1")
        .bind(policy_id)
        .execute(&pool)
        .await
        .expect("a qualified operator may deliberately restore direct delivery");

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("isolated migration schema should be removed");
    admin.close().await;
}
