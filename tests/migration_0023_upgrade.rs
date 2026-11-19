use std::{borrow::Cow, time::Duration};

use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use url::Url;
use uuid::Uuid;

#[tokio::test]
async fn job_dismissal_upgrade_preserves_rows_and_enforces_complete_tombstones() {
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
    let schema = format!("migration_0023_{}", Uuid::now_v7().simple());
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
    let through_0022 = Migrator {
        migrations: Cow::Owned(
            all.iter()
                .filter(|migration| migration.version <= 22)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    through_0022
        .run(&pool)
        .await
        .expect("schema should migrate through Native v2 overlap reconciliation");

    sqlx::raw_sql(
        r#"
        INSERT INTO jobs (id, external_id, kind)
        VALUES ('00000000-0000-0000-0000-000000002301', '0023-job', 'backup');
        INSERT INTO job_runs
            (id, job_id, state, correlation_id, started_at, finished_at)
        VALUES
            ('00000000-0000-0000-0000-000000002302',
             '00000000-0000-0000-0000-000000002301', 'failed',
             '00000000-0000-0000-0000-000000002303', now(), now()),
            ('00000000-0000-0000-0000-000000002304',
             '00000000-0000-0000-0000-000000002301', 'completed',
             '00000000-0000-0000-0000-000000002305', now(), now());
        "#,
    )
    .execute(&pool)
    .await
    .expect("pre-upgrade job history should persist");

    all.run(&pool)
        .await
        .expect("0023 should add non-destructive job dismissal");

    sqlx::query(
        "UPDATE job_runs
            SET dismissed_at = now(), dismissed_by_label = 'Migration test',
                dismissal_reason = 'remove from operator history'
          WHERE id = '00000000-0000-0000-0000-000000002302'",
    )
    .execute(&pool)
    .await
    .expect("a complete dismissal tombstone should be accepted");
    let dismissed = sqlx::query_as::<_, (String, bool, bool, bool)>(
        "SELECT state, dismissed_at IS NOT NULL,
                dismissed_by_label IS NOT NULL, dismissal_reason IS NOT NULL
           FROM job_runs
          WHERE id = '00000000-0000-0000-0000-000000002302'",
    )
    .fetch_one(&pool)
    .await
    .expect("dismissed history should remain queryable");
    assert_eq!(dismissed, ("failed".to_owned(), true, true, true));

    let partial = sqlx::query(
        "UPDATE job_runs SET dismissed_at = now()
          WHERE id = '00000000-0000-0000-0000-000000002304'",
    )
    .execute(&pool)
    .await;
    assert!(
        partial.is_err(),
        "partial dismissal metadata must fail closed"
    );

    let event_constraint = sqlx::query_scalar::<_, String>(
        "SELECT pg_get_constraintdef(oid)
           FROM pg_constraint
          WHERE conrelid = 'backup_request_events'::regclass
            AND conname = 'backup_request_events_event_type_check'",
    )
    .fetch_one(&pool)
    .await
    .expect("backup request event constraint should remain present");
    assert!(event_constraint.contains("deleted"));

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("isolated migration schema should be removed");
}
