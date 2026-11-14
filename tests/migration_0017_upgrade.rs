use std::{borrow::Cow, time::Duration};

use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use url::Url;
use uuid::Uuid;

#[tokio::test]
async fn continuous_delivery_upgrade_preserves_runs_and_accelerates_future_policy_runs() {
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
    let schema = format!("migration_0017_{}", Uuid::now_v7().simple());
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
    let through_0016 = Migrator {
        migrations: Cow::Owned(
            all.iter()
                .filter(|migration| migration.version <= 16)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    through_0016
        .run(&pool)
        .await
        .expect("legacy schema should migrate through 0016");

    let absent = sqlx::query_scalar::<_, i64>(
        "SELECT count(*)
           FROM information_schema.columns
          WHERE table_schema = current_schema()
            AND column_name = 'data_delivery'
            AND table_name IN ('protection_policies', 'single_tape_data_runs')",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy delivery columns should be inspectable");
    assert_eq!(absent, 0);

    all.run(&pool)
        .await
        .expect("0017 should add selectable tape delivery");

    let columns = sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT table_name, is_nullable, column_default
           FROM information_schema.columns
          WHERE table_schema = current_schema()
            AND column_name = 'data_delivery'
            AND table_name IN ('protection_policies', 'single_tape_data_runs')
          ORDER BY table_name",
    )
    .fetch_all(&pool)
    .await
    .expect("delivery columns should be queryable");
    assert_eq!(columns.len(), 2);
    assert_eq!(columns[0].0, "protection_policies");
    assert_eq!(columns[0].1, "NO");
    assert!(columns[0]
        .2
        .as_deref()
        .is_some_and(|default| default.contains("direct")));
    assert_eq!(columns[1].0, "single_tape_data_runs");
    assert_eq!(columns[1].1, "NO");
    assert!(columns[1]
        .2
        .as_deref()
        .is_some_and(|default| default.contains("disk_spool")));

    for table in ["protection_policies", "single_tape_data_runs"] {
        let constraint = sqlx::query_scalar::<_, String>(
            "SELECT pg_get_constraintdef(oid)
               FROM pg_constraint
              WHERE conrelid = $1::regclass
                AND pg_get_constraintdef(oid) LIKE '%data_delivery%'",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("delivery constraint should be queryable");
        assert!(constraint.contains("direct"));
        assert!(constraint.contains("disk_spool"));
    }

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("isolated migration schema should be removed");
    admin.close().await;
}
