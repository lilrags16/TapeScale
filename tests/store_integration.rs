use std::{path::PathBuf, time::Duration};

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use chrono::Utc;
use serde_json::{json, Value};
use tapescale::{
    api::{self, AppState},
    device::{MissingSasDrive, SasDriveObservation, SasMotionOperation},
    store::{
        self, ClaimDriveOperationResult, ClaimQualificationRunResult,
        ClaimRestoreRehearsalRunResult, ClaimSingleTapeDataRunResult,
        CreateSingleTapeDataProfileResult, DriveOperationTerminalState,
        FinalizeRestoreRehearsalResult, FinalizeSingleTapeDataResult, NewMedia,
        NewSingleTapeDataProfile, QualificationTerminalState, QueueDriveOperationResult,
        QueueQualificationRunResult, QueueSingleTapeDataRunResult, ReconcileDriveResult,
        RestoreRehearsalPhase, SingleTapeDataPhase, SingleTapePassEvidence,
    },
};
use tower::ServiceExt;
use uuid::Uuid;

async fn api_request(
    application: &Router,
    method: Method,
    uri: String,
    body: Option<Value>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    let request_body = match body {
        Some(body) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&body).expect("test request should serialize"))
        }
        None => Body::empty(),
    };
    application
        .clone()
        .oneshot(
            builder
                .body(request_body)
                .expect("test request should build"),
        )
        .await
        .expect("API request should complete")
}

async fn response_json(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("API response body should be readable");
    serde_json::from_slice(&body).expect("API response should be JSON")
}

#[tokio::test]
async fn fresh_database_persists_inventory_and_sas_identity_safely() {
    let Ok(database_url) = std::env::var("TAPESCALE_TEST_DATABASE_URL") else {
        eprintln!("TAPESCALE_TEST_DATABASE_URL is not set; skipping PostgreSQL integration test");
        return;
    };

    let pool = store::connect(&database_url, Duration::from_secs(10))
        .await
        .expect("test database should connect and migrate");

    let mut worker_lock = store::acquire_worker_singleton(&pool)
        .await
        .expect("first worker should acquire the singleton lock");
    assert!(store::acquire_worker_singleton(&pool).await.is_err());
    store::release_worker_singleton(&mut worker_lock)
        .await
        .expect("worker lock should release cleanly");
    drop(worker_lock);

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

    let mut observation = SasDriveObservation {
        stable_id: "serial:IBM:ULTRIUM-HH9:DRIVE-001".to_owned(),
        identity_verified: true,
        identity_conflict: false,
        identity_basis: "vpd_80_serial".to_owned(),
        vendor: Some("IBM".to_owned()),
        model: Some("ULTRIUM-HH9".to_owned()),
        serial: Some("DRIVE-001".to_owned()),
        wwn: None,
        firmware: Some("A1B2".to_owned()),
        generation: None,
        observed_path: "/dev/nst0".to_owned(),
        device_major: 9,
        device_minor: 128,
        scsi_address: Some("1:0:0:0".to_owned()),
        generic_path: None,
        sysfs_state: Some("running".to_owned()),
        driver_options: Some(0),
        immediate_mode: Some(false),
        in_flight: Some(0),
        at_bot: None,
        accessible: true,
        ready: Some(false),
        medium_loaded: Some(false),
        position_lost: Some(false),
        status: "no_media".to_owned(),
        status_detail: Some("drive is accessible without loaded media".to_owned()),
        raw_status: None,
        observed_at: Utc::now(),
    };
    let detected = store::record_drive_observation(&pool, &observation)
        .await
        .expect("drive observation should persist");
    let drive_id = detected.id;
    assert!(!detected.reconciled);
    assert_eq!(detected.generation, None);
    assert_eq!(detected.qualification_status, "detected_unqualified");

    let observed_count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM drives")
        .fetch_one(&pool)
        .await
        .expect("drive count query should succeed");
    assert_eq!(observed_count, 1);

    assert!(matches!(
        store::reconcile_drive(&pool, drive_id, "wrong-stable-id")
            .await
            .expect("mismatch should be a domain outcome"),
        ReconcileDriveResult::Blocked
    ));
    let reconciled = store::reconcile_drive(&pool, drive_id, "serial:IBM:ULTRIUM-HH9:DRIVE-001")
        .await
        .expect("drive reconciliation query should succeed");
    let ReconcileDriveResult::Reconciled(reconciled) = reconciled else {
        panic!("accessible exact identity should reconcile");
    };
    assert!(reconciled.reconciled);
    assert_eq!(reconciled.expected_path.as_deref(), Some("/dev/nst0"));
    assert_eq!(reconciled.generation, None);
    assert_eq!(reconciled.qualification_status, "detected_unqualified");

    let observed_again = store::record_drive_observation(&pool, &observation)
        .await
        .expect("unchanged observation should persist");
    assert!(observed_again.reconciled);

    observation.ready = Some(true);
    observation.medium_loaded = Some(true);
    observation.at_bot = Some(false);
    observation.status = "ready".to_owned();
    observation.observed_at = Utc::now();
    store::record_drive_observation(&pool, &observation)
        .await
        .expect("loaded-media observation should persist");
    assert!(matches!(
        store::queue_drive_operation(
            &pool,
            drive_id,
            "serial:IBM:ULTRIUM-HH9:DRIVE-001",
            SasMotionOperation::Rewind,
            false
        )
        .await
        .expect("scratch-media gate should be a domain outcome"),
        QueueDriveOperationResult::Blocked(_)
    ));
    let malformed_request = sqlx::query(
        "INSERT INTO drive_operations
            (id, drive_id, expected_stable_id, expected_path, action,
             scratch_media_confirmed, requested_by_label, correlation_id)
         VALUES ($1, $2, $3, '/dev/nst0', 'rewind', false, 'test', $4)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(drive_id)
    .bind("serial:IBM:ULTRIUM-HH9:DRIVE-001")
    .bind(uuid::Uuid::now_v7())
    .execute(&pool)
    .await
    .expect_err("database must reject motion without scratch-media confirmation");
    assert_eq!(
        malformed_request
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("23514")
    );
    assert!(matches!(
        store::queue_drive_operation(
            &pool,
            drive_id,
            "wrong-stable-id",
            SasMotionOperation::Rewind,
            true
        )
        .await
        .expect("identity mismatch should be a domain outcome"),
        QueueDriveOperationResult::Blocked(_)
    ));
    let queued = store::queue_drive_operation(
        &pool,
        drive_id,
        "serial:IBM:ULTRIUM-HH9:DRIVE-001",
        SasMotionOperation::Rewind,
        true,
    )
    .await
    .expect("motion request should persist");
    let QueueDriveOperationResult::Queued(queued) = queued else {
        panic!("eligible motion request should queue");
    };
    assert_eq!(queued.action, "rewind");
    assert_eq!(queued.state, "queued");

    let claimed = store::claim_next_drive_operation(&pool, uuid::Uuid::now_v7())
        .await
        .expect("worker claim should succeed");
    let ClaimDriveOperationResult::Claimed(claimed) = claimed else {
        panic!("queued motion should be claimed");
    };
    assert_eq!(claimed.operation.state, "running");
    assert_eq!(
        store::running_drive_operation_paths(&pool)
            .await
            .expect("running paths should query"),
        vec!["/dev/nst0".to_owned()]
    );
    assert!(store::finish_drive_operation(
        &pool,
        &claimed,
        DriveOperationTerminalState::Succeeded,
        json!({"detail":"integration postcondition","position_uncertain":false})
    )
    .await
    .expect("motion completion should persist"));
    assert!(store::running_drive_operation_paths(&pool)
        .await
        .expect("completed path should no longer be active")
        .is_empty());
    let operations = store::list_drive_operations(&pool)
        .await
        .expect("motion history should reload");
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].state, "succeeded");
    let motion_audit = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_events
         WHERE object_type = 'drive_operation' AND object_id = $1",
    )
    .bind(queued.id.to_string())
    .fetch_one(&pool)
    .await
    .expect("motion audit query should succeed");
    assert_eq!(motion_audit, 3);

    assert!(matches!(
        store::queue_qualification_run(
            &pool,
            drive_id,
            "serial:IBM:ULTRIUM-HH9:DRIVE-001",
            created.id,
            &created.internal_label,
            true,
            true,
        )
        .await
        .expect("WORM gate should be a domain outcome"),
        QueueQualificationRunResult::Blocked(_)
    ));
    let scratch_request = NewMedia {
        internal_label: "INTEGRATION-SCRATCH-L9-000001".to_owned(),
        barcode: Some("INT9SCR001".to_owned()),
        media_generation: 9,
        media_format: "LTO-9".to_owned(),
        worm: false,
    };
    let scratch = store::create_media(&pool, &scratch_request)
        .await
        .expect("scratch cartridge should be inserted");
    let qualification_web = tempfile::tempdir().expect("qualification web root should exist");
    let qualification_application = api::router(
        AppState::new(
            Some(pool.clone()),
            false,
            true,
            true,
            true,
            PathBuf::from("/source"),
            PathBuf::from("/restore"),
        ),
        qualification_web.path().to_path_buf(),
    );
    let qualification_idempotency_key = "store-integration-qualification-001";
    let qualification_request = json!({
        "drive_id": drive_id,
        "stable_id": "serial:IBM:ULTRIUM-HH9:DRIVE-001",
        "cartridge_id": scratch.id,
        "cartridge_label": scratch.internal_label,
        "confirmation": format!(
            "DESTROY {} WITH BACULA BTAPE TEST",
            scratch.internal_label
        ),
        "scratch_media_confirmed": true,
        "destructive_overwrite_confirmed": true,
        "idempotency_key": qualification_idempotency_key
    });
    let mut missing_idempotency_key = qualification_request.clone();
    missing_idempotency_key
        .as_object_mut()
        .expect("qualification request is an object")
        .remove("idempotency_key");
    let missing_key_response = api_request(
        &qualification_application,
        Method::POST,
        "/api/v1/qualification/runs".to_owned(),
        Some(missing_idempotency_key),
    )
    .await;
    assert_eq!(missing_key_response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM qualification_runs")
            .fetch_one(&pool)
            .await
            .expect("missing key must not create a qualification run"),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM qualification_events")
            .fetch_one(&pool)
            .await
            .expect("missing key must not append a qualification event"),
        0
    );
    let (first_response, second_response) = tokio::join!(
        api_request(
            &qualification_application,
            Method::POST,
            "/api/v1/qualification/runs".to_owned(),
            Some(qualification_request.clone()),
        ),
        api_request(
            &qualification_application,
            Method::POST,
            "/api/v1/qualification/runs".to_owned(),
            Some(qualification_request.clone()),
        )
    );
    let first_status = first_response.status();
    let second_status = second_response.status();
    assert!(matches!(
        (first_status, second_status),
        (StatusCode::ACCEPTED, StatusCode::OK) | (StatusCode::OK, StatusCode::ACCEPTED)
    ));
    let first_response = response_json(first_response).await;
    let second_response = response_json(second_response).await;
    assert_eq!(first_response["run"]["id"], second_response["run"]["id"]);
    let (queued_response, replay_response) = if first_status == StatusCode::ACCEPTED {
        (&first_response, &second_response)
    } else {
        (&second_response, &first_response)
    };
    assert_eq!(queued_response["idempotent_replay"], false);
    assert_eq!(queued_response["audit"], "appended");
    assert_eq!(queued_response["disposition"], "queued");
    assert_eq!(replay_response["idempotent_replay"], true);
    assert_eq!(replay_response["audit"], "existing");
    assert_eq!(replay_response["disposition"], "existing");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM qualification_runs WHERE idempotency_key = $1"
        )
        .bind(qualification_idempotency_key)
        .fetch_one(&pool)
        .await
        .expect("idempotent qualification row count should query"),
        1
    );
    let qualification_id = Uuid::parse_str(
        queued_response["run"]["id"]
            .as_str()
            .expect("queued run should expose its id"),
    )
    .expect("queued qualification id should parse");
    let queued_qualification = store::get_qualification_run(&pool, qualification_id)
        .await
        .expect("qualification request should reload")
        .expect("qualification request should persist");
    assert_eq!(queued_qualification.state, "queued");
    assert_eq!(queued_qualification.engine_version, "15.0.3");
    let mut changed_qualification_request = qualification_request.clone();
    changed_qualification_request["cartridge_id"] = json!(created.id);
    let changed_replay = api_request(
        &qualification_application,
        Method::POST,
        "/api/v1/qualification/runs".to_owned(),
        Some(changed_qualification_request),
    )
    .await;
    assert_eq!(changed_replay.status(), StatusCode::CONFLICT);
    assert_eq!(
        store::qualification_events(&pool, queued_qualification.id)
            .await
            .expect("idempotent retry should preserve events")
            .iter()
            .filter(|event| event.event_type == "queued")
            .count(),
        1
    );
    assert!(matches!(
        store::queue_drive_operation(
            &pool,
            drive_id,
            "serial:IBM:ULTRIUM-HH9:DRIVE-001",
            SasMotionOperation::Rewind,
            true,
        )
        .await
        .expect("qualification should block a competing motion"),
        QueueDriveOperationResult::Blocked(_)
    ));
    let claimed_qualification = store::claim_next_qualification_run(&pool, uuid::Uuid::now_v7())
        .await
        .expect("qualification claim should succeed");
    let ClaimQualificationRunResult::Claimed(claimed_qualification) = claimed_qualification else {
        panic!("queued qualification should be claimed");
    };
    assert!(
        store::renew_qualification_run_lease(&pool, &claimed_qualification)
            .await
            .expect("qualification lease should renew")
    );
    assert!(store::finish_qualification_run(
        &pool,
        &claimed_qualification,
        QualificationTerminalState::Passed,
        json!({
            "detail":"integration btape evidence fixture",
            "position_uncertain":false,
            "engine_version":"15.0.3"
        }),
    )
    .await
    .expect("qualification completion should persist"));
    let completed_qualification = store::get_qualification_run(&pool, queued_qualification.id)
        .await
        .expect("qualification should reload")
        .expect("qualification remains in history");
    assert_eq!(completed_qualification.state, "passed");
    let terminal_replay = api_request(
        &qualification_application,
        Method::POST,
        "/api/v1/qualification/runs".to_owned(),
        Some(qualification_request.clone()),
    )
    .await;
    assert_eq!(terminal_replay.status(), StatusCode::OK);
    let terminal_replay = response_json(terminal_replay).await;
    assert_eq!(terminal_replay["run"]["id"], json!(queued_qualification.id));
    assert_eq!(terminal_replay["run"]["state"], "passed");
    assert_eq!(terminal_replay["idempotent_replay"], true);
    let disabled_web = tempfile::tempdir().expect("disabled qualification web root should exist");
    let disabled_qualification_application = api::router(
        AppState::new(
            Some(pool.clone()),
            false,
            true,
            false,
            true,
            PathBuf::from("/source"),
            PathBuf::from("/restore"),
        ),
        disabled_web.path().to_path_buf(),
    );
    let disabled_known_replay = api_request(
        &disabled_qualification_application,
        Method::POST,
        "/api/v1/qualification/runs".to_owned(),
        Some(qualification_request.clone()),
    )
    .await;
    assert_eq!(disabled_known_replay.status(), StatusCode::OK);
    let disabled_known_replay = response_json(disabled_known_replay).await;
    assert_eq!(
        disabled_known_replay["run"]["id"],
        json!(queued_qualification.id)
    );
    assert_eq!(disabled_known_replay["run"]["state"], "passed");
    assert_eq!(disabled_known_replay["idempotent_replay"], true);
    let mut disabled_new_request = qualification_request.clone();
    disabled_new_request["idempotency_key"] = json!("disabled-new-qualification-request");
    let disabled_new_response = api_request(
        &disabled_qualification_application,
        Method::POST,
        "/api/v1/qualification/runs".to_owned(),
        Some(disabled_new_request),
    )
    .await;
    assert_eq!(disabled_new_response.status(), StatusCode::CONFLICT);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM qualification_runs")
            .fetch_one(&pool)
            .await
            .expect("disabled gate must not create a qualification run"),
        1
    );
    let qualification_events = store::qualification_events(&pool, queued_qualification.id)
        .await
        .expect("qualification events should reload");
    assert_eq!(
        qualification_events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["queued", "started", "passed"]
    );
    for (suffix, terminal_state, expected_state) in [
        ("failed", QualificationTerminalState::Failed, "failed"),
        ("blocked", QualificationTerminalState::Blocked, "blocked"),
        (
            "interrupted",
            QualificationTerminalState::Interrupted,
            "interrupted",
        ),
    ] {
        let idempotency_key = format!("store-integration-qualification-{suffix}");
        let mut terminal_request = qualification_request.clone();
        terminal_request["idempotency_key"] = json!(idempotency_key);
        let queued_response = api_request(
            &qualification_application,
            Method::POST,
            "/api/v1/qualification/runs".to_owned(),
            Some(terminal_request.clone()),
        )
        .await;
        assert_eq!(queued_response.status(), StatusCode::ACCEPTED);
        let queued_response = response_json(queued_response).await;
        let terminal_run_id = Uuid::parse_str(
            queued_response["run"]["id"]
                .as_str()
                .expect("terminal replay fixture should expose a run id"),
        )
        .expect("terminal replay fixture id should parse");
        let claimed = store::claim_next_qualification_run(&pool, Uuid::now_v7())
            .await
            .expect("terminal replay fixture should be claimable");
        let ClaimQualificationRunResult::Claimed(claimed) = claimed else {
            panic!("terminal replay fixture should be claimed");
        };
        assert_eq!(claimed.run.id, terminal_run_id);
        assert!(store::finish_qualification_run(
            &pool,
            &claimed,
            terminal_state,
            json!({
                "detail": format!("integration {suffix} terminal replay fixture"),
                "position_uncertain": false
            }),
        )
        .await
        .expect("terminal replay fixture should finish"));

        let replay = api_request(
            &qualification_application,
            Method::POST,
            "/api/v1/qualification/runs".to_owned(),
            Some(terminal_request),
        )
        .await;
        assert_eq!(replay.status(), StatusCode::OK);
        let replay = response_json(replay).await;
        assert_eq!(replay["run"]["id"], json!(terminal_run_id));
        assert_eq!(replay["run"]["state"], expected_state);
        assert_eq!(replay["idempotent_replay"], true);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM qualification_runs WHERE idempotency_key = $1"
            )
            .bind(&idempotency_key)
            .fetch_one(&pool)
            .await
            .expect("terminal replay row count should query"),
            1
        );
        assert_eq!(
            store::qualification_events(&pool, terminal_run_id)
                .await
                .expect("terminal replay events should reload")
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["queued", "started", expected_state]
        );
    }
    // The terminal-replay matrix intentionally creates newer non-passing
    // evidence for the exact tuple. Re-establish a latest passing run before
    // the rest of this end-to-end test exercises the qualified data path.
    let mut restored_pass_request = qualification_request.clone();
    restored_pass_request["idempotency_key"] =
        json!("store-integration-qualification-restored-pass");
    let restored_pass = api_request(
        &qualification_application,
        Method::POST,
        "/api/v1/qualification/runs".to_owned(),
        Some(restored_pass_request),
    )
    .await;
    assert_eq!(restored_pass.status(), StatusCode::ACCEPTED);
    let restored_pass = response_json(restored_pass).await;
    let restored_pass_id = Uuid::parse_str(
        restored_pass["run"]["id"]
            .as_str()
            .expect("restored qualification pass should expose a run id"),
    )
    .expect("restored qualification pass id should parse");
    let restored_pass_claim = store::claim_next_qualification_run(&pool, Uuid::now_v7())
        .await
        .expect("restored qualification pass should be claimable");
    let ClaimQualificationRunResult::Claimed(restored_pass_claim) = restored_pass_claim else {
        panic!("restored qualification pass should be claimed");
    };
    assert_eq!(restored_pass_claim.run.id, restored_pass_id);
    assert!(store::finish_qualification_run(
        &pool,
        &restored_pass_claim,
        QualificationTerminalState::Passed,
        json!({
            "detail": "integration qualification restored after terminal replay matrix",
            "position_uncertain": false,
            "engine_version": "15.0.3"
        }),
    )
    .await
    .expect("restored qualification pass should persist"));
    let reservation =
        sqlx::query_scalar::<_, Option<String>>("SELECT reserved_by FROM cartridges WHERE id = $1")
            .bind(scratch.id)
            .fetch_one(&pool)
            .await
            .expect("scratch reservation should reload");
    assert_eq!(reservation, None);
    let qualified_status =
        sqlx::query_scalar::<_, String>("SELECT qualification_status FROM drives WHERE id = $1")
            .bind(drive_id)
            .fetch_one(&pool)
            .await
            .expect("drive qualification status should reload");
    assert_eq!(qualified_status, "detected_unqualified");
    let immutable_event = sqlx::query(
        "UPDATE qualification_events SET evidence = '{}' WHERE qualification_run_id = $1",
    )
    .bind(queued_qualification.id)
    .execute(&pool)
    .await
    .expect_err("qualification evidence must be append-only");
    assert_eq!(
        immutable_event
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("P0001")
    );
    let queued_then_disabled = store::queue_qualification_run(
        &pool,
        drive_id,
        "serial:IBM:ULTRIUM-HH9:DRIVE-001",
        scratch.id,
        &scratch.internal_label,
        true,
        true,
    )
    .await
    .expect("second qualification should persist");
    assert!(matches!(
        queued_then_disabled,
        QueueQualificationRunResult::Queued(_)
    ));
    assert_eq!(
        store::block_queued_qualification_runs(&pool, "qualification gate disabled")
            .await
            .expect("disabled qualification should terminalize queued work"),
        1
    );
    let reservation_after_block =
        sqlx::query_scalar::<_, Option<String>>("SELECT reserved_by FROM cartridges WHERE id = $1")
            .bind(scratch.id)
            .fetch_one(&pool)
            .await
            .expect("blocked scratch reservation should reload");
    assert_eq!(reservation_after_block, None);

    let stale_confirmation = store::queue_qualification_run(
        &pool,
        drive_id,
        "serial:IBM:ULTRIUM-HH9:DRIVE-001",
        scratch.id,
        &scratch.internal_label,
        true,
        true,
    )
    .await
    .expect("stale-confirmation fixture should queue");
    let QueueQualificationRunResult::Queued(stale_confirmation) = stale_confirmation else {
        panic!("stale-confirmation fixture should initially be eligible");
    };
    sqlx::query(
        "UPDATE qualification_runs SET created_at = now() - interval '6 minutes' WHERE id = $1",
    )
    .bind(stale_confirmation.id)
    .execute(&pool)
    .await
    .expect("fixture timestamp should update");
    assert!(matches!(
        store::claim_next_qualification_run(&pool, uuid::Uuid::now_v7())
            .await
            .expect("expired confirmation should be a domain outcome"),
        ClaimQualificationRunResult::Blocked { .. }
    ));
    let reservation_after_expiry =
        sqlx::query_scalar::<_, Option<String>>("SELECT reserved_by FROM cartridges WHERE id = $1")
            .bind(scratch.id)
            .fetch_one(&pool)
            .await
            .expect("expired confirmation should release scratch media");
    assert_eq!(reservation_after_expiry, None);

    let rejected_profile = NewSingleTapeDataProfile {
        name: "Unattested fixture".to_owned(),
        source_subpath: "fixtures/unattested".to_owned(),
        restore_subpath: "exercises/unattested".to_owned(),
        source_mount_path: "/source/fixtures/unattested".to_owned(),
        restore_root_path: "/restore/exercises/unattested".to_owned(),
        immutable_snapshot_confirmed: false,
    };
    assert!(matches!(
        store::create_single_tape_data_profile(&pool, &rejected_profile)
            .await
            .expect("an unattested profile should be a domain outcome"),
        CreateSingleTapeDataProfileResult::Blocked(_)
    ));
    let data_profile = NewSingleTapeDataProfile {
        name: "Integration physical recovery fixture".to_owned(),
        source_subpath: "fixtures/immutable-001".to_owned(),
        restore_subpath: "exercises/integration".to_owned(),
        source_mount_path: "/source/fixtures/immutable-001".to_owned(),
        restore_root_path: "/restore/exercises/integration".to_owned(),
        immutable_snapshot_confirmed: true,
    };
    let created_profile = store::create_single_tape_data_profile(&pool, &data_profile)
        .await
        .expect("the self-contained mounted-source profile should persist");
    let CreateSingleTapeDataProfileResult::Created(created_profile) = created_profile else {
        panic!("an attested contained profile should be accepted");
    };
    assert_eq!(created_profile.source_subpath, "fixtures/immutable-001");
    assert_eq!(created_profile.restore_subpath, "exercises/integration");
    assert_eq!(created_profile.logical_root, "/fixtures/immutable-001");
    assert!(created_profile.immutable_snapshot_confirmed);
    assert!(!created_profile.production_eligible);
    let source_binding = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT source.dataset_name, source.logical_root, snapshot.logical_root,
                snapshot.physical_path
         FROM sources source
         JOIN snapshots snapshot ON snapshot.source_id = source.id
         WHERE source.id = $1 AND snapshot.id = $2",
    )
    .bind(created_profile.source_id)
    .bind(created_profile.source_snapshot_id)
    .fetch_one(&pool)
    .await
    .expect("the profile should create its exact mounted snapshot binding");
    assert_eq!(source_binding.0, "fixtures/immutable-001");
    assert_eq!(source_binding.1, "/fixtures/immutable-001");
    assert_eq!(source_binding.2, "/fixtures/immutable-001");
    assert_eq!(
        source_binding.3.as_deref(),
        Some("/source/fixtures/immutable-001")
    );

    assert!(matches!(
        store::queue_single_tape_data_run(
            &pool,
            created_profile.id,
            drive_id,
            "serial:IBM:ULTRIUM-HH9:DRIVE-001",
            scratch.id,
            &scratch.internal_label,
            true,
            true,
            false,
        )
        .await
        .expect("the encryption deviation gate should be a domain outcome"),
        QueueSingleTapeDataRunResult::Blocked(_)
    ));
    let queued_data_run = store::queue_single_tape_data_run(
        &pool,
        created_profile.id,
        drive_id,
        "serial:IBM:ULTRIUM-HH9:DRIVE-001",
        scratch.id,
        &scratch.internal_label,
        true,
        true,
        true,
    )
    .await
    .expect("the exact passed drive and cartridge should queue a physical data exercise");
    let QueueSingleTapeDataRunResult::Queued(queued_data_run) = queued_data_run else {
        panic!("qualified single-tape data exercise should queue");
    };
    assert_eq!(
        queued_data_run.source_snapshot_id,
        created_profile.source_snapshot_id
    );
    assert!(queued_data_run.encryption_deviation_confirmed);
    assert!(matches!(
        store::queue_single_tape_data_run(
            &pool,
            created_profile.id,
            drive_id,
            "serial:IBM:ULTRIUM-HH9:DRIVE-001",
            scratch.id,
            &scratch.internal_label,
            true,
            true,
            true,
        )
        .await
        .expect("duplicate active work should be a domain outcome"),
        QueueSingleTapeDataRunResult::Blocked(_)
    ));
    assert!(matches!(
        store::queue_drive_operation(
            &pool,
            drive_id,
            "serial:IBM:ULTRIUM-HH9:DRIVE-001",
            SasMotionOperation::Rewind,
            true,
        )
        .await
        .expect("the active data exercise should exclude standalone motion"),
        QueueDriveOperationResult::Blocked(_)
    ));
    let data_claim = store::claim_next_single_tape_data_run(&pool, uuid::Uuid::now_v7())
        .await
        .expect("the worker should claim both exact resources");
    let ClaimSingleTapeDataRunResult::Claimed(data_claim) = data_claim else {
        panic!("the queued physical recovery exercise should be claimable");
    };
    assert!(store::renew_single_tape_data_run_leases(&pool, &data_claim)
        .await
        .expect("both data-run leases should renew"));
    assert!(store::running_drive_operation_paths(&pool)
        .await
        .expect("active physical paths should query")
        .contains(&"/dev/nst0".to_owned()));
    let source_manifest_digest = "d".repeat(64);
    assert!(store::append_single_tape_data_artifact(
        &pool,
        &data_claim,
        "source_manifest",
        "artifacts/source-manifest.json",
        &"a".repeat(64),
        1024,
        json!({"manifest_digest":source_manifest_digest.clone(),"file_count":3}),
    )
    .await
    .expect("source manifest evidence should append"));
    assert!(store::transition_single_tape_data_phase(
        &pool,
        &data_claim,
        SingleTapeDataPhase::Writing,
        json!({"engine":"Bacula Community","operation":"full"}),
    )
    .await
    .expect("the fenced run should enter writing"));
    assert!(store::mark_single_tape_media_write_started(
        &pool,
        &data_claim,
        "INTEGRATION-SCRATCH-L9-000001"
    )
    .await
    .expect("the pre-label boundary should commit atomically"));
    assert!(!store::mark_single_tape_media_write_started(
        &pool,
        &data_claim,
        "INTEGRATION-SCRATCH-L9-000001"
    )
    .await
    .expect("the media-write boundary cannot be committed twice"));
    let (write_started, write_started_at, cartridge_health) =
        sqlx::query_as::<_, (bool, bool, String)>(
            "SELECT data_run.media_write_started,
                    data_run.media_write_started_at IS NOT NULL,
                    cartridge.health_status
             FROM single_tape_data_runs data_run
             JOIN cartridges cartridge ON cartridge.id = data_run.cartridge_id
             WHERE data_run.id = $1",
        )
        .bind(data_claim.run.id)
        .fetch_one(&pool)
        .await
        .expect("the durable media boundary should reload");
    assert!(write_started);
    assert!(write_started_at);
    assert_eq!(cartridge_health, "write_in_progress");
    assert!(store::append_single_tape_data_evidence_event(
        &pool,
        &data_claim,
        "media_prepared",
        json!({"operation":"clear_prior_scratch_header","exit_code":0}),
    )
    .await
    .expect("scratch-label invalidation evidence should append"));
    assert!(store::append_single_tape_data_evidence_event(
        &pool,
        &data_claim,
        "engine_dispatched",
        json!({"operation":"backup","engine_job_id":1001}),
    )
    .await
    .expect("correlated engine dispatch should append"));
    assert!(store::transition_single_tape_data_phase(
        &pool,
        &data_claim,
        SingleTapeDataPhase::Written,
        json!({"engine_volume":"INTEGRATION-SCRATCH-L9-000001"}),
    )
    .await
    .expect("the fenced run should record engine write completion"));
    assert!(store::append_single_tape_data_artifact(
        &pool,
        &data_claim,
        "engine_write",
        "artifacts/engine-write.json",
        &"b".repeat(64),
        512,
        json!({"finalized":true,"engine":"Bacula Community","version":"15.0.3","media_id":2001}),
    )
    .await
    .expect("finalized engine-write evidence should append"));
    assert!(store::transition_single_tape_data_phase(
        &pool,
        &data_claim,
        SingleTapeDataPhase::Verifying,
        json!({"path":"physical_tape_restore"}),
    )
    .await
    .expect("the fenced run should enter physical verification"));
    let pass_evidence = SingleTapePassEvidence {
        engine_operation_id: "bacula-job:1001".to_owned(),
        engine_volume_name: "INTEGRATION-SCRATCH-L9-000001".to_owned(),
        engine_record_id: "bacula-media:2001:job:1001".to_owned(),
        first_record: "file=0:block=0".to_owned(),
        last_record: Some("file=1:block=42".to_owned()),
        logical_bytes: 4096,
        encoded_bytes: 8192,
        file_count: 3,
        coverage: json!({"content_sha256":true,"mode":true,"numeric_owner":true}),
        full_readback_evidence: json!({
            "physical_media_readback":true,
            "engine":"Bacula Community",
            "volume":"INTEGRATION-SCRATCH-L9-000001"
        }),
        restore_test_evidence: json!({
            "matched":true,
            "isolated_destination":"/restore/exercises/integration/test-run"
        }),
        result: json!({"scope":"integration physical round trip"}),
    };
    assert!(matches!(
        store::finalize_single_tape_data_pass(&pool, &data_claim, &pass_evidence)
            .await
            .expect("incomplete evidence should be a domain outcome"),
        FinalizeSingleTapeDataResult::Rejected(_)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM recovery_points")
            .fetch_one(&pool)
            .await
            .expect("recovery-point count should query"),
        0
    );
    assert!(store::transition_single_tape_data_phase(
        &pool,
        &data_claim,
        SingleTapeDataPhase::Restoring,
        json!({"destination":"/restore/exercises/integration/test-run"}),
    )
    .await
    .expect("the fenced run should enter isolated restore"));
    assert!(
        store::overview_counts(&pool)
            .await
            .expect("restoring overview should query")
            .active_jobs
            >= 1
    );
    assert!(store::append_single_tape_data_artifact(
        &pool,
        &data_claim,
        "restored_manifest",
        "artifacts/restored-manifest.json",
        &"a".repeat(64),
        1024,
        json!({"manifest_digest":source_manifest_digest.clone(),"file_count":3}),
    )
    .await
    .expect("restored manifest evidence should append"));
    assert!(store::transition_single_tape_data_phase(
        &pool,
        &data_claim,
        SingleTapeDataPhase::Comparing,
        json!({"method":"normalized_manifest_sha256"}),
    )
    .await
    .expect("the fenced run should enter manifest comparison"));
    assert!(
        store::overview_counts(&pool)
            .await
            .expect("comparing overview should query")
            .active_jobs
            >= 1
    );
    assert!(store::append_single_tape_data_artifact(
        &pool,
        &data_claim,
        "comparison",
        "artifacts/comparison.json",
        &"c".repeat(64),
        384,
        json!({"matched":true,"differences":[]}),
    )
    .await
    .expect("comparison evidence should append"));
    let finalized = store::finalize_single_tape_data_pass(&pool, &data_claim, &pass_evidence)
        .await
        .expect("complete physical recovery evidence should finalize atomically");
    let FinalizeSingleTapeDataResult::Passed {
        recovery_point_id,
        volume_record_id,
    } = finalized
    else {
        panic!("complete matching evidence should create a verified recovery point");
    };
    let legacy_manifest_binding = store::recovery_point_manifest_binding(&pool, recovery_point_id)
        .await
        .expect("G0.2 manifest binding should query")
        .expect("G0.2 recovery point should retain a manifest binding");
    assert_eq!(
        legacy_manifest_binding.manifest_digest.as_deref(),
        Some(source_manifest_digest.as_str())
    );
    assert_eq!(
        legacy_manifest_binding.artifact_sha256.as_deref(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert!(matches!(
        store::finalize_single_tape_data_pass(&pool, &data_claim, &pass_evidence)
            .await
            .expect("a stale terminal claim should be a domain outcome"),
        FinalizeSingleTapeDataResult::FencedOut
    ));
    let durable_pass = sqlx::query_as::<_, (String, String, bool, String, i64)>(
        "SELECT recovery.status, recovery.representation, volume.finalized,
                verification.kind, count(*) OVER ()
         FROM recovery_points recovery
         JOIN volume_records volume ON volume.recovery_point_id = recovery.id
         JOIN verification_records verification ON verification.recovery_point_id = recovery.id
         WHERE recovery.id = $1 AND volume.id = $2
         ORDER BY verification.kind",
    )
    .bind(recovery_point_id)
    .bind(volume_record_id)
    .fetch_all(&pool)
    .await
    .expect("the pass should expose durable recovery, volume, and verification evidence");
    assert_eq!(durable_pass.len(), 2);
    assert!(durable_pass
        .iter()
        .all(|row| row.0 == "verified" && row.1 == "file" && row.2 && row.4 == 2));
    assert_eq!(
        durable_pass
            .iter()
            .map(|row| row.3.as_str())
            .collect::<Vec<_>>(),
        vec!["full_readback", "restore_test"]
    );
    let (job_state, job_finished, engine_operation_id) =
        sqlx::query_as::<_, (String, bool, Option<String>)>(
            "SELECT state, finished_at IS NOT NULL, engine_operation_id
             FROM job_runs WHERE id = $1",
        )
        .bind(data_claim.run.job_run_id)
        .fetch_one(&pool)
        .await
        .expect("the finalized parent job should reload");
    assert_eq!(job_state, "verified");
    assert!(job_finished);
    assert_eq!(engine_operation_id.as_deref(), Some("bacula-job:1001"));

    let web_directory = tempfile::tempdir().expect("temporary web directory should be created");
    let application = api::router(
        AppState::new(
            Some(pool.clone()),
            false,
            true,
            true,
            true,
            PathBuf::from("/source"),
            PathBuf::from("/restore"),
        ),
        web_directory.path().to_path_buf(),
    );

    let detail_response = api_request(
        &application,
        Method::GET,
        format!("/api/v1/recovery-points/{recovery_point_id}"),
        None,
    )
    .await;
    assert_eq!(detail_response.status(), StatusCode::OK);
    let detail = response_json(detail_response).await;
    assert_eq!(detail["recovery_point"]["id"], json!(recovery_point_id));
    assert_eq!(detail["recovery_point"]["status"], "verified");
    assert_eq!(detail["volumes"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        detail["volumes"][0]["internal_label"],
        "INTEGRATION-SCRATCH-L9-000001"
    );
    assert_eq!(detail["volumes"][0]["volume_order"], 1);
    assert_eq!(detail["volumes"][0]["finalized"], true);
    assert_eq!(detail["verifications"].as_array().map(Vec::len), Some(2));
    assert_eq!(detail["origin"]["run_id"], json!(data_claim.run.job_run_id));
    assert_eq!(detail["origin"]["data_run_id"], json!(data_claim.run.id));
    assert!(detail["events"]
        .as_array()
        .is_some_and(|events| !events.is_empty()));
    assert!(detail["artifacts"]
        .as_array()
        .is_some_and(|artifacts| artifacts.len() >= 3));
    assert!(
        !serde_json::to_string(&detail)
            .expect("detail should serialize")
            .contains("storage_ref"),
        "public recovery detail must never expose artifact storage paths"
    );

    let evidence_response = api_request(
        &application,
        Method::GET,
        format!("/api/v1/recovery-points/{recovery_point_id}/evidence"),
        None,
    )
    .await;
    assert_eq!(evidence_response.status(), StatusCode::OK);
    assert_eq!(
        evidence_response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json; charset=utf-8")
    );
    assert!(evidence_response
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.contains(&format!(
                "tapescale-recovery-point-{recovery_point_id}-evidence.json"
            ))
        }));
    assert!(evidence_response
        .headers()
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("\"sha256-") && value.ends_with('"')));
    let evidence_bytes = to_bytes(evidence_response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("evidence response should be bounded and readable");
    let evidence_text = std::str::from_utf8(&evidence_bytes).expect("evidence should be UTF-8");
    assert!(!evidence_text.contains("storage_ref"));
    assert!(
        !evidence_text.contains('\n'),
        "canonical JSON should be compact"
    );
    let evidence: Value =
        serde_json::from_slice(&evidence_bytes).expect("evidence should be valid JSON");
    assert_eq!(evidence["format"], "tapescale.recovery-point-evidence.v1");
    assert_eq!(evidence["recovery_point"]["id"], json!(recovery_point_id));

    let plans_before = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM restore_plans")
        .fetch_one(&pool)
        .await
        .expect("restore-plan count should query");
    let traversal_response = api_request(
        &application,
        Method::POST,
        "/api/v1/restores/plan".to_owned(),
        Some(json!({
            "recovery_point_id": recovery_point_id,
            "destination": "../outside-the-recovery-root"
        })),
    )
    .await;
    assert_eq!(traversal_response.status(), StatusCode::BAD_REQUEST);
    let traversal_error = response_json(traversal_response).await;
    assert_eq!(traversal_error["error"]["code"], "invalid_request");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM restore_plans")
            .fetch_one(&pool)
            .await
            .expect("rejected plan must not persist"),
        plans_before
    );

    let plan_response = api_request(
        &application,
        Method::POST,
        "/api/v1/restores/plan".to_owned(),
        Some(json!({
            "recovery_point_id": recovery_point_id,
            "mode": "file_archive",
            "destination": "planned-restores/integration-recovery",
            "conflict_policy": "skip",
            "destination_approved": true,
            "is_new_destination": true,
            "acl_semantics_supported": true,
            "identity_mapping_complete": true,
            "estimated_required_bytes": 4096,
            "available_bytes": 1048576,
            "required_free_reserve_bytes": 4096,
            "automatic_publication_requested": false
        })),
    )
    .await;
    assert_eq!(plan_response.status(), StatusCode::CREATED);
    let plan_response = response_json(plan_response).await;
    let plan = &plan_response["plan"];
    assert_eq!(plan["execution_supported"], false);
    assert_eq!(plan["ready"], false);
    assert_eq!(
        plan["required_tape_ids"],
        json!(["INTEGRATION-SCRATCH-L9-000001"])
    );
    assert_eq!(
        plan["verification_prerequisites"]["full_readback_passed"],
        true
    );
    assert_eq!(
        plan["verification_prerequisites"]["restore_test_passed"],
        true
    );
    let blocker_codes = plan["blockers"]
        .as_array()
        .expect("blocked plan should return blocker objects")
        .iter()
        .filter_map(|blocker| blocker["code"].as_str())
        .collect::<Vec<_>>();
    assert!(blocker_codes.contains(&"tape_location_unknown"));
    assert!(!blocker_codes.contains(&"no_qualified_compatible_drive"));
    assert!(plan["volumes"][0]["compatible_qualified_drives"]
        .as_array()
        .is_some_and(|drives| !drives.is_empty()));
    assert!(plan["preview_hash"]
        .as_str()
        .is_some_and(|digest| digest.len() == 64));
    assert!(plan["expires_at"].as_str().is_some());
    let persisted_plan = sqlx::query_as::<_, (Value, Value, String, bool)>(
        "SELECT selection, blockers, preview_hash,
                expires_at > created_at AND expires_at <= created_at + interval '31 minutes'
         FROM restore_plans WHERE id = $1",
    )
    .bind(
        uuid::Uuid::parse_str(
            plan["id"]
                .as_str()
                .expect("created plan should return a UUID"),
        )
        .expect("plan id should parse"),
    )
    .fetch_one(&pool)
    .await
    .expect("restore plan should persist transactionally");
    assert_eq!(persisted_plan.0["execution_supported"], false);
    assert_eq!(persisted_plan.1, plan["blockers"]);
    assert_eq!(persisted_plan.2, plan["preview_hash"]);
    assert!(persisted_plan.3);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_events
             WHERE action = 'restore.plan' AND object_id = $1",
        )
        .bind(plan["id"].as_str().expect("plan id should be text"))
        .fetch_one(&pool)
        .await
        .expect("restore-plan audit should query"),
        1
    );

    let onsite_response = api_request(
        &application,
        Method::POST,
        "/api/v1/locations".to_owned(),
        Some(json!({
            "name": "Integration library room",
            "kind": "onsite",
            "details": {"purpose":"primary tape storage"},
            "actor": "integration operator"
        })),
    )
    .await;
    assert_eq!(onsite_response.status(), StatusCode::CREATED);
    let onsite = response_json(onsite_response).await;
    let onsite_id = uuid::Uuid::parse_str(
        onsite["location"]["id"]
            .as_str()
            .expect("created location should have an id"),
    )
    .expect("onsite id should parse");
    let vault_response = api_request(
        &application,
        Method::POST,
        "/api/v1/locations".to_owned(),
        Some(json!({
            "name": "Integration off-site vault",
            "kind": "offsite",
            "actor": "integration operator"
        })),
    )
    .await;
    assert_eq!(vault_response.status(), StatusCode::CREATED);
    let vault = response_json(vault_response).await;
    let vault_id = uuid::Uuid::parse_str(
        vault["location"]["id"]
            .as_str()
            .expect("created vault should have an id"),
    )
    .expect("vault id should parse");

    let onboarding_body = json!({
        "idempotency_key": "integration-create-onboard-001",
        "kind": "return",
        "source_location_id": null,
        "destination_location_id": onsite_id,
        "cartridge_ids": [scratch.id, created.id],
        "reason": "Establish confirmed custody for registered cartridges",
        "due_at": Utc::now() + chrono::TimeDelta::hours(1),
        "actor": "integration operator"
    });
    let onboarding_response = api_request(
        &application,
        Method::POST,
        "/api/v1/movements".to_owned(),
        Some(onboarding_body.clone()),
    )
    .await;
    assert_eq!(onboarding_response.status(), StatusCode::CREATED);
    let onboarding = response_json(onboarding_response).await;
    assert_eq!(onboarding["movement"]["summary"]["state"], "ready");
    assert_eq!(
        onboarding["movement"]["summary"]["source_name"],
        Value::Null
    );
    assert_eq!(onboarding["movement"]["summary"]["media_total"], 2);
    let onboarding_id = onboarding["movement"]["summary"]["id"]
        .as_str()
        .expect("onboarding movement should return an id")
        .to_owned();
    let onboarding_create_replay = api_request(
        &application,
        Method::POST,
        "/api/v1/movements".to_owned(),
        Some(onboarding_body),
    )
    .await;
    assert_eq!(onboarding_create_replay.status(), StatusCode::CREATED);
    let onboarding_create_replay = response_json(onboarding_create_replay).await;
    assert_eq!(
        onboarding_create_replay["receipt"]["idempotent_replay"],
        true
    );
    assert_eq!(
        onboarding_create_replay["movement"]["summary"]["id"],
        onboarding_id
    );
    let onboarding_confirm = json!({
        "action": "direct",
        "cartridge_ids": [scratch.id, created.id],
        "expected_confirmation_version": 0,
        "idempotency_key": "integration-confirm-onboard-001",
        "physically_confirmed": true,
        "note": "Both physical labels checked in the library room",
        "actor": "integration operator"
    });
    let onboarding_confirmation = api_request(
        &application,
        Method::POST,
        format!("/api/v1/movements/{onboarding_id}/confirm"),
        Some(onboarding_confirm.clone()),
    )
    .await;
    assert_eq!(onboarding_confirmation.status(), StatusCode::OK);
    let onboarding_confirmation = response_json(onboarding_confirmation).await;
    assert_eq!(
        onboarding_confirmation["movement"]["summary"]["state"],
        "confirmed"
    );
    assert_eq!(
        onboarding_confirmation["receipt"]["confirmation_version"],
        1
    );
    assert_eq!(
        onboarding_confirmation["receipt"]["idempotent_replay"],
        false
    );

    let replay = api_request(
        &application,
        Method::POST,
        format!("/api/v1/movements/{onboarding_id}/confirm"),
        Some(onboarding_confirm),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    let replay = response_json(replay).await;
    assert_eq!(replay["receipt"]["idempotent_replay"], true);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM custody_events
             WHERE movement_task_id = $1 AND event_type = 'direct_move_confirmed'",
        )
        .bind(uuid::Uuid::parse_str(&onboarding_id).expect("movement id should parse"))
        .fetch_one(&pool)
        .await
        .expect("idempotent custody event count should query"),
        2
    );

    let offsite_response = api_request(
        &application,
        Method::POST,
        "/api/v1/movements".to_owned(),
        Some(json!({
            "external_id": "INT-MOVE-OFFSITE-001",
            "idempotency_key": "integration-create-offsite-001",
            "kind": "take_offsite",
            "source_location_id": onsite_id,
            "destination_location_id": vault_id,
            "cartridge_ids": [scratch.id, created.id],
            "reason": "Prove partial and complete off-site custody",
            "due_at": Utc::now() + chrono::TimeDelta::hours(1),
            "actor": "integration operator"
        })),
    )
    .await;
    assert_eq!(offsite_response.status(), StatusCode::CREATED);
    let offsite = response_json(offsite_response).await;
    let offsite_id = offsite["movement"]["summary"]["id"]
        .as_str()
        .expect("off-site movement should return an id")
        .to_owned();

    let first_delivery = api_request(
        &application,
        Method::POST,
        format!("/api/v1/movements/{offsite_id}/confirm"),
        Some(json!({
            "action": "direct",
            "cartridge_ids": [scratch.id],
            "expected_confirmation_version": 0,
            "idempotency_key": "integration-offsite-partial-001",
            "physically_confirmed": true,
            "note": "First cartridge received at vault",
            "actor": "vault operator"
        })),
    )
    .await;
    assert_eq!(first_delivery.status(), StatusCode::OK);
    let first_delivery = response_json(first_delivery).await;
    assert_eq!(
        first_delivery["movement"]["summary"]["state"],
        "partially_confirmed"
    );
    assert_eq!(first_delivery["movement"]["summary"]["media_confirmed"], 1);
    assert_eq!(first_delivery["movement"]["summary"]["media_planned"], 1);
    let locations_after_partial =
        sqlx::query_as::<_, (uuid::Uuid, Option<uuid::Uuid>, Option<String>)>(
            "SELECT id, confirmed_location_id, reserved_by
         FROM cartridges WHERE id = ANY($1::uuid[]) ORDER BY id",
        )
        .bind(vec![scratch.id, created.id])
        .fetch_all(&pool)
        .await
        .expect("partial custody projection should query");
    assert!(locations_after_partial
        .iter()
        .any(|row| { row.0 == scratch.id && row.1 == Some(vault_id) && row.2.is_none() }));
    assert!(locations_after_partial.iter().any(|row| {
        row.0 == created.id
            && row.1 == Some(onsite_id)
            && row
                .2
                .as_deref()
                .is_some_and(|reservation| reservation.starts_with("movement:"))
    }));

    let completed_delivery = api_request(
        &application,
        Method::POST,
        format!("/api/v1/movements/{offsite_id}/confirm"),
        Some(json!({
            "action": "direct",
            "cartridge_ids": [created.id],
            "expected_confirmation_version": 1,
            "idempotency_key": "integration-offsite-complete-001",
            "physically_confirmed": true,
            "note": "Second cartridge received at vault",
            "actor": "vault operator"
        })),
    )
    .await;
    assert_eq!(completed_delivery.status(), StatusCode::OK);
    let completed_delivery = response_json(completed_delivery).await;
    assert_eq!(
        completed_delivery["movement"]["summary"]["state"],
        "confirmed"
    );
    assert_eq!(
        completed_delivery["movement"]["summary"]["media_confirmed"],
        2
    );

    // Restore execution requires an explicit LTO read-matrix result in addition
    // to the exact physical qualification. Model inference supplies this in a
    // real probe; make the synthetic HH9 fixture equally explicit.
    observation.generation = Some(9);
    observation.observed_at = Utc::now();
    store::record_drive_observation(&pool, &observation)
        .await
        .expect("restore fixture should retain the observed LTO generation");
    assert!(matches!(
        store::reconcile_drive(&pool, drive_id, "serial:IBM:ULTRIUM-HH9:DRIVE-001")
            .await
            .expect("restore fixture drive identity should reconcile"),
        ReconcileDriveResult::Reconciled(_)
    ));

    let ready_plan_response = api_request(
        &application,
        Method::POST,
        "/api/v1/restores/plan".to_owned(),
        Some(json!({
            "recovery_point_id": recovery_point_id,
            "mode": "file_archive",
            "destination": "planned-restores/integration-ready-recovery",
            "conflict_policy": "skip",
            "destination_approved": true,
            "is_new_destination": true,
            "acl_semantics_supported": true,
            "identity_mapping_complete": true,
            "estimated_required_bytes": 4096,
            "available_bytes": 1048576,
            "required_free_reserve_bytes": 4096,
            "automatic_publication_requested": false
        })),
    )
    .await;
    assert_eq!(ready_plan_response.status(), StatusCode::CREATED);
    let ready_plan = response_json(ready_plan_response).await;
    assert_eq!(ready_plan["plan"]["ready"], true);
    assert_eq!(ready_plan["plan"]["blockers"], json!([]));
    assert_eq!(
        ready_plan["plan"]["volumes"][0]["location_name"],
        "Integration off-site vault"
    );
    assert!(
        ready_plan["plan"]["volumes"][0]["compatible_qualified_drives"]
            .as_array()
            .is_some_and(|drives| !drives.is_empty())
    );
    assert_eq!(ready_plan["plan"]["execution_supported"], true);
    assert_eq!(
        ready_plan["plan"]["execution"]["original_drive_id"],
        json!(drive_id)
    );
    let ready_plan_id = uuid::Uuid::parse_str(
        ready_plan["plan"]["id"]
            .as_str()
            .expect("ready restore plan should have an id"),
    )
    .expect("ready restore plan id should parse");
    let ready_preview_hash = ready_plan["plan"]["preview_hash"]
        .as_str()
        .expect("ready restore plan should have a preview hash")
        .to_owned();
    let ready_confirmation = ready_plan["plan"]["confirmation_phrase"]
        .as_str()
        .expect("ready restore plan should have an exact confirmation phrase")
        .to_owned();
    let queue_rehearsal_body = json!({
        "plan_id": ready_plan_id,
        "preview_hash": ready_preview_hash,
        "drive_id": drive_id,
        "confirmation": ready_confirmation,
        "tape_loaded_confirmed": true,
        "isolated_destination_confirmed": true,
        "attended_confirmed": true,
        "actor": "integration recovery operator"
    });
    let oversized_manifest_bytes = i64::try_from(store::RESTORE_MANIFEST_ARTIFACT_MAX_BYTES + 1)
        .expect("fixed manifest limit plus one fits i64");
    sqlx::query(
        "ALTER TABLE single_tape_data_artifacts
         DISABLE TRIGGER single_tape_data_artifacts_append_only",
    )
    .execute(&pool)
    .await
    .expect("integration fixture should temporarily expose a legacy oversized artifact");
    assert_eq!(
        sqlx::query(
            "UPDATE single_tape_data_artifacts SET size_bytes = $1
             WHERE single_tape_data_run_id = $2 AND artifact_kind = 'source_manifest'",
        )
        .bind(oversized_manifest_bytes)
        .bind(data_claim.run.id)
        .execute(&pool)
        .await
        .expect("legacy oversized manifest fixture should persist")
        .rows_affected(),
        1
    );
    let oversized_queue_response = api_request(
        &application,
        Method::POST,
        "/api/v1/restore-rehearsals".to_owned(),
        Some(queue_rehearsal_body.clone()),
    )
    .await;
    let oversized_plan_response = api_request(
        &application,
        Method::POST,
        "/api/v1/restores/plan".to_owned(),
        Some(json!({
            "recovery_point_id": recovery_point_id,
            "mode": "file_archive",
            "destination": "planned-restores/integration-oversized-manifest",
            "conflict_policy": "skip",
            "destination_approved": true,
            "is_new_destination": true,
            "acl_semantics_supported": true,
            "identity_mapping_complete": true,
            "estimated_required_bytes": 4096,
            "available_bytes": 1048576,
            "required_free_reserve_bytes": 4096,
            "automatic_publication_requested": false
        })),
    )
    .await;
    assert_eq!(
        sqlx::query(
            "UPDATE single_tape_data_artifacts SET size_bytes = 1024
             WHERE single_tape_data_run_id = $1 AND artifact_kind = 'source_manifest'",
        )
        .bind(data_claim.run.id)
        .execute(&pool)
        .await
        .expect("integration fixture should restore the supported manifest size")
        .rows_affected(),
        1
    );
    sqlx::query(
        "ALTER TABLE single_tape_data_artifacts
         ENABLE TRIGGER single_tape_data_artifacts_append_only",
    )
    .execute(&pool)
    .await
    .expect("append-only artifact enforcement should be restored");
    assert_eq!(oversized_queue_response.status(), StatusCode::CONFLICT);
    let oversized_queue = response_json(oversized_queue_response).await;
    assert!(oversized_queue["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("attended restore supports at most")));
    assert_eq!(oversized_plan_response.status(), StatusCode::CREATED);
    let oversized_plan = response_json(oversized_plan_response).await;
    assert_eq!(oversized_plan["plan"]["ready"], false);
    assert_eq!(oversized_plan["plan"]["execution_supported"], false);
    assert_eq!(
        oversized_plan["plan"]["execution"]["source_manifest_artifact"]["size_bytes"],
        oversized_manifest_bytes
    );
    assert_eq!(
        oversized_plan["plan"]["execution"]["source_manifest_artifact"]["maximum_supported_bytes"],
        store::RESTORE_MANIFEST_ARTIFACT_MAX_BYTES
    );
    assert_eq!(
        oversized_plan["plan"]["execution"]["source_manifest_artifact"]["supported"],
        false
    );
    assert!(oversized_plan["plan"]["blockers"]
        .as_array()
        .is_some_and(|blockers| blockers.iter().any(|blocker| {
            blocker["code"] == "source_manifest_artifact_too_large"
                && blocker["size_bytes"] == oversized_manifest_bytes
        })));
    assert!(oversized_plan["plan"]["execution_detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("attended restore supports artifacts")));
    let stale_rehearsal_response = api_request(
        &application,
        Method::POST,
        "/api/v1/restore-rehearsals".to_owned(),
        Some(json!({
            "plan_id": ready_plan_id,
            "preview_hash": "0".repeat(64),
            "drive_id": drive_id,
            "confirmation": format!("RESTORE PLAN {ready_plan_id} TO ISOLATED DESTINATION"),
            "tape_loaded_confirmed": true,
            "isolated_destination_confirmed": true,
            "attended_confirmed": true,
            "actor": "integration recovery operator"
        })),
    )
    .await;
    assert_eq!(stale_rehearsal_response.status(), StatusCode::CONFLICT);

    let queued_rehearsal_response = api_request(
        &application,
        Method::POST,
        "/api/v1/restore-rehearsals".to_owned(),
        Some(queue_rehearsal_body.clone()),
    )
    .await;
    let queued_rehearsal_status = queued_rehearsal_response.status();
    let queued_rehearsal = response_json(queued_rehearsal_response).await;
    assert_eq!(
        queued_rehearsal_status,
        StatusCode::ACCEPTED,
        "queue response: {queued_rehearsal}"
    );
    let cancelled_rehearsal_id = uuid::Uuid::parse_str(
        queued_rehearsal["rehearsal"]["id"]
            .as_str()
            .expect("queued restore rehearsal should have an id"),
    )
    .expect("queued restore rehearsal id should parse");
    assert_eq!(queued_rehearsal["rehearsal"]["state"], "queued");
    assert_eq!(queued_rehearsal["writes_tape"], false);
    let retained_attestations = sqlx::query_scalar::<_, Value>(
        "SELECT evidence->'operator_attestations'
         FROM restore_rehearsal_events
         WHERE restore_rehearsal_id = $1 AND event_type = 'queued'",
    )
    .bind(cancelled_rehearsal_id)
    .fetch_one(&pool)
    .await
    .expect("attended assertions should be retained with the queued event");
    assert_eq!(retained_attestations["tape_loaded_confirmed"], true);
    assert_eq!(
        retained_attestations["isolated_destination_confirmed"],
        true
    );
    assert_eq!(retained_attestations["attended_confirmed"], true);
    assert_eq!(retained_attestations["assertion_only"], true);
    assert_eq!(retained_attestations["safety_authority"], false);

    let idempotent_rehearsal_response = api_request(
        &application,
        Method::POST,
        "/api/v1/restore-rehearsals".to_owned(),
        Some(queue_rehearsal_body.clone()),
    )
    .await;
    assert_eq!(idempotent_rehearsal_response.status(), StatusCode::ACCEPTED);
    let idempotent_rehearsal = response_json(idempotent_rehearsal_response).await;
    assert_eq!(
        idempotent_rehearsal["rehearsal"]["id"],
        json!(cancelled_rehearsal_id)
    );

    let cancel_rehearsal_response = api_request(
        &application,
        Method::POST,
        format!("/api/v1/restore-rehearsals/{cancelled_rehearsal_id}/cancel"),
        Some(json!({
            "confirmation": format!("CANCEL RESTORE REHEARSAL {cancelled_rehearsal_id}"),
            "actor": "integration recovery operator"
        })),
    )
    .await;
    assert_eq!(cancel_rehearsal_response.status(), StatusCode::OK);
    let cancelled_rehearsal = response_json(cancel_rehearsal_response).await;
    assert_eq!(cancelled_rehearsal["rehearsal"]["state"], "cancelled");
    assert_eq!(
        cancelled_rehearsal["cancel"],
        "completed_before_worker_claim"
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT reserved_by FROM cartridges WHERE id = $1",
        )
        .bind(scratch.id)
        .fetch_one(&pool)
        .await
        .expect("queued cancellation should release the cartridge"),
        None
    );

    let replay_rehearsal_response = api_request(
        &application,
        Method::POST,
        "/api/v1/restore-rehearsals".to_owned(),
        Some(queue_rehearsal_body),
    )
    .await;
    assert_eq!(replay_rehearsal_response.status(), StatusCode::ACCEPTED);
    let replay_rehearsal = response_json(replay_rehearsal_response).await;
    let rehearsal_id = uuid::Uuid::parse_str(
        replay_rehearsal["rehearsal"]["id"]
            .as_str()
            .expect("requeued restore rehearsal should have an id"),
    )
    .expect("requeued restore rehearsal id should parse");
    assert_ne!(rehearsal_id, cancelled_rehearsal_id);

    let rehearsal_claim = store::claim_next_restore_rehearsal_run(&pool, uuid::Uuid::now_v7())
        .await
        .expect("the worker should claim the exact restore plan, drive, and cartridge");
    let ClaimRestoreRehearsalRunResult::Claimed(rehearsal_claim) = rehearsal_claim else {
        panic!("the attended restore rehearsal should be claimable");
    };
    assert_eq!(rehearsal_claim.run.id, rehearsal_id);
    let complete_restore_scope = rehearsal_claim.run.restore_scope.clone();
    assert_eq!(complete_restore_scope["kind"], "complete_point");
    assert!(
        store::renew_restore_rehearsal_run_leases(&pool, &rehearsal_claim)
            .await
            .expect("both restore rehearsal leases should renew")
    );
    assert_eq!(
        store::running_restore_rehearsal_paths(&pool)
            .await
            .expect("running restore paths should query"),
        vec!["/dev/nst0".to_owned()]
    );
    assert!(store::append_restore_rehearsal_event(
        &pool,
        &rehearsal_claim,
        "media_observed",
        json!({"stable_id":"serial:IBM:ULTRIUM-HH9:DRIVE-001","ready":true}),
    )
    .await
    .expect("current drive observation should append"));
    assert!(store::transition_restore_rehearsal_phase(
        &pool,
        &rehearsal_claim,
        RestoreRehearsalPhase::Restoring,
        json!({"detail":"integration physical read started","media_mutation_allowed":false}),
    )
    .await
    .expect("rehearsal should enter restoring"));
    let claimed_restore_volumes = store::restore_rehearsal_volumes(&pool, rehearsal_id)
        .await
        .expect("the normalized single-volume restore ledger should load");
    assert_eq!(claimed_restore_volumes.len(), 1);
    assert_eq!(claimed_restore_volumes[0].state, "loaded");
    let ordered_restore_volumes = json!([{
        "volume_order": claimed_restore_volumes[0].volume_order,
        "cartridge_label": claimed_restore_volumes[0].expected_cartridge_label.clone(),
        "volume_label": claimed_restore_volumes[0].expected_volume_label.clone(),
        "engine_media_id": claimed_restore_volumes[0].expected_engine_media_id
    }]);
    assert!(store::append_restore_rehearsal_event(
        &pool,
        &rehearsal_claim,
        "engine_dispatched",
        json!({"engine_operation_id":"bacula-job:2002","restore_job_id":2002}),
    )
    .await
    .expect("restore dispatch evidence should append"));
    let rehearsal_destination =
        format!("/restore/planned-restores/integration-ready-recovery/{rehearsal_id}");
    assert!(store::append_restore_rehearsal_artifact(
        &pool,
        &rehearsal_claim,
        "engine_restore",
        "/var/lib/tapescale/bacula/restore-rehearsals/secret-engine.json",
        &"e".repeat(64),
        512,
        json!({
            "completed": true,
            "finalized": true,
            "read_only_media_operation": true,
            "physical_media_readback": true,
            "backup_job_id": 1001,
            "restore_job_id": 2002,
            "engine_media_id": 2001,
            "media_id": 2001,
            "volume": "INTEGRATION-SCRATCH-L9-000001",
            "volume_count": 1,
            "volumes": ordered_restore_volumes.clone(),
            "restore_scope": complete_restore_scope.clone(),
            "destination": rehearsal_destination.clone()
        }),
    )
    .await
    .expect("engine restore artifact should append"));
    assert!(store::append_restore_rehearsal_event(
        &pool,
        &rehearsal_claim,
        "engine_completed",
        json!({"restore_job_id":2002,"job_status":"T","job_errors":0}),
    )
    .await
    .expect("restore completion evidence should append"));
    assert!(
        store::complete_restore_rehearsal_volume(&pool, &rehearsal_claim, 1)
            .await
            .expect("the single ordered restore volume should complete")
    );
    assert!(store::transition_restore_rehearsal_phase(
        &pool,
        &rehearsal_claim,
        RestoreRehearsalPhase::Comparing,
        json!({"detail":"integration independently hashing restored content"}),
    )
    .await
    .expect("rehearsal should enter comparing"));
    assert!(store::append_restore_rehearsal_artifact(
        &pool,
        &rehearsal_claim,
        "restored_manifest",
        "/var/lib/tapescale/bacula/restore-rehearsals/secret-restored-manifest.json",
        &"f".repeat(64),
        1024,
        json!({
            "manifest_digest": source_manifest_digest.clone(),
            "entry_count": 3,
            "file_count": 3,
            "logical_bytes": 4096
        }),
    )
    .await
    .expect("restored manifest artifact should append"));
    assert!(store::append_restore_rehearsal_artifact(
        &pool,
        &rehearsal_claim,
        "comparison",
        "/var/lib/tapescale/bacula/restore-rehearsals/secret-comparison.json",
        &"1".repeat(64),
        384,
        json!({
            "matched": true,
            "restore_scope": complete_restore_scope.clone(),
            "expected_digest": source_manifest_digest.clone(),
            "actual_digest": source_manifest_digest.clone(),
            "differences": 0
        }),
    )
    .await
    .expect("manifest comparison artifact should append"));
    assert!(store::append_restore_rehearsal_artifact(
        &pool,
        &rehearsal_claim,
        "evidence_report",
        "/var/lib/tapescale/bacula/restore-rehearsals/secret-report.json",
        &"2".repeat(64),
        768,
        json!({
            "complete": true,
            "matched": true,
            "restore_scope": complete_restore_scope.clone(),
            "read_only_media_operation": true,
            "volume_count": 1,
            "volumes": ordered_restore_volumes.clone(),
            "source_manifest_digest": source_manifest_digest.clone(),
            "restored_manifest_digest": source_manifest_digest.clone(),
            "raw_restored_manifest_digest": source_manifest_digest.clone()
        }),
    )
    .await
    .expect("bounded rehearsal report should append"));

    let recovery_points_before_rehearsal =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM recovery_points")
            .fetch_one(&pool)
            .await
            .expect("recovery point count should query");
    let restore_tests_before_rehearsal = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM verification_records
         WHERE recovery_point_id = $1 AND kind = 'restore_test'",
    )
    .bind(recovery_point_id)
    .fetch_one(&pool)
    .await
    .expect("restore-test count should query");
    let rehearsal_result = json!({
        "scope": "integration attended G0.2 physical restore rehearsal",
        "production_eligible": false,
        "media_operation": "read_only",
        "source_backup_job_id": 1001,
        "engine_operation_id": "bacula-job:2002",
        "restore_job_id": 2002,
        "restore_job": {"job_id":2002,"job_status":"T","job_errors":0},
        "restore_catalog": {
            "source_backup_job_id": 1001,
            "job": {"job_id":2002}
        },
        "volume_count": 1,
        "volumes": ordered_restore_volumes,
        "restore_scope": complete_restore_scope,
        "retained_manifest_digest": source_manifest_digest.clone(),
        "source_manifest_digest": source_manifest_digest.clone(),
        "restored_manifest_digest": source_manifest_digest.clone(),
        "raw_restored_manifest_digest": source_manifest_digest.clone(),
        "restored_manifest_sha256": "f".repeat(64),
        "comparison": {
            "matched": true,
            "expected_digest": source_manifest_digest.clone(),
            "actual_digest": source_manifest_digest.clone(),
            "differences": []
        },
        "restore_destination": rehearsal_destination,
        "runtime": {
            "daemon_logs": {"director":"/var/lib/tapescale/bacula/private-director.log"},
            "director_config_path": "/var/lib/tapescale/bacula/private-director.conf"
        },
        "cleanup": {"postcondition":"ready_at_bot","position_uncertain":false},
        "media_mutation_commands_dispatched": false
    });
    let mut mismatched_rehearsal_result = rehearsal_result.clone();
    mismatched_rehearsal_result["restored_manifest_sha256"] = json!("0".repeat(64));
    assert!(matches!(
        store::finalize_restore_rehearsal_pass(
            &pool,
            &rehearsal_claim,
            mismatched_rehearsal_result,
        )
        .await
        .expect("mismatched artifact evidence should be a domain outcome"),
        FinalizeRestoreRehearsalResult::Rejected(_)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM verification_records
             WHERE recovery_point_id = $1 AND kind = 'restore_test'",
        )
        .bind(recovery_point_id)
        .fetch_one(&pool)
        .await
        .expect("rejected finalization must not append verification"),
        restore_tests_before_rehearsal
    );
    let finalized_rehearsal =
        store::finalize_restore_rehearsal_pass(&pool, &rehearsal_claim, rehearsal_result.clone())
            .await
            .expect("complete physical restore evidence should finalize atomically");
    let FinalizeRestoreRehearsalResult::Passed {
        verification_record_id,
    } = finalized_rehearsal
    else {
        panic!("complete matching restore rehearsal evidence should pass");
    };
    assert!(matches!(
        store::finalize_restore_rehearsal_pass(&pool, &rehearsal_claim, rehearsal_result)
            .await
            .expect("a terminal stale claim should be a domain outcome"),
        FinalizeRestoreRehearsalResult::FencedOut
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM recovery_points")
            .fetch_one(&pool)
            .await
            .expect("recovery point count should remain stable"),
        recovery_points_before_rehearsal
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM verification_records
             WHERE recovery_point_id = $1 AND kind = 'restore_test'",
        )
        .bind(recovery_point_id)
        .fetch_one(&pool)
        .await
        .expect("restore-test count should advance"),
        restore_tests_before_rehearsal + 1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT outcome FROM verification_records WHERE id = $1",)
            .bind(verification_record_id)
            .fetch_one(&pool)
            .await
            .expect("new restore verification should reload"),
        "passed"
    );
    assert!(store::running_restore_rehearsal_paths(&pool)
        .await
        .expect("passed rehearsal should release its path")
        .is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT reserved_by FROM cartridges WHERE id = $1",
        )
        .bind(scratch.id)
        .fetch_one(&pool)
        .await
        .expect("passed rehearsal should release the cartridge"),
        None
    );

    let rehearsal_detail_response = api_request(
        &application,
        Method::GET,
        format!("/api/v1/restore-rehearsals/{rehearsal_id}"),
        None,
    )
    .await;
    assert_eq!(rehearsal_detail_response.status(), StatusCode::OK);
    let rehearsal_detail = response_json(rehearsal_detail_response).await;
    assert_eq!(rehearsal_detail["rehearsal"]["state"], "passed");
    assert_eq!(rehearsal_detail["rehearsal"]["production_eligible"], false);
    assert_eq!(
        rehearsal_detail["artifacts"].as_array().map(Vec::len),
        Some(4)
    );
    let rehearsal_detail_text = serde_json::to_string(&rehearsal_detail)
        .expect("restore rehearsal detail should serialize");
    for private_value in [
        "storage_ref",
        "source_manifest_storage_ref",
        "source_mount_path",
        "drive_lease_id",
        "drive_fencing_token",
        "media_lease_id",
        "media_fencing_token",
        "daemon_logs",
        "director_config_path",
        "private-director.log",
    ] {
        assert!(
            !rehearsal_detail_text.contains(private_value),
            "public rehearsal detail leaked {private_value}"
        );
    }

    let rehearsal_evidence_response = api_request(
        &application,
        Method::GET,
        format!("/api/v1/restore-rehearsals/{rehearsal_id}/evidence"),
        None,
    )
    .await;
    assert_eq!(rehearsal_evidence_response.status(), StatusCode::OK);
    assert!(rehearsal_evidence_response
        .headers()
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("\"sha256-") && value.ends_with('"')));
    let rehearsal_evidence = to_bytes(rehearsal_evidence_response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("bounded restore rehearsal evidence should be readable");
    assert!(!rehearsal_evidence.contains(&b'\n'));
    let rehearsal_evidence_text =
        std::str::from_utf8(&rehearsal_evidence).expect("rehearsal evidence should be UTF-8");
    assert!(!rehearsal_evidence_text.contains("storage_ref"));
    assert!(!rehearsal_evidence_text.contains("private-director"));

    let rehearsal_artifact_mutation = sqlx::query(
        "UPDATE restore_rehearsal_artifacts SET metadata = '{}' WHERE restore_rehearsal_id = $1",
    )
    .bind(rehearsal_id)
    .execute(&pool)
    .await
    .expect_err("restore rehearsal artifacts must be append-only");
    assert_eq!(
        rehearsal_artifact_mutation
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("P0001")
    );
    let rehearsal_event_mutation =
        sqlx::query("DELETE FROM restore_rehearsal_events WHERE restore_rehearsal_id = $1")
            .bind(rehearsal_id)
            .execute(&pool)
            .await
            .expect_err("restore rehearsal events must be append-only");
    assert_eq!(
        rehearsal_event_mutation
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("P0001")
    );

    let movement_actions = store::list_required_actions(&pool)
        .await
        .expect("required-action feed should query");
    assert!(movement_actions
        .iter()
        .all(|action| action.id != offsite_id));
    assert_eq!(
        store::overview_counts(&pool)
            .await
            .expect("required-action summary should query")
            .required_actions,
        i64::try_from(movement_actions.len()).expect("fixture action count should fit in i64")
    );

    let data_events = store::single_tape_data_events(&pool, queued_data_run.id)
        .await
        .expect("data-run events should reload");
    assert_eq!(
        data_events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec![
            "queued",
            "started",
            "writing",
            "media_write_started",
            "media_prepared",
            "engine_dispatched",
            "written",
            "verifying",
            "restoring",
            "comparing",
            "passed"
        ]
    );
    let artifact_mutation = sqlx::query(
        "UPDATE single_tape_data_artifacts SET metadata = '{}' WHERE single_tape_data_run_id = $1",
    )
    .bind(queued_data_run.id)
    .execute(&pool)
    .await
    .expect_err("physical recovery artifacts must be append-only");
    assert_eq!(
        artifact_mutation
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("P0001")
    );
    let event_mutation =
        sqlx::query("DELETE FROM single_tape_data_events WHERE single_tape_data_run_id = $1")
            .bind(queued_data_run.id)
            .execute(&pool)
            .await
            .expect_err("physical recovery phase evidence must be append-only");
    assert_eq!(
        event_mutation
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("P0001")
    );
    let reservation_after_data =
        sqlx::query_scalar::<_, Option<String>>("SELECT reserved_by FROM cartridges WHERE id = $1")
            .bind(scratch.id)
            .fetch_one(&pool)
            .await
            .expect("passed data exercise should release its exact reservation");
    assert_eq!(reservation_after_data, None);
    let passed_cartridge_health =
        sqlx::query_scalar::<_, String>("SELECT health_status FROM cartridges WHERE id = $1")
            .bind(scratch.id)
            .fetch_one(&pool)
            .await
            .expect("passed cartridge health should reload");
    assert_eq!(passed_cartridge_health, "written_verified");

    let inventory = store::inventory_counts(&pool)
        .await
        .expect("inventory counts should succeed");
    assert_eq!(inventory.drives, 1);
    assert_eq!(inventory.accessible_drives, 1);
    assert_eq!(inventory.reconciled_drives, 1);
    assert_eq!(inventory.qualified_drives, 0);
    let reconcile_audit = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_events
         WHERE action = 'drive.reconcile' AND object_id = $1",
    )
    .bind(drive_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("drive audit query should succeed");
    assert_eq!(reconcile_audit, 2);

    // Exercise the complete Media Control Center contract against the same
    // cartridge, recovery point, drive, and custody history proved above.
    let device_location_response = api_request(
        &application,
        Method::POST,
        "/api/v1/locations".to_owned(),
        Some(json!({
            "name": "Integration tape drive bay",
            "kind": "device",
            "details": {"stable_drive_id":"serial:IBM:ULTRIUM-HH9:DRIVE-001"},
            "actor": "integration operator"
        })),
    )
    .await;
    assert_eq!(device_location_response.status(), StatusCode::CREATED);
    let device_location = response_json(device_location_response).await;
    let device_location_id = uuid::Uuid::parse_str(
        device_location["location"]["id"]
            .as_str()
            .expect("device location should have an id"),
    )
    .expect("device location id should parse");

    let load_movement_response = api_request(
        &application,
        Method::POST,
        "/api/v1/movements".to_owned(),
        Some(json!({
            "external_id": "INT-MOVE-DEVICE-001",
            "idempotency_key": "integration-create-device-load-001",
            "kind": "recall",
            "source_location_id": vault_id,
            "destination_location_id": device_location_id,
            "cartridge_ids": [scratch.id],
            "reason": "Load the selected cartridge for read-only reuse evaluation",
            "due_at": Utc::now() + chrono::TimeDelta::hours(1),
            "actor": "integration operator"
        })),
    )
    .await;
    assert_eq!(load_movement_response.status(), StatusCode::CREATED);
    let load_movement = response_json(load_movement_response).await;
    let load_movement_id = load_movement["movement"]["summary"]["id"]
        .as_str()
        .expect("device-load movement should have an id")
        .to_owned();
    let load_confirmation_response = api_request(
        &application,
        Method::POST,
        format!("/api/v1/movements/{load_movement_id}/confirm"),
        Some(json!({
            "action": "direct",
            "cartridge_ids": [scratch.id],
            "expected_confirmation_version": 0,
            "idempotency_key": "integration-confirm-device-load-001",
            "physically_confirmed": true,
            "note": "Barcode and drive bay were physically checked",
            "actor": "integration operator"
        })),
    )
    .await;
    assert_eq!(load_confirmation_response.status(), StatusCode::OK);

    observation.generation = Some(9);
    observation.position_lost = Some(false);
    observation.ready = Some(true);
    observation.medium_loaded = Some(true);
    observation.status = "ready".to_owned();
    observation.observed_at = Utc::now();
    store::record_drive_observation(&pool, &observation)
        .await
        .expect("reuse evaluation should have a current generation observation");
    assert!(matches!(
        store::reconcile_drive(&pool, drive_id, "serial:IBM:ULTRIUM-HH9:DRIVE-001")
            .await
            .expect("current drive identity should reconcile"),
        ReconcileDriveResult::Reconciled(_)
    ));
    let routine_pool_response = api_request(
        &application,
        Method::POST,
        "/api/v1/media-pools".to_owned(),
        Some(json!({
            "name": "Integration routine pool",
            "purpose": "routine",
            "details": {"retention_owner":"integration"},
            "actor": "integration administrator"
        })),
    )
    .await;
    assert_eq!(routine_pool_response.status(), StatusCode::CREATED);
    let routine_pool = response_json(routine_pool_response).await;
    let routine_pool_id = uuid::Uuid::parse_str(
        routine_pool["pool"]["id"]
            .as_str()
            .expect("routine pool should have an id"),
    )
    .expect("routine pool id should parse");

    let scratch_pool_response = api_request(
        &application,
        Method::POST,
        "/api/v1/media-pools".to_owned(),
        Some(json!({
            "name": "Integration scratch pool",
            "purpose": "scratch",
            "actor": "integration administrator"
        })),
    )
    .await;
    assert_eq!(scratch_pool_response.status(), StatusCode::CREATED);
    let scratch_pool = response_json(scratch_pool_response).await;
    let scratch_pool_id = uuid::Uuid::parse_str(
        scratch_pool["pool"]["id"]
            .as_str()
            .expect("scratch pool should have an id"),
    )
    .expect("scratch pool id should parse");

    let archive_pool_response = api_request(
        &application,
        Method::POST,
        "/api/v1/media-pools".to_owned(),
        Some(json!({
            "name": "Integration archive pool",
            "purpose": "archive",
            "indefinite_retention": false,
            "actor": "integration administrator"
        })),
    )
    .await;
    assert_eq!(archive_pool_response.status(), StatusCode::CREATED);
    let archive_pool = response_json(archive_pool_response).await;
    assert_eq!(archive_pool["pool"]["indefinite_retention"], true);
    let archive_pool_id = uuid::Uuid::parse_str(
        archive_pool["pool"]["id"]
            .as_str()
            .expect("archive pool should have an id"),
    )
    .expect("archive pool id should parse");

    let pools_response = api_request(
        &application,
        Method::GET,
        "/api/v1/media-pools".to_owned(),
        None,
    )
    .await;
    assert_eq!(pools_response.status(), StatusCode::OK);
    let pools = response_json(pools_response).await;
    assert_eq!(pools["pools"].as_array().map(Vec::len), Some(3));

    let inventory_response =
        api_request(&application, Method::GET, "/api/v1/media".to_owned(), None).await;
    assert_eq!(inventory_response.status(), StatusCode::OK);
    let inventory_response = response_json(inventory_response).await;
    let scratch_inventory = inventory_response["media"]
        .as_array()
        .expect("media inventory should be an array")
        .iter()
        .find(|media| media["id"] == json!(scratch.id))
        .expect("written cartridge should be in media inventory");
    assert_eq!(scratch_inventory["has_content"], true);
    assert_eq!(scratch_inventory["retained_recovery_points"], 1);
    assert_eq!(scratch_inventory["confirmed_location"]["kind"], "device");
    assert_eq!(
        scratch_inventory["location_name"],
        "Integration tape drive bay"
    );
    assert_eq!(scratch_inventory["overwrite_eligible"], false);
    assert!(scratch_inventory["overwrite_blockers"]
        .as_array()
        .is_some_and(|blockers| !blockers.is_empty()));
    assert!(scratch_inventory["latest_verification"].is_object());
    let initial_media_version = scratch_inventory["state_version"]
        .as_i64()
        .expect("inventory should expose the cartridge state version");

    let pool_assignment_body = json!({
        "pool_id": routine_pool_id,
        "expected_state_version": initial_media_version,
        "idempotency_key": "integration-media-pool-assign-001",
        "actor": "integration administrator"
    });
    let pool_assignment_response = api_request(
        &application,
        Method::POST,
        format!("/api/v1/media/{}/pool-assignment", scratch.id),
        Some(pool_assignment_body.clone()),
    )
    .await;
    assert_eq!(pool_assignment_response.status(), StatusCode::OK);
    let pool_assignment = response_json(pool_assignment_response).await;
    assert_eq!(pool_assignment["receipt"]["idempotent_replay"], false);
    assert_eq!(
        pool_assignment["media"]["cartridge"]["pool"]["id"],
        json!(routine_pool_id)
    );
    let mut media_version = pool_assignment["receipt"]["state_version"]
        .as_i64()
        .expect("pool assignment should return its resulting version");

    let pool_assignment_replay = api_request(
        &application,
        Method::POST,
        format!("/api/v1/media/{}/pool-assignment", scratch.id),
        Some(pool_assignment_body),
    )
    .await;
    assert_eq!(pool_assignment_replay.status(), StatusCode::OK);
    let pool_assignment_replay = response_json(pool_assignment_replay).await;
    assert_eq!(pool_assignment_replay["receipt"]["idempotent_replay"], true);
    assert_eq!(
        pool_assignment_replay["receipt"]["state_version"],
        media_version
    );

    let stale_assignment = api_request(
        &application,
        Method::POST,
        format!("/api/v1/media/{}/pool-assignment", scratch.id),
        Some(json!({
            "pool_id": archive_pool_id,
            "expected_state_version": initial_media_version,
            "idempotency_key": "integration-media-pool-stale-001",
            "actor": "integration administrator"
        })),
    )
    .await;
    assert_eq!(stale_assignment.status(), StatusCode::CONFLICT);
    let stale_assignment = response_json(stale_assignment).await;
    assert!(stale_assignment["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("stale cartridge state version")));

    let unsafe_scratch_assignment = api_request(
        &application,
        Method::POST,
        format!("/api/v1/media/{}/pool-assignment", scratch.id),
        Some(json!({
            "pool_id": scratch_pool_id,
            "expected_state_version": media_version,
            "idempotency_key": "integration-media-pool-protected-001",
            "actor": "integration administrator"
        })),
    )
    .await;
    assert_eq!(unsafe_scratch_assignment.status(), StatusCode::CONFLICT);
    let unsafe_scratch_assignment = response_json(unsafe_scratch_assignment).await;
    assert!(unsafe_scratch_assignment["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("protected_media_pool_assignment")));

    let mut created_hold_ids = Vec::new();
    for (index, kind) in ["manual", "legal", "archive"].iter().enumerate() {
        let body = json!({
            "kind": kind,
            "reason": format!("Integration {kind} protection review"),
            "expected_state_version": media_version,
            "idempotency_key": format!("integration-media-hold-create-{index}"),
            "actor": "integration compliance operator"
        });
        let create_hold_response = api_request(
            &application,
            Method::POST,
            format!("/api/v1/media/{}/holds", scratch.id),
            Some(body.clone()),
        )
        .await;
        assert_eq!(create_hold_response.status(), StatusCode::CREATED);
        let create_hold = response_json(create_hold_response).await;
        assert_eq!(create_hold["receipt"]["idempotent_replay"], false);
        let hold_id = uuid::Uuid::parse_str(
            create_hold["receipt"]["object_id"]
                .as_str()
                .expect("hold mutation should return the hold id"),
        )
        .expect("hold id should parse");
        created_hold_ids.push(hold_id);
        media_version = create_hold["receipt"]["state_version"]
            .as_i64()
            .expect("hold mutation should return a cartridge version");

        if index == 0 {
            let replay = api_request(
                &application,
                Method::POST,
                format!("/api/v1/media/{}/holds", scratch.id),
                Some(body),
            )
            .await;
            assert_eq!(replay.status(), StatusCode::CREATED);
            let replay = response_json(replay).await;
            assert_eq!(replay["receipt"]["idempotent_replay"], true);
            assert_eq!(replay["receipt"]["object_id"], json!(hold_id));
        }
    }

    let descendant_job_id = uuid::Uuid::now_v7();
    let descendant_run_id = uuid::Uuid::now_v7();
    let descendant_recovery_point_id = uuid::Uuid::now_v7();
    let descendant_hold_id = uuid::Uuid::now_v7();
    let descendant_correlation_id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO jobs (id, external_id, kind, priority)
         VALUES ($1, 'integration-dependent-recovery-001', 'backup', 100)",
    )
    .bind(descendant_job_id)
    .execute(&pool)
    .await
    .expect("dependent recovery fixture job should persist");
    sqlx::query(
        "INSERT INTO job_runs (
             id, job_id, scheduled_occurrence, attempt, state, correlation_id,
             source_snapshot_id, started_at, finished_at
         ) VALUES (
             $1, $2, 'integration-dependent-recovery-001', 1, 'verified', $3,
             $4, now() - interval '2 minutes', now() - interval '1 minute'
         )",
    )
    .bind(descendant_run_id)
    .bind(descendant_job_id)
    .bind(descendant_correlation_id)
    .bind(created_profile.source_snapshot_id)
    .execute(&pool)
    .await
    .expect("dependent recovery fixture run should persist");
    sqlx::query(
        "INSERT INTO recovery_points (
             id, source_id, run_id, representation, level, source_scope_hash,
             format_version, key_lineage_id, status, recovery_time,
             expires_at, indefinite, coverage, manifest_digest, created_at
         )
         SELECT $1, source_id, $2, representation, 'incremental', source_scope_hash,
                format_version, key_lineage_id, 'verified', now(),
                NULL, true, coverage, $3, now()
         FROM recovery_points WHERE id = $4",
    )
    .bind(descendant_recovery_point_id)
    .bind(descendant_run_id)
    .bind("d".repeat(64))
    .bind(recovery_point_id)
    .execute(&pool)
    .await
    .expect("verified indefinite descendant recovery point should persist");
    sqlx::query(
        "INSERT INTO recovery_dependencies (
             recovery_point_id, depends_on_id, dependency_kind
         ) VALUES ($1, $2, 'incremental_base')",
    )
    .bind(descendant_recovery_point_id)
    .bind(recovery_point_id)
    .execute(&pool)
    .await
    .expect("descendant dependency edge should persist");
    sqlx::query(
        "INSERT INTO holds (
             id, recovery_point_id, kind, reason,
             created_by_label, created_correlation_id
         ) VALUES (
             $1, $2, 'archive', 'Preserve dependent recovery chain for integration proof',
             'integration compliance operator', $3
         )",
    )
    .bind(descendant_hold_id)
    .bind(descendant_recovery_point_id)
    .bind(descendant_correlation_id)
    .execute(&pool)
    .await
    .expect("descendant archive hold should persist");

    let dependent_inventory_response =
        api_request(&application, Method::GET, "/api/v1/media".to_owned(), None).await;
    assert_eq!(dependent_inventory_response.status(), StatusCode::OK);
    let dependent_inventory = response_json(dependent_inventory_response).await;
    let dependent_cartridge = dependent_inventory["media"]
        .as_array()
        .expect("dependent inventory should be an array")
        .iter()
        .find(|media| media["id"] == json!(scratch.id))
        .expect("dependency-protected cartridge should be in inventory");
    assert_eq!(dependent_cartridge["dependent_recovery_points"], 1);
    assert_eq!(dependent_cartridge["indefinite_archive_claims"], 1);
    assert_eq!(dependent_cartridge["active_holds"], 4);
    assert_eq!(dependent_cartridge["reuse_state"], "protected");

    let dependent_dossier_response = api_request(
        &application,
        Method::GET,
        format!("/api/v1/media/{}", scratch.id),
        None,
    )
    .await;
    assert_eq!(dependent_dossier_response.status(), StatusCode::OK);
    let dependent_dossier = response_json(dependent_dossier_response).await;
    assert!(dependent_dossier["media"]["dependent_recovery_points"]
        .as_array()
        .is_some_and(|points| points.iter().any(|point| {
            point["id"] == json!(descendant_recovery_point_id)
                && point["depth"] == 1
                && point["indefinite"] == true
        })));
    assert!(dependent_dossier["media"]["holds"]
        .as_array()
        .is_some_and(|holds| holds.iter().any(|hold| {
            hold["id"] == json!(descendant_hold_id)
                && hold["recovery_point_id"] == json!(descendant_recovery_point_id)
                && hold["active"] == true
        })));

    let protected_reuse_response = api_request(
        &application,
        Method::GET,
        format!(
            "/api/v1/media/{}/reuse-evaluation?drive_id={drive_id}",
            scratch.id
        ),
        None,
    )
    .await;
    assert_eq!(protected_reuse_response.status(), StatusCode::OK);
    assert_eq!(
        protected_reuse_response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let protected_reuse = response_json(protected_reuse_response).await;
    assert_eq!(protected_reuse["read_only"], true);
    assert_eq!(protected_reuse["evaluation"]["allowed"], false);
    assert_eq!(
        protected_reuse["evaluation"]["destructive_operation_available"],
        false
    );
    let protected_blockers = protected_reuse["evaluation"]["blockers"]
        .as_array()
        .expect("reuse blockers should be an array")
        .iter()
        .filter_map(|blocker| blocker["code"].as_str())
        .collect::<Vec<_>>();
    assert!(protected_blockers.contains(&"retained_recovery_points"));
    assert!(protected_blockers.contains(&"dependent_recovery_points"));
    assert!(protected_blockers.contains(&"explicit_holds"));
    assert!(protected_blockers.contains(&"indefinite_archive"));
    assert_eq!(
        protected_reuse["evaluation"]["selected_drive"]["exact_qualification_current"],
        true
    );
    assert!(protected_reuse["evaluation"]["affected_recovery_points"]
        .as_array()
        .is_some_and(|points| points
            .iter()
            .any(|point| point["id"] == json!(recovery_point_id))));
    assert!(protected_reuse["evaluation"]["affected_recovery_points"]
        .as_array()
        .is_some_and(|points| points.iter().any(|point| {
            point["id"] == json!(descendant_recovery_point_id)
                && point["relation"] == "dependent"
                && point["dependency_depth"] == 1
        })));

    for (index, hold_id) in created_hold_ids.iter().enumerate() {
        let body = json!({
            "reason": format!("Integration {index} review completed"),
            "expected_state_version": media_version,
            "idempotency_key": format!("integration-media-hold-release-{index}"),
            "actor": "integration compliance operator"
        });
        let release_response = api_request(
            &application,
            Method::POST,
            format!("/api/v1/holds/{hold_id}/release"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(release_response.status(), StatusCode::OK);
        let release = response_json(release_response).await;
        assert_eq!(release["receipt"]["idempotent_replay"], false);
        media_version = release["receipt"]["state_version"]
            .as_i64()
            .expect("hold release should return a cartridge version");

        if index == 0 {
            let replay = api_request(
                &application,
                Method::POST,
                format!("/api/v1/holds/{hold_id}/release"),
                Some(body),
            )
            .await;
            assert_eq!(replay.status(), StatusCode::OK);
            let replay = response_json(replay).await;
            assert_eq!(replay["receipt"]["idempotent_replay"], true);
            assert_eq!(replay["receipt"]["state_version"], media_version);
        }
    }

    sqlx::query(
        "UPDATE holds
         SET released_at = now(),
             released_by_label = 'integration compliance operator',
             release_reason = 'Dependent archive proof completed',
             release_correlation_id = $2
         WHERE id = $1",
    )
    .bind(descendant_hold_id)
    .bind(uuid::Uuid::now_v7())
    .execute(&pool)
    .await
    .expect("descendant hold should retain a valid release receipt");
    sqlx::query(
        "UPDATE recovery_points
         SET expires_at = now() - interval '1 day', indefinite = false
         WHERE id = $1",
    )
    .bind(descendant_recovery_point_id)
    .execute(&pool)
    .await
    .expect("descendant recovery protection should expire");

    let released_dependency_inventory_response =
        api_request(&application, Method::GET, "/api/v1/media".to_owned(), None).await;
    assert_eq!(
        released_dependency_inventory_response.status(),
        StatusCode::OK
    );
    let released_dependency_inventory = response_json(released_dependency_inventory_response).await;
    let released_dependency_cartridge = released_dependency_inventory["media"]
        .as_array()
        .expect("released dependency inventory should be an array")
        .iter()
        .find(|media| media["id"] == json!(scratch.id))
        .expect("released dependency cartridge should remain in inventory");
    assert_eq!(
        released_dependency_cartridge["dependent_recovery_points"],
        0
    );
    assert_eq!(
        released_dependency_cartridge["indefinite_archive_claims"],
        0
    );
    assert_eq!(released_dependency_cartridge["active_holds"], 0);

    let dossier_response = api_request(
        &application,
        Method::GET,
        format!("/api/v1/media/{}", scratch.id),
        None,
    )
    .await;
    assert_eq!(dossier_response.status(), StatusCode::OK);
    let dossier = response_json(dossier_response).await;
    assert_eq!(dossier["media"]["cartridge"]["active_holds"], 0);
    assert!(dossier["media"]["holds"]
        .as_array()
        .is_some_and(|holds| holds.len() >= 4 && holds.iter().all(|hold| hold["active"] == false)));
    let dossier_hold_kinds = dossier["media"]["holds"]
        .as_array()
        .expect("media dossier holds should be an array")
        .iter()
        .filter_map(|hold| hold["kind"].as_str())
        .collect::<Vec<_>>();
    assert!(dossier_hold_kinds.contains(&"manual"));
    assert!(dossier_hold_kinds.contains(&"legal"));
    assert!(dossier_hold_kinds.contains(&"archive"));
    assert!(dossier["media"]["volumes"]
        .as_array()
        .is_some_and(|volumes| !volumes.is_empty()));
    assert!(dossier["media"]["custody_events"]
        .as_array()
        .is_some_and(|events| !events.is_empty()));
    assert!(dossier["media"]["operations"]
        .as_array()
        .is_some_and(|operations| operations.len() >= 7));
    assert!(
        !serde_json::to_string(&dossier)
            .expect("media dossier should serialize")
            .contains("storage_ref"),
        "public media detail must never expose artifact storage paths"
    );

    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audit_events
             WHERE object_type = 'cartridge' AND object_id = $1
               AND action IN ('media.pool_assign', 'media.hold_create', 'media.hold_release')",
        )
        .bind(scratch.id.to_string())
        .fetch_one(&pool)
        .await
        .expect("media-control audit records should query"),
        7
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notification_outbox
             WHERE payload ->> 'cartridge_id' = $1
               AND payload ->> 'event' IN (
                   'media_pool_assigned', 'media_hold_created', 'media_hold_released'
               )",
        )
        .bind(scratch.id.to_string())
        .fetch_one(&pool)
        .await
        .expect("media-control outbox records should query"),
        7
    );
    let immutable_media_operation =
        sqlx::query("UPDATE media_operations SET result = '{}' WHERE cartridge_id = $1")
            .bind(scratch.id)
            .execute(&pool)
            .await
            .expect_err("media mutation receipts must be append-only");
    assert_eq!(
        immutable_media_operation
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("P0001")
    );

    sqlx::query(
        "UPDATE recovery_points
         SET expires_at = now() - interval '1 day', indefinite = false
         WHERE id = $1",
    )
    .bind(recovery_point_id)
    .execute(&pool)
    .await
    .expect("expired recovery-point fixture should update");

    let eligible_reuse_response = api_request(
        &application,
        Method::GET,
        format!(
            "/api/v1/media/{}/reuse-evaluation?drive_id={drive_id}",
            scratch.id
        ),
        None,
    )
    .await;
    assert_eq!(eligible_reuse_response.status(), StatusCode::OK);
    let eligible_reuse = response_json(eligible_reuse_response).await;
    assert_eq!(eligible_reuse["evaluation"]["allowed"], true);
    assert_eq!(eligible_reuse["evaluation"]["blockers"], json!([]));
    assert_eq!(
        eligible_reuse["evaluation"]["destructive_operation_available"],
        false
    );
    assert_eq!(eligible_reuse["evaluation"]["state_version"], media_version);

    sqlx::query("UPDATE drives SET generation = 8 WHERE id = $1")
        .bind(drive_id)
        .execute(&pool)
        .await
        .expect("incompatible-drive fixture should update");
    let incompatible_reuse_response = api_request(
        &application,
        Method::GET,
        format!(
            "/api/v1/media/{}/reuse-evaluation?drive_id={drive_id}",
            scratch.id
        ),
        None,
    )
    .await;
    assert_eq!(incompatible_reuse_response.status(), StatusCode::OK);
    let incompatible_reuse = response_json(incompatible_reuse_response).await;
    assert_eq!(incompatible_reuse["evaluation"]["allowed"], false);
    assert!(incompatible_reuse["evaluation"]["blockers"]
        .as_array()
        .is_some_and(|blockers| blockers
            .iter()
            .any(|blocker| { blocker["code"] == "drive_cannot_write_media_format" })));
    sqlx::query("UPDATE drives SET generation = 9 WHERE id = $1")
        .bind(drive_id)
        .execute(&pool)
        .await
        .expect("drive generation fixture should restore");

    let media_actions = store::list_required_actions(&pool)
        .await
        .expect("media required actions should query");
    assert!(media_actions.iter().any(|action| {
        action.id == format!("media:{}", created.id)
            && action.route == format!("tapes/{}", created.id)
    }));
    assert_eq!(
        store::overview_counts(&pool)
            .await
            .expect("media required-action summary should query")
            .required_actions,
        i64::try_from(media_actions.len()).expect("fixture action count should fit in i64")
    );

    let quarantine_media = store::create_media(
        &pool,
        &NewMedia {
            internal_label: "INTEGRATION-QUARANTINE-L9-000002".to_owned(),
            barcode: Some("INT9000002".to_owned()),
            media_generation: 9,
            media_format: "LTO-9".to_owned(),
            worm: false,
        },
    )
    .await
    .expect("quarantine fixture cartridge should persist");
    let qualification = store::queue_qualification_run(
        &pool,
        drive_id,
        "serial:IBM:ULTRIUM-HH9:DRIVE-001",
        quarantine_media.id,
        &quarantine_media.internal_label,
        true,
        true,
    )
    .await
    .expect("quarantine fixture qualification should queue");
    assert!(matches!(
        qualification,
        QueueQualificationRunResult::Queued(_)
    ));
    let quarantine_qualification = store::claim_next_qualification_run(&pool, uuid::Uuid::now_v7())
        .await
        .expect("quarantine fixture qualification should claim");
    let ClaimQualificationRunResult::Claimed(quarantine_qualification) = quarantine_qualification
    else {
        panic!("quarantine fixture qualification should be claimed");
    };
    assert!(store::finish_qualification_run(
        &pool,
        &quarantine_qualification,
        QualificationTerminalState::Passed,
        json!({"detail":"qualified quarantine fixture","position_uncertain":false}),
    )
    .await
    .expect("quarantine fixture qualification should finish"));

    let fenced_run = store::queue_single_tape_data_run(
        &pool,
        created_profile.id,
        drive_id,
        "serial:IBM:ULTRIUM-HH9:DRIVE-001",
        quarantine_media.id,
        &quarantine_media.internal_label,
        true,
        true,
        true,
    )
    .await
    .expect("qualified quarantine fixture should queue");
    assert!(matches!(
        fenced_run,
        QueueSingleTapeDataRunResult::Queued(_)
    ));
    let fenced_claim = store::claim_next_single_tape_data_run(&pool, uuid::Uuid::now_v7())
        .await
        .expect("quarantine fixture should claim");
    let ClaimSingleTapeDataRunResult::Claimed(fenced_claim) = fenced_claim else {
        panic!("quarantine fixture should be claimed");
    };
    assert!(store::transition_single_tape_data_phase(
        &pool,
        &fenced_claim,
        SingleTapeDataPhase::Writing,
        json!({"operation":"full"}),
    )
    .await
    .expect("quarantine fixture should enter writing"));
    assert!(store::mark_single_tape_media_write_started(
        &pool,
        &fenced_claim,
        "INTEGRATION-QUARANTINE-L9-000002",
    )
    .await
    .expect("quarantine fixture should commit the media boundary"));
    sqlx::query(
        "UPDATE resource_leases SET expires_at = now() - interval '1 second'
         WHERE id = ANY($1::uuid[])",
    )
    .bind(vec![
        fenced_claim.drive_lease_id,
        fenced_claim.media_lease_id,
    ])
    .execute(&pool)
    .await
    .expect("both recovery-exercise leases should expire");
    assert!(store::quarantine_fenced_single_tape_write(
        &pool,
        &fenced_claim,
        "worker lost its fencing lease after label dispatch",
    )
    .await
    .expect("the stale worker should quarantine without releasing a newer owner"));
    assert_eq!(
        store::recover_expired_single_tape_data_runs(&pool)
            .await
            .expect("expired media write should recover"),
        1
    );
    let (quarantined_health, interrupted_state, interrupted_marker) =
        sqlx::query_as::<_, (String, String, bool)>(
            "SELECT cartridge.health_status, data_run.state,
                    (data_run.result ->> 'media_write_started')::boolean
             FROM single_tape_data_runs data_run
             JOIN cartridges cartridge ON cartridge.id = data_run.cartridge_id
             WHERE data_run.id = $1",
        )
        .bind(fenced_claim.run.id)
        .fetch_one(&pool)
        .await
        .expect("quarantined run and cartridge should reload");
    assert_eq!(quarantined_health, "write_unverified");
    assert_eq!(interrupted_state, "interrupted");
    assert!(interrupted_marker);
    let fenced_events = store::single_tape_data_events(&pool, fenced_claim.run.id)
        .await
        .expect("fenced events should reload");
    assert!(fenced_events
        .iter()
        .any(|event| event.event_type == "fenced_out"));

    observation.observed_at = Utc::now();
    store::record_drive_observation(&pool, &observation)
        .await
        .expect("fresh observation should clear the stale motion evidence");
    assert!(matches!(
        store::reconcile_drive(&pool, drive_id, "serial:IBM:ULTRIUM-HH9:DRIVE-001")
            .await
            .expect("drive should reconcile after a fresh safe observation"),
        ReconcileDriveResult::Reconciled(_)
    ));
    assert!(matches!(
        store::queue_single_tape_data_run(
            &pool,
            created_profile.id,
            drive_id,
            "serial:IBM:ULTRIUM-HH9:DRIVE-001",
            quarantine_media.id,
            &quarantine_media.internal_label,
            true,
            true,
            true,
        )
        .await
        .expect("quarantined media should be a domain outcome"),
        QueueSingleTapeDataRunResult::Blocked(_)
    ));

    let requalification = store::queue_qualification_run(
        &pool,
        drive_id,
        "serial:IBM:ULTRIUM-HH9:DRIVE-001",
        quarantine_media.id,
        &quarantine_media.internal_label,
        true,
        true,
    )
    .await
    .expect("write-unverified scratch media should permit destructive requalification");
    assert!(matches!(
        requalification,
        QueueQualificationRunResult::Queued(_)
    ));
    let requalification = store::claim_next_qualification_run(&pool, uuid::Uuid::now_v7())
        .await
        .expect("requalification should claim");
    let ClaimQualificationRunResult::Claimed(requalification) = requalification else {
        panic!("requalification should be claimed");
    };
    assert!(store::finish_qualification_run(
        &pool,
        &requalification,
        QualificationTerminalState::Passed,
        json!({"detail":"fresh destructive qualification passed","position_uncertain":false}),
    )
    .await
    .expect("fresh qualification should clear quarantine"));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT health_status FROM cartridges WHERE id = $1")
            .bind(quarantine_media.id)
            .fetch_one(&pool)
            .await
            .expect("requalified cartridge should reload"),
        "qualified_scratch"
    );

    let failed_newer_qualification = store::queue_qualification_run(
        &pool,
        drive_id,
        "serial:IBM:ULTRIUM-HH9:DRIVE-001",
        quarantine_media.id,
        &quarantine_media.internal_label,
        true,
        true,
    )
    .await
    .expect("newer qualification attempt should queue");
    assert!(matches!(
        failed_newer_qualification,
        QueueQualificationRunResult::Queued(_)
    ));
    let failed_newer_qualification =
        store::claim_next_qualification_run(&pool, uuid::Uuid::now_v7())
            .await
            .expect("newer qualification attempt should claim");
    let ClaimQualificationRunResult::Claimed(failed_newer_qualification) =
        failed_newer_qualification
    else {
        panic!("newer qualification attempt should be claimed");
    };
    assert!(store::finish_qualification_run(
        &pool,
        &failed_newer_qualification,
        QualificationTerminalState::Failed,
        json!({"detail":"newer physical attempt failed","position_uncertain":false}),
    )
    .await
    .expect("newer failed qualification should persist"));
    assert!(matches!(
        store::queue_single_tape_data_run(
            &pool,
            created_profile.id,
            drive_id,
            "serial:IBM:ULTRIUM-HH9:DRIVE-001",
            quarantine_media.id,
            &quarantine_media.internal_label,
            true,
            true,
            true,
        )
        .await
        .expect("the latest exact failed qualification should govern"),
        QueueSingleTapeDataRunResult::Blocked(_)
    ));

    let queued_for_expiry = store::queue_drive_operation(
        &pool,
        drive_id,
        "serial:IBM:ULTRIUM-HH9:DRIVE-001",
        SasMotionOperation::Unload,
        true,
    )
    .await
    .expect("second motion request should persist");
    assert!(matches!(
        queued_for_expiry,
        QueueDriveOperationResult::Queued(_)
    ));
    let claimed_for_expiry = store::claim_next_drive_operation(&pool, uuid::Uuid::now_v7())
        .await
        .expect("second worker claim should succeed");
    let ClaimDriveOperationResult::Claimed(claimed_for_expiry) = claimed_for_expiry else {
        panic!("second motion should be claimed");
    };
    sqlx::query(
        "UPDATE resource_leases SET expires_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(claimed_for_expiry.lease_id)
    .execute(&pool)
    .await
    .expect("lease expiry fixture should update");
    assert!(!store::finish_drive_operation(
        &pool,
        &claimed_for_expiry,
        DriveOperationTerminalState::Succeeded,
        json!({"detail":"late success must not commit"})
    )
    .await
    .expect("expired fencing check should be a domain outcome"));
    assert_eq!(
        store::recover_expired_drive_operations(&pool)
            .await
            .expect("expired motion should recover safely"),
        1
    );
    let expired = store::list_drive_operations(&pool)
        .await
        .expect("expired motion should reload")
        .into_iter()
        .find(|operation| operation.id == claimed_for_expiry.operation.id)
        .expect("expired motion remains in history");
    assert_eq!(expired.state, "blocked");
    assert_eq!(expired.result["position_uncertain"], json!(true));
    let uncertain_drive = store::list_drives(&pool)
        .await
        .expect("uncertain drive should reload")
        .pop()
        .expect("drive remains present");
    assert!(!uncertain_drive.reconciled);
    assert!(matches!(
        store::reconcile_drive(&pool, drive_id, "serial:IBM:ULTRIUM-HH9:DRIVE-001")
            .await
            .expect("stale post-expiry state should be a domain outcome"),
        ReconcileDriveResult::Blocked
    ));

    observation.observed_at = Utc::now();
    store::record_drive_observation(&pool, &observation)
        .await
        .expect("post-expiry observation should persist");
    let recovered_identity =
        store::reconcile_drive(&pool, drive_id, "serial:IBM:ULTRIUM-HH9:DRIVE-001")
            .await
            .expect("post-expiry identity can be rechecked");
    assert!(matches!(
        recovered_identity,
        ReconcileDriveResult::Reconciled(_)
    ));

    observation.position_lost = Some(true);
    observation.status = "position_lost".to_owned();
    observation.observed_at = Utc::now();
    let reset_position = store::record_drive_observation(&pool, &observation)
        .await
        .expect("position loss should persist");
    assert!(reset_position.reconciled);
    assert!(matches!(
        store::reconcile_drive(&pool, drive_id, "serial:IBM:ULTRIUM-HH9:DRIVE-001")
            .await
            .expect("identity trust is independent of current tape position"),
        ReconcileDriveResult::Reconciled(_)
    ));
    let reset_inventory = store::inventory_counts(&pool)
        .await
        .expect("reset-position inventory counts should succeed");
    assert_eq!(reset_inventory.accessible_drives, 1);
    assert_eq!(reset_inventory.reconciled_drives, 1);

    let recovery_motion = store::queue_drive_operation(
        &pool,
        drive_id,
        "serial:IBM:ULTRIUM-HH9:DRIVE-001",
        SasMotionOperation::Rewind,
        true,
    )
    .await
    .expect("rewind should remain available to recover from unknown position");
    assert!(matches!(
        recovery_motion,
        QueueDriveOperationResult::Queued(_)
    ));
    assert_eq!(
        store::block_queued_drive_operations(&pool, "integration cleanup")
            .await
            .expect("queued recovery motion should be cleanly terminalized"),
        1
    );

    observation.position_lost = Some(false);
    observation.status = "no_media".to_owned();
    observation.ready = Some(false);
    observation.medium_loaded = Some(false);
    observation.at_bot = None;
    observation.observed_at = Utc::now();
    store::record_drive_observation(&pool, &observation)
        .await
        .expect("known position should persist");

    sqlx::query("UPDATE drives SET qualification_status = 'physically_qualified' WHERE id = $1")
        .bind(drive_id)
        .execute(&pool)
        .await
        .expect("qualification setup should succeed");
    observation.firmware = Some("A1B3".to_owned());
    observation.observed_at = Utc::now();
    let changed = store::record_drive_observation(&pool, &observation)
        .await
        .expect("firmware change should persist");
    assert!(!changed.reconciled);
    assert_eq!(changed.qualification_status, "detected_unqualified");

    let missing = MissingSasDrive {
        observed_path: "/dev/nst0".to_owned(),
        reason: "device node disappeared".to_owned(),
        observed_at: Utc::now(),
    };
    assert_eq!(
        store::mark_drive_missing(&pool, &missing)
            .await
            .expect("missing state should persist"),
        1
    );
    let missing_record = store::list_drives(&pool)
        .await
        .expect("drive should reload")
        .pop()
        .expect("drive remains as history");
    assert!(!missing_record.reconciled);
    assert_eq!(missing_record.health["present"], json!(false));
}
