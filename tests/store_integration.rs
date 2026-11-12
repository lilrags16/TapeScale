use std::time::Duration;

use tapescale::store::{self, NewMedia};

#[tokio::test]
async fn fresh_database_is_empty_and_cartridge_registration_is_durable() {
    let Ok(database_url) = std::env::var("TAPESCALE_TEST_DATABASE_URL") else {
        eprintln!("TAPESCALE_TEST_DATABASE_URL is not set; skipping PostgreSQL integration test");
        return;
    };

    let pool = store::connect(&database_url, Duration::from_secs(10))
        .await
        .expect("test database should connect and migrate");

    let overview = store::overview_counts(&pool)
        .await
        .expect("overview query should succeed");
    assert_eq!(overview.protected_sources, 0);
    assert_eq!(overview.active_jobs, 0);
    assert_eq!(overview.required_actions, 0);
    assert!(store::list_sources(&pool)
        .await
        .expect("sources query")
        .is_empty());
    assert!(store::list_jobs(&pool)
        .await
        .expect("jobs query")
        .is_empty());
    assert!(store::list_media(&pool)
        .await
        .expect("media query")
        .is_empty());
    assert!(store::list_drives(&pool)
        .await
        .expect("drives query")
        .is_empty());
    assert!(store::list_slots(&pool)
        .await
        .expect("slots query")
        .is_empty());
    assert!(store::list_movements(&pool)
        .await
        .expect("movements query")
        .is_empty());
    assert!(store::list_verification(&pool)
        .await
        .expect("verification query")
        .is_empty());
    assert!(store::list_recovery_points(&pool)
        .await
        .expect("recovery-points query")
        .is_empty());

    let request = NewMedia {
        internal_label: "INTEGRATION-L9-000001".to_owned(),
        barcode: Some("INT9000001".to_owned()),
        media_generation: 9,
        media_format: "LTO-9".to_owned(),
        worm: true,
    };
    let created = store::create_media(&pool, &request)
        .await
        .expect("cartridge should be inserted");
    assert_eq!(created.internal_label, request.internal_label);
    assert_eq!(created.identity_status, "unknown");
    assert_eq!(created.health_status, "unknown");
    assert!(created.worm);
    assert!(!created.has_content);
    assert_eq!(created.active_holds, 0);

    let reloaded = store::list_media(&pool)
        .await
        .expect("created cartridge should reload");
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].id, created.id);

    let audit_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_events WHERE action = 'media.register' AND object_id = $1",
    )
    .bind(created.id.to_string())
    .fetch_one(&pool)
    .await
    .expect("audit query should succeed");
    assert_eq!(audit_count, 1);

    let duplicate = store::create_media(&pool, &request)
        .await
        .expect_err("duplicate cartridge must be rejected");
    assert_eq!(
        duplicate
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("23505")
    );
}
