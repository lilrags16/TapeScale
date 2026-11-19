use std::{borrow::Cow, time::Duration};

use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use url::Url;
use uuid::Uuid;

const RETIREMENT_REASON: &str =
    "Bacula media declared non-recoverable during TapeScale Native v1 migration";

#[tokio::test]
async fn retired_legacy_media_upgrade_preserves_history_without_protecting_native_media() {
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
    let schema = format!("migration_0020_{}", Uuid::now_v7().simple());
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
    through_0019
        .run(&pool)
        .await
        .expect("alpha.1 schema should migrate through immutable migration 0019");

    let retirement_column_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
             SELECT 1
               FROM information_schema.columns
              WHERE table_schema = current_schema()
                AND table_name = 'volume_records'
                AND column_name = 'retired_at'
         )",
    )
    .fetch_one(&pool)
    .await
    .expect("alpha.1 volume schema should be inspectable");
    assert!(
        !retirement_column_exists,
        "migration 0019 must remain byte-compatible with the alpha.1 tag"
    );

    sqlx::raw_sql(
        r#"
        INSERT INTO truenas_instances
            (id, name, websocket_url, certificate_fingerprint, credential_secret_ref)
        VALUES
            ('00000000-0000-0000-0000-000000002001', '0020 fixture',
             'wss://fixture.invalid', 'fixture', 'fixture');
        INSERT INTO sources
            (id, truenas_instance_id, dataset_guid, dataset_name, source_type,
             logical_root, selection, scope_hash)
        VALUES
            ('00000000-0000-0000-0000-000000002002',
             '00000000-0000-0000-0000-000000002001', 'dataset-0020',
             'pool/fixture', 'filesystem', '/fixture', '{}', 'fixture-scope');
        INSERT INTO jobs (id, external_id, kind)
        VALUES
            ('00000000-0000-0000-0000-000000002003', '0020-legacy', 'backup'),
            ('00000000-0000-0000-0000-000000002004', '0020-native', 'backup');
        INSERT INTO job_runs (id, job_id, state, correlation_id, finished_at)
        VALUES
            ('00000000-0000-0000-0000-000000002005',
             '00000000-0000-0000-0000-000000002003', 'failed',
             '00000000-0000-0000-0000-000000002105', now()),
            ('00000000-0000-0000-0000-000000002006',
             '00000000-0000-0000-0000-000000002004', 'verified',
             '00000000-0000-0000-0000-000000002106', now());
        INSERT INTO cartridges
            (id, internal_label, media_generation, media_format)
        VALUES
            ('00000000-0000-0000-0000-000000002007', 'LEGACY020', 9, 'LTO-9'),
            ('00000000-0000-0000-0000-000000002008', 'NATIVE020', 9, 'LTO-9');
        INSERT INTO recovery_points
            (id, source_id, run_id, representation, level, source_scope_hash,
             format_version, status, recovery_time, indefinite, coverage,
             manifest_digest, archive_id, archive_codec)
        VALUES
            ('00000000-0000-0000-0000-000000002009',
             '00000000-0000-0000-0000-000000002002',
             '00000000-0000-0000-0000-000000002005', 'file', 'full',
             'fixture-scope', 'bacula-community-15.0.3-lab-v1', 'failed', now(),
             true, '{"legacy_format_retired":true}', repeat('b', 64),
             '00000000-0000-0000-0000-000000002109', 'legacy_bacula'),
            ('00000000-0000-0000-0000-000000002010',
             '00000000-0000-0000-0000-000000002002',
             '00000000-0000-0000-0000-000000002006', 'file', 'full',
             'fixture-scope', 'tapescale-native-v1', 'verified', now(),
             true, '{"native_fixture":true}', repeat('a', 64),
             '00000000-0000-0000-0000-000000002110', 'dar');
        INSERT INTO volume_records
            (id, recovery_point_id, cartridge_id, volume_order, first_record,
             last_record, encoded_bytes, finalized, engine_record_id,
             archive_volume_id)
        VALUES
            ('00000000-0000-0000-0000-000000002011',
             '00000000-0000-0000-0000-000000002009',
             '00000000-0000-0000-0000-000000002007', 1, 'bacula-file-0',
             'bacula-file-1', 1024, true, 'bacula:volume:0020', NULL),
            ('00000000-0000-0000-0000-000000002012',
             '00000000-0000-0000-0000-000000002010',
             '00000000-0000-0000-0000-000000002008', 1, 'native-file-0',
             'native-file-1', 2048, true, 'native:volume:0020',
             '00000000-0000-0000-0000-000000002112');
        "#,
    )
    .execute(&pool)
    .await
    .expect("alpha.1 legacy and native volume fixtures should persist");

    all.run(&pool)
        .await
        .expect("0020 should retire only non-recoverable Bacula volume history");

    let volumes = sqlx::query_as::<_, (String, bool, Option<String>)>(
        "SELECT recovery.archive_codec, volume.retired_at IS NOT NULL,
                volume.retirement_reason
           FROM volume_records volume
           JOIN recovery_points recovery ON recovery.id = volume.recovery_point_id
          ORDER BY recovery.archive_codec DESC",
    )
    .fetch_all(&pool)
    .await
    .expect("retired and active volume history should remain queryable");
    assert_eq!(
        volumes,
        vec![
            (
                "legacy_bacula".to_owned(),
                true,
                Some(RETIREMENT_REASON.to_owned())
            ),
            ("dar".to_owned(), false, None),
        ]
    );

    let active_content = sqlx::query_as::<_, (String, bool)>(
        "SELECT cartridge.internal_label,
                EXISTS (
                    SELECT 1 FROM volume_records volume
                     WHERE volume.cartridge_id = cartridge.id
                       AND volume.retired_at IS NULL
                ) AS has_active_content
           FROM cartridges cartridge
          WHERE cartridge.id IN (
              '00000000-0000-0000-0000-000000002007',
              '00000000-0000-0000-0000-000000002008'
          )
          ORDER BY cartridge.internal_label",
    )
    .fetch_all(&pool)
    .await
    .expect("active-content semantics should be queryable");
    assert_eq!(
        active_content,
        vec![
            ("LEGACY020".to_owned(), false),
            ("NATIVE020".to_owned(), true)
        ]
    );

    let invalid_pair = sqlx::query(
        "UPDATE volume_records
            SET retirement_reason = 'reason without timestamp'
          WHERE id = '00000000-0000-0000-0000-000000002012'",
    )
    .execute(&pool)
    .await;
    assert!(
        invalid_pair.is_err(),
        "retirement timestamp and reason must always be set together"
    );

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("isolated migration schema should be removed");
    admin.close().await;
}
