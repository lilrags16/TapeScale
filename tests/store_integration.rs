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
        self, ClaimDriveOperationResult, ClaimQualificationRunResult, ClaimSingleTapeDataRunResult,
        CreateSingleTapeDataProfileResult, DriveOperationTerminalState,
        FinalizeSingleTapeDataResult, NewMedia, NewSingleTapeDataProfile,
        QualificationTerminalState, QueueDriveOperationResult, QueueQualificationRunResult,
        QueueSingleTapeDataRunResult, ReconcileDriveResult, SingleTapeDataPhase,
        SingleTapePassEvidence,
    },
};
use tower::ServiceExt;

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
    let queued_qualification = store::queue_qualification_run(
        &pool,
        drive_id,
        "serial:IBM:ULTRIUM-HH9:DRIVE-001",
        scratch.id,
        &scratch.internal_label,
        true,
        true,
    )
    .await
    .expect("qualification request should persist");
    let QueueQualificationRunResult::Queued(queued_qualification) = queued_qualification else {
        panic!("eligible qualification request should queue");
    };
    assert_eq!(queued_qualification.state, "queued");
    assert_eq!(queued_qualification.engine_version, "15.0.3");
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
    assert!(store::append_single_tape_data_artifact(
        &pool,
        &data_claim,
        "source_manifest",
        "artifacts/source-manifest.json",
        &"a".repeat(64),
        1024,
        json!({"manifest_digest":"fixture-manifest-v1","file_count":3}),
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
        json!({"finalized":true,"engine":"Bacula Community","version":"15.0.3"}),
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
        engine_operation_id: "bacula-job-1001".to_owned(),
        engine_volume_name: "INTEGRATION-SCRATCH-L9-000001".to_owned(),
        engine_record_id: "bacula-media-record-1001".to_owned(),
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
        json!({"manifest_digest":"fixture-manifest-v1","file_count":3}),
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
    assert_eq!(engine_operation_id.as_deref(), Some("bacula-job-1001"));

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
    assert_eq!(reconcile_audit, 1);

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
