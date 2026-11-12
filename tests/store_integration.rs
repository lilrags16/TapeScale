use std::time::Duration;

use chrono::Utc;
use serde_json::json;
use tapescale::{
    device::{MissingSasDrive, SasDriveObservation, SasMotionOperation},
    store::{
        self, ClaimDriveOperationResult, ClaimQualificationRunResult, DriveOperationTerminalState,
        NewMedia, QualificationTerminalState, QueueDriveOperationResult,
        QueueQualificationRunResult, ReconcileDriveResult,
    },
};

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
