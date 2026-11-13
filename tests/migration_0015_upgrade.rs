use std::{borrow::Cow, time::Duration};

use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use url::Url;
use uuid::Uuid;

#[tokio::test]
async fn selective_restore_upgrade_adds_immutable_scope_and_distinct_proof_kind() {
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
    let schema = format!("migration_0015_{}", Uuid::now_v7().simple());
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
    let through_0014 = Migrator {
        migrations: Cow::Owned(
            all.iter()
                .filter(|migration| migration.version <= 14)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    through_0014
        .run(&pool)
        .await
        .expect("alpha.3 schema should migrate through 0014");

    let absent = sqlx::query_scalar::<_, bool>(
        "SELECT NOT EXISTS(
             SELECT 1 FROM information_schema.columns
             WHERE table_schema = current_schema()
               AND table_name = 'restore_rehearsals'
               AND column_name = 'restore_scope')",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy restore scope lookup should succeed");
    assert!(absent);

    all.run(&pool)
        .await
        .expect("0015 should add the selective restore contract");

    let column = sqlx::query_as::<_, (String, String, Option<String>)>(
        "SELECT data_type, is_nullable, column_default
         FROM information_schema.columns
         WHERE table_schema = current_schema()
           AND table_name = 'restore_rehearsals'
           AND column_name = 'restore_scope'",
    )
    .fetch_one(&pool)
    .await
    .expect("restore_scope should exist after migration 0015");
    assert_eq!(column.0, "jsonb");
    assert_eq!(column.1, "NO");
    assert!(column
        .2
        .as_deref()
        .is_some_and(|default| default.contains("complete_point")));

    let scope_constraint = sqlx::query_scalar::<_, String>(
        "SELECT pg_get_constraintdef(oid)
         FROM pg_constraint
         WHERE conrelid = 'restore_rehearsals'::regclass
           AND pg_get_constraintdef(oid) LIKE '%selected_paths%'",
    )
    .fetch_one(&pool)
    .await
    .expect("restore scope constraint should be queryable");
    assert!(scope_constraint.contains("complete_point"));
    assert!(scope_constraint.contains("selected_paths"));

    let verification_constraint = sqlx::query_scalar::<_, String>(
        "SELECT pg_get_constraintdef(oid)
         FROM pg_constraint
         WHERE conrelid = 'verification_records'::regclass
           AND conname = 'verification_records_kind_check'",
    )
    .fetch_one(&pool)
    .await
    .expect("verification kind constraint should be queryable");
    assert!(verification_constraint.contains("restore_test"));
    assert!(verification_constraint.contains("selective_restore_test"));

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("isolated migration schema should be removed");
    admin.close().await;
}
