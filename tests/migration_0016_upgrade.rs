use std::{borrow::Cow, time::Duration};

use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use url::Url;
use uuid::Uuid;

#[tokio::test]
async fn live_inventory_upgrade_adds_the_durable_scan_phase() {
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
    let schema = format!("migration_0016_{}", Uuid::now_v7().simple());
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
    let through_0015 = Migrator {
        migrations: Cow::Owned(
            all.iter()
                .filter(|migration| migration.version <= 15)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    through_0015
        .run(&pool)
        .await
        .expect("legacy schema should migrate through 0015");

    let before = sqlx::query_scalar::<_, String>(
        "SELECT pg_get_constraintdef(oid)
         FROM pg_constraint
         WHERE conrelid = 'job_runs'::regclass
           AND conname = 'job_runs_state_check'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy job-state constraint should be queryable");
    assert!(!before.contains("inventorying"));

    all.run(&pool)
        .await
        .expect("0016 should add live source inventory state");

    let job_constraint = sqlx::query_scalar::<_, String>(
        "SELECT pg_get_constraintdef(oid)
         FROM pg_constraint
         WHERE conrelid = 'job_runs'::regclass
           AND conname = 'job_runs_state_check'",
    )
    .fetch_one(&pool)
    .await
    .expect("updated job-state constraint should be queryable");
    assert!(job_constraint.contains("inventorying"));

    let event_constraint = sqlx::query_scalar::<_, String>(
        "SELECT pg_get_constraintdef(oid)
         FROM pg_constraint
         WHERE conrelid = 'single_tape_data_events'::regclass
           AND conname = 'single_tape_data_events_event_type_check'",
    )
    .fetch_one(&pool)
    .await
    .expect("updated data-event constraint should be queryable");
    assert!(event_constraint.contains("inventorying"));

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("isolated migration schema should be removed");
    admin.close().await;
}
