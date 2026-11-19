use std::{fs, path::PathBuf, time::Duration};

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, PgPool};
use tapescale::{
    api::{self, AppState},
    backup_ops::{self, CancelBackupRequestResult, CreateBackupRequestResult, NewBackupProfile},
    data_path::{build_manifest, persist_manifest},
    device::SasDriveObservation,
    store::{
        self, ClaimQualificationRunResult, CreateSingleTapeDataProfileResult, NewMedia,
        NewSingleTapeDataProfile, QualificationTerminalState, QueueQualificationRunResult,
        QueueSingleTapeDataRunResult, ReconcileDriveResult,
    },
};
use tower::ServiceExt;
use url::Url;
use uuid::Uuid;

async fn api_request(
    application: &Router,
    method: Method,
    uri: String,
    body: Value,
) -> axum::response::Response {
    application
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&body).expect("test request should serialize"),
                ))
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

async fn isolated_database() -> Option<(PgPool, PgPool, String)> {
    let Ok(database_url) = std::env::var("TAPESCALE_TEST_DATABASE_URL") else {
        eprintln!("TAPESCALE_TEST_DATABASE_URL is not set; skipping PostgreSQL integration test");
        return None;
    };

    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("test database should accept an administrative connection");
    let schema = format!("backup_workflow_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("isolated integration schema should be created");

    let mut isolated_url = Url::parse(&database_url).expect("test database URL should parse");
    isolated_url
        .query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema},public"));
    let pool = store::connect(isolated_url.as_str(), Duration::from_secs(10))
        .await
        .expect("isolated database should connect and migrate");
    Some((pool, admin, schema))
}

async fn create_source_profile(
    pool: &PgPool,
    name: &str,
    source_subpath: &str,
) -> store::SingleTapeDataProfileRecord {
    let profile = NewSingleTapeDataProfile {
        name: name.to_owned(),
        source_subpath: source_subpath.to_owned(),
        restore_subpath: format!("backup-integration/{source_subpath}"),
        source_mount_path: format!("/source/{source_subpath}"),
        restore_root_path: format!("/restore/backup-integration/{source_subpath}"),
        immutable_snapshot_confirmed: true,
    };
    let created = store::create_single_tape_data_profile(pool, &profile)
        .await
        .expect("attested source profile should persist");
    let CreateSingleTapeDataProfileResult::Created(created) = created else {
        panic!("attested source profile should be accepted");
    };
    *created
}

async fn create_backup_profile(
    pool: &PgPool,
    name: &str,
    source_subpath: &str,
    scheduled: bool,
) -> backup_ops::BackupProfileRecord {
    let source = create_source_profile(pool, name, source_subpath).await;
    let mut request = NewBackupProfile::attended_full_file(source.id);
    request.schedule_enabled = scheduled;
    request.schedule_timezone = "America/New_York".to_owned();
    request.actor_label = "Backup integration operator".to_owned();
    let profile = backup_ops::create_backup_profile(pool, &request)
        .await
        .expect("full-file backup profile should persist");
    assert_eq!(profile.engine_name, "TapeScale Native");
    assert_eq!(profile.engine_version, "2");
    assert_eq!(profile.adapter_version, "native-v2");
    assert_eq!(profile.format_version, "tapescale-native-v2");
    profile
}

fn loaded_drive_observation() -> SasDriveObservation {
    SasDriveObservation {
        stable_id: "serial:IBM:ULTRIUM-HH9:BACKUP-INTEGRATION".to_owned(),
        identity_verified: true,
        identity_conflict: false,
        identity_basis: "vpd_80_serial".to_owned(),
        vendor: Some("IBM".to_owned()),
        model: Some("ULTRIUM-HH9".to_owned()),
        serial: Some("BACKUP-INTEGRATION".to_owned()),
        wwn: None,
        firmware: Some("B100".to_owned()),
        generation: Some(9),
        observed_path: "/dev/nst9".to_owned(),
        device_major: 9,
        device_minor: 200,
        scsi_address: Some("9:0:0:0".to_owned()),
        generic_path: None,
        sysfs_state: Some("running".to_owned()),
        driver_options: Some(0),
        immediate_mode: Some(false),
        in_flight: Some(0),
        at_bot: Some(true),
        accessible: true,
        ready: Some(true),
        medium_loaded: Some(true),
        position_lost: Some(false),
        status: "ready".to_owned(),
        status_detail: Some("integration fixture has loaded scratch media".to_owned()),
        raw_status: None,
        observed_at: Utc::now(),
    }
}

async fn create_loaded_drive(pool: &PgPool) -> store::DriveRecord {
    let observation = loaded_drive_observation();
    let detected = store::record_drive_observation(pool, &observation)
        .await
        .expect("drive observation should persist");
    let reconciled = store::reconcile_drive(pool, detected.id, &observation.stable_id)
        .await
        .expect("drive identity should reconcile");
    let ReconcileDriveResult::Reconciled(reconciled) = reconciled else {
        panic!("exact accessible drive identity should reconcile");
    };
    *reconciled
}

async fn qualify_backup_media(
    pool: &PgPool,
    drive: &store::DriveRecord,
    media: &store::MediaRecord,
) {
    let queued = store::queue_qualification_run(
        pool,
        drive.id,
        &drive.stable_id,
        media.id,
        &media.internal_label,
        true,
        true,
    )
    .await
    .expect("integration media qualification should queue");
    let QueueQualificationRunResult::Queued(queued) = queued else {
        panic!("fresh integration media should be eligible for qualification");
    };
    let claimed = store::claim_next_qualification_run(pool, Uuid::now_v7())
        .await
        .expect("qualification worker should claim integration media");
    let ClaimQualificationRunResult::Claimed(claimed) = claimed else {
        panic!("queued integration qualification should be claimable");
    };
    assert_eq!(claimed.run.id, queued.id);
    assert!(store::finish_qualification_run(
        pool,
        &claimed,
        QualificationTerminalState::Passed,
        json!({
            "detail": "manual-media state integration qualification",
            "position_uncertain": false,
            "engine_version": "15.0.3",
            "maximum_block_size_bytes": 262144
        }),
    )
    .await
    .expect("integration media qualification should finish"));
}

#[tokio::test]
async fn operational_backup_requests_schedule_and_assign_media_durably() {
    let Some((pool, admin, schema)) = isolated_database().await else {
        return;
    };

    let manual_profile =
        create_backup_profile(&pool, "Manual backup integration", "manual-source", false).await;
    assert_eq!(manual_profile.representation, "file");
    assert_eq!(manual_profile.verification_mode, "full_readback");
    assert_eq!(manual_profile.compression_mode, "auto");
    assert!(manual_profile.spanning_enabled);
    assert_eq!(manual_profile.data_delivery, "disk_spool");
    assert!(!manual_profile.production_eligible);

    let invalid_delivery_source = create_source_profile(
        &pool,
        "Invalid delivery integration",
        "invalid-delivery-source",
    )
    .await;
    let mut invalid_delivery = NewBackupProfile::attended_full_file(invalid_delivery_source.id);
    invalid_delivery.data_delivery = "ram_ring".to_owned();
    let invalid_delivery = backup_ops::create_backup_profile(&pool, &invalid_delivery)
        .await
        .expect_err("an invented data delivery mode must fail closed");
    assert!(matches!(
        invalid_delivery,
        backup_ops::BackupOpsError::Invalid(_)
    ));

    let empty_pool_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO media_pools (id, name, purpose, indefinite_retention)
         VALUES ($1, 'Empty backup integration pool', 'scratch', false)",
    )
    .bind(empty_pool_id)
    .execute(&pool)
    .await
    .expect("empty fixed-pool fixture should persist");
    let empty_pool_source =
        create_source_profile(&pool, "Empty pool backup integration", "empty-pool-source").await;
    let mut empty_pool_request = NewBackupProfile::attended_full_file(empty_pool_source.id);
    empty_pool_request.media_pool_id = Some(empty_pool_id);
    empty_pool_request.data_delivery = "disk_spool".to_owned();
    empty_pool_request.actor_label = "Backup integration operator".to_owned();
    let empty_pool_profile = backup_ops::create_backup_profile(&pool, &empty_pool_request)
        .await
        .expect("fixed-pool backup profile should persist");
    assert_eq!(empty_pool_profile.data_delivery, "disk_spool");
    let empty_pool_readiness = store::backup_media_readiness(&pool, empty_pool_profile.id, None)
        .await
        .expect("fixed-pool readiness should be queryable");
    let store::BackupMediaReadinessResult::Readiness(empty_pool_readiness) = empty_pool_readiness
    else {
        panic!("the fixed-pool profile should have a readiness result");
    };
    let empty_pool_blocker = empty_pool_readiness
        .blockers
        .iter()
        .find(|blocker| blocker.code == "no_media_in_pool")
        .expect("an empty fixed pool should direct the operator to tape inventory");
    assert_eq!(empty_pool_blocker.action, "open_media");
    assert_eq!(empty_pool_blocker.related_resource_id, Some(empty_pool_id));
    assert!(!empty_pool_readiness
        .blockers
        .iter()
        .any(|blocker| blocker.code == "no_registered_media"));

    let first = backup_ops::create_manual_backup_request(
        &pool,
        manual_profile.id,
        "integration-manual-001",
        "Backup integration operator",
    )
    .await
    .expect("manual request should persist before media assignment");
    let CreateBackupRequestResult::Created(first) = first else {
        panic!("the first idempotency key should create a request");
    };
    assert_eq!(first.state, "waiting_for_media");
    assert_eq!(first.trigger_kind, "manual");
    assert_eq!(first.requested_level, "full");
    assert!(first.assigned_data_run_id.is_none());

    let retained = backup_ops::get_backup_request_by_idempotency_key(
        &pool,
        manual_profile.id,
        "integration-manual-001",
    )
    .await
    .expect("the retained request should be addressable by its idempotency key")
    .expect("the retained request should exist");
    assert_eq!(retained.id, first.id);
    assert_eq!(retained.job_run_id, first.job_run_id);
    let serialized_retained = backup_ops::get_backup_request_by_idempotency_key_serialized(
        &pool,
        manual_profile.id,
        "integration-manual-001",
    )
    .await
    .expect("serialized replay lookup should succeed")
    .expect("serialized replay lookup should retain the request");
    assert_eq!(serialized_retained.id, first.id);
    assert!(backup_ops::get_backup_request_by_idempotency_key(
        &pool,
        manual_profile.id,
        "integration-manual-missing",
    )
    .await
    .expect("a missing idempotency key lookup should succeed")
    .is_none());

    let replay_source_root = tempfile::tempdir().expect("replay source root should exist");
    let replay_web_root = tempfile::tempdir().expect("replay web root should exist");
    let replay_application = api::router(
        AppState::new(
            Some(pool.clone()),
            false,
            true,
            true,
            true,
            replay_source_root.path().to_path_buf(),
            PathBuf::from("/restore"),
        ),
        replay_web_root.path().to_path_buf(),
    );
    let replay = api_request(
        &replay_application,
        Method::POST,
        format!("/api/v1/backup-profiles/{}/runs", manual_profile.id),
        json!({
            "idempotency_key": "integration-manual-001",
            "requested_by_label": "Backup integration operator"
        }),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    let replay = response_json(replay).await;
    assert_eq!(replay["run"]["id"], json!(first.id));
    assert_eq!(replay["run"]["job_run_id"], json!(first.job_run_id));

    let duplicate = backup_ops::create_manual_backup_request(
        &pool,
        manual_profile.id,
        "integration-manual-001",
        "Backup integration operator",
    )
    .await
    .expect("an idempotent retry should reload the retained request");
    let CreateBackupRequestResult::Existing(duplicate) = duplicate else {
        panic!("a repeated idempotency key must not create another request");
    };
    assert_eq!(duplicate.id, first.id);
    assert_eq!(duplicate.job_run_id, first.job_run_id);
    assert_eq!(
        backup_ops::backup_request_events(&pool, first.id)
            .await
            .expect("request events should reload")
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["requested", "waiting_for_media"]
    );

    let cancelled = backup_ops::cancel_waiting_backup_request(
        &pool,
        first.id,
        "Backup integration operator",
        "integration cancellation before cartridge assignment",
    )
    .await
    .expect("waiting request should cancel without tape I/O");
    let CancelBackupRequestResult::Cancelled(cancelled) = cancelled else {
        panic!("the active waiting request should transition to cancelled");
    };
    assert_eq!(cancelled.state, "cancelled");
    assert_eq!(
        cancelled.cancelled_by_label.as_deref(),
        Some("Backup integration operator")
    );
    let failure_detail = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT failure_detail FROM job_runs WHERE id = $1",
    )
    .bind(cancelled.job_run_id)
    .fetch_one(&pool)
    .await
    .expect("cancellation reason should be retained");
    assert_eq!(
        failure_detail["reason"],
        json!("integration cancellation before cartridge assignment")
    );

    let scheduled_profile = create_backup_profile(
        &pool,
        "Scheduled backup integration",
        "scheduled-source",
        true,
    )
    .await;
    let as_of = DateTime::parse_from_rfc3339("2030-01-05T07:30:00Z")
        .expect("known Saturday scheduler instant should parse")
        .with_timezone(&Utc);
    let due = backup_ops::enqueue_due_backup_requests(&pool, as_of)
        .await
        .expect("latest due New York Saturday occurrence should enqueue");
    assert_eq!(due.created.len(), 1);
    assert!(due.existing.is_empty());
    assert!(due.blocked.is_empty());
    assert_eq!(due.created[0].profile_id, scheduled_profile.id);
    assert_eq!(due.created[0].trigger_kind, "scheduled");
    assert!(due.created[0]
        .scheduled_occurrence
        .as_deref()
        .is_some_and(|occurrence| occurrence.contains("2030-01-05")));
    let retry = backup_ops::enqueue_due_backup_requests(&pool, as_of)
        .await
        .expect("scheduler retry should be idempotent");
    assert!(retry.created.is_empty());
    assert_eq!(retry.existing.len(), 1);
    assert_eq!(retry.existing[0].id, due.created[0].id);

    let assignment_profile = create_backup_profile(
        &pool,
        "Assigned backup integration",
        "assignment-source",
        false,
    )
    .await;
    // The finalization fixture below exercises the preserved v1 evidence API.
    // Native v2 finalization is intentionally gated on its pack/volume seal
    // ledger and is covered by the v2 store and migration contracts.
    sqlx::query(
        "UPDATE single_tape_data_profiles
            SET engine_version = '1', adapter_version = 'native-v1',
                format_version = 'tapescale-native-v1'
          WHERE id = $1",
    )
    .bind(assignment_profile.id)
    .execute(&pool)
    .await
    .expect("legacy finalization fixture should retain its v1 binding");
    let assignment_request = backup_ops::create_manual_backup_request(
        &pool,
        assignment_profile.id,
        "integration-assignment-001",
        "Backup integration operator",
    )
    .await
    .expect("assignment request should persist");
    let assignment_request = assignment_request.record().clone();
    let drive = create_loaded_drive(&pool).await;

    let protected_media = store::create_media(
        &pool,
        &NewMedia {
            internal_label: "BACKUP-INTEGRATION-WORM-001".to_owned(),
            barcode: Some("BIWORM001".to_owned()),
            media_generation: 9,
            media_format: "LTO-9".to_owned(),
            worm: true,
        },
    )
    .await
    .expect("protected media fixture should persist");
    let protected_rejection = store::attach_single_tape_backup_run(
        &pool,
        assignment_profile.id,
        assignment_request.id,
        assignment_request.job_run_id,
        &assignment_request.trigger_kind,
        assignment_request.scheduled_occurrence.as_deref(),
        assignment_profile.retention_days,
        drive.id,
        &drive.stable_id,
        protected_media.id,
        &protected_media.internal_label,
        true,
        true,
        true,
    )
    .await
    .expect("protected media selection should be a fail-closed domain result");
    assert!(matches!(
        protected_rejection,
        QueueSingleTapeDataRunResult::Blocked(reason) if reason.contains("WORM")
    ));
    let still_waiting = backup_ops::get_backup_request(&pool, assignment_request.id)
        .await
        .expect("request should reload")
        .expect("request remains durable");
    assert_eq!(still_waiting.state, "waiting_for_media");
    assert!(still_waiting.assigned_data_run_id.is_none());

    let scratch = store::create_media(
        &pool,
        &NewMedia {
            internal_label: "BACKUP-INTEGRATION-SCRATCH-001".to_owned(),
            barcode: Some("BISCR001".to_owned()),
            media_generation: 9,
            media_format: "LTO-9".to_owned(),
            worm: false,
        },
    )
    .await
    .expect("scratch media fixture should persist");
    let readiness = store::backup_media_readiness(
        &pool,
        assignment_profile.id,
        Some((assignment_request.id, assignment_request.job_run_id)),
    )
    .await
    .expect("readiness preview should be queryable");
    let store::BackupMediaReadinessResult::Readiness(readiness) = readiness else {
        panic!("the retained backup profile should have a readiness result");
    };
    assert_eq!(readiness.state, "blocked");
    assert!(readiness.candidates.is_empty());
    assert!(readiness
        .blockers
        .iter()
        .any(|blocker| blocker.code == "media_worm"
            && blocker.resource_id == Some(protected_media.id)));
    assert!(readiness.blockers.iter().any(|blocker| {
        blocker.code == "setup_test_required"
            && blocker.resource_id == Some(drive.id)
            && blocker.related_resource_id == Some(scratch.id)
    }));

    let blocked_preview = api_request(
        &replay_application,
        Method::GET,
        format!(
            "/api/v1/backup-runs/{}/media-readiness",
            assignment_request.id
        ),
        Value::Null,
    )
    .await;
    assert_eq!(blocked_preview.status(), StatusCode::OK);
    assert_eq!(blocked_preview.headers()[header::CACHE_CONTROL], "no-store");
    let blocked_preview = response_json(blocked_preview).await;
    assert_eq!(blocked_preview["readiness"]["state"], "blocked");
    assert_eq!(blocked_preview["safety_authority"], false);
    assert_eq!(blocked_preview["revalidated_on_assignment"], true);

    let qualification = store::queue_qualification_run(
        &pool,
        drive.id,
        &drive.stable_id,
        scratch.id,
        &scratch.internal_label,
        true,
        true,
    )
    .await
    .expect("exact scratch qualification should queue");
    let QueueQualificationRunResult::Queued(qualification) = qualification else {
        panic!("safe scratch media should be eligible for qualification");
    };
    let claimed = store::claim_next_qualification_run(&pool, Uuid::now_v7())
        .await
        .expect("qualification worker claim should succeed");
    let ClaimQualificationRunResult::Claimed(claimed) = claimed else {
        panic!("queued exact qualification should be claimable");
    };
    assert_eq!(claimed.run.id, qualification.id);
    assert!(store::finish_qualification_run(
        &pool,
        &claimed,
        QualificationTerminalState::Passed,
        json!({
            "detail": "legacy backup integration qualification",
            "position_uncertain": false,
            "engine_version": "15.0.3"
        }),
    )
    .await
    .expect("legacy qualification pass should persist"));

    let legacy_readiness = store::backup_media_readiness(
        &pool,
        assignment_profile.id,
        Some((assignment_request.id, assignment_request.job_run_id)),
    )
    .await
    .expect("legacy qualification readiness should be queryable");
    let store::BackupMediaReadinessResult::Readiness(legacy_readiness) = legacy_readiness else {
        panic!("the retained backup profile should have a readiness result");
    };
    assert_eq!(legacy_readiness.state, "blocked");
    assert!(legacy_readiness.candidates.is_empty());
    assert!(legacy_readiness.blockers.iter().any(|blocker| {
        blocker.code == "setup_test_required"
            && blocker.resource_id == Some(drive.id)
            && blocker.related_resource_id == Some(scratch.id)
    }));

    let current_qualification = store::queue_qualification_run(
        &pool,
        drive.id,
        &drive.stable_id,
        scratch.id,
        &scratch.internal_label,
        true,
        true,
    )
    .await
    .expect("current block-size qualification should queue");
    let QueueQualificationRunResult::Queued(current_qualification) = current_qualification else {
        panic!("prepared scratch media should permit a current setup test");
    };
    let current_claim = store::claim_next_qualification_run(&pool, Uuid::now_v7())
        .await
        .expect("current qualification worker claim should succeed");
    let ClaimQualificationRunResult::Claimed(current_claim) = current_claim else {
        panic!("current qualification should be claimable");
    };
    assert_eq!(current_claim.run.id, current_qualification.id);
    let qualified_scratch_identity = tapescale::qualification::preparation_volume_label(scratch.id);
    assert!(store::finish_qualification_run(
        &pool,
        &current_claim,
        QualificationTerminalState::Passed,
        json!({
            "detail": "backup integration 256 KiB block qualification",
            "position_uncertain": false,
            "engine_version": "15.0.3",
            "maximum_block_size_bytes": 262144,
            "on_tape_identity": qualified_scratch_identity
        }),
    )
    .await
    .expect("current block-size qualification pass should persist"));
    let qualified_scratch = sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT on_tape_identity, identity_status FROM cartridges WHERE id = $1",
    )
    .bind(scratch.id)
    .fetch_one(&pool)
    .await
    .expect("qualified scratch identity should be queryable");
    assert_eq!(
        qualified_scratch.0.as_deref(),
        Some(qualified_scratch_identity.as_str())
    );
    assert_eq!(qualified_scratch.1, "reconciled");

    // Exercise the complete attended continuation/readback request contract on
    // its own run so the later finalization fixture remains single-volume.
    let continuation_first = store::create_media(
        &pool,
        &NewMedia {
            internal_label: "BACKUP-INTEGRATION-SPAN-001".to_owned(),
            barcode: Some("BISPAN001".to_owned()),
            media_generation: 9,
            media_format: "LTO-9".to_owned(),
            worm: false,
        },
    )
    .await
    .expect("first spanning cartridge should persist");
    let continuation_second = store::create_media(
        &pool,
        &NewMedia {
            internal_label: "BACKUP-INTEGRATION-SPAN-002".to_owned(),
            barcode: Some("BISPAN002".to_owned()),
            media_generation: 9,
            media_format: "LTO-9".to_owned(),
            worm: false,
        },
    )
    .await
    .expect("second spanning cartridge should persist");
    qualify_backup_media(&pool, &drive, &continuation_first).await;
    qualify_backup_media(&pool, &drive, &continuation_second).await;

    let continuation_profile = create_backup_profile(
        &pool,
        "Continuation state integration",
        "continuation-source",
        false,
    )
    .await;
    // This fixture exercises the preserved generic/v1 handoff contract. Native
    // v2 continuation authorization intentionally requires the preceding
    // volume's UUID and index to have passed physical readback first.
    sqlx::query(
        "UPDATE single_tape_data_profiles
            SET engine_version = '1', adapter_version = 'native-v1',
                format_version = 'tapescale-native-v1'
          WHERE id = $1",
    )
    .bind(continuation_profile.id)
    .execute(&pool)
    .await
    .expect("legacy continuation fixture should retain its v1 binding");
    let continuation_request = backup_ops::create_manual_backup_request(
        &pool,
        continuation_profile.id,
        "integration-continuation-001",
        "Backup integration operator",
    )
    .await
    .expect("continuation request should persist")
    .record()
    .clone();
    let continuation_run = store::attach_single_tape_backup_run(
        &pool,
        continuation_profile.id,
        continuation_request.id,
        continuation_request.job_run_id,
        &continuation_request.trigger_kind,
        continuation_request.scheduled_occurrence.as_deref(),
        continuation_profile.retention_days,
        drive.id,
        &drive.stable_id,
        continuation_first.id,
        &continuation_first.internal_label,
        true,
        true,
        true,
    )
    .await
    .expect("first spanning cartridge should attach");
    let QueueSingleTapeDataRunResult::Queued(continuation_run) = continuation_run else {
        panic!("the spanning integration run should queue");
    };
    assert_eq!(continuation_run.data_delivery, "disk_spool");
    let continuation_claim = store::claim_next_single_tape_data_run(&pool, Uuid::now_v7())
        .await
        .expect("spanning integration run should be claimable");
    let store::ClaimSingleTapeDataRunResult::Claimed(continuation_claim) = continuation_claim
    else {
        panic!("the spanning integration worker should own the run");
    };
    assert_eq!(continuation_claim.run.id, continuation_run.id);
    assert!(store::transition_single_tape_data_phase(
        &pool,
        &continuation_claim,
        store::SingleTapeDataPhase::Writing,
        json!({"detail": "begin integration spanning write"}),
    )
    .await
    .expect("spanning write phase should persist"));
    let first_engine_volume = "TS-INTEGRATION-SPAN-V1";
    assert!(store::mark_data_run_volume_write_started(
        &pool,
        &continuation_claim,
        1,
        first_engine_volume,
    )
    .await
    .expect("first spanning write boundary should persist"));
    let continuation_live_sample = store::SingleTapeLiveIoSample {
        phase: store::SingleTapeLiveIoPhase::BackupWrite,
        sampled_at: Utc::now(),
        sample_window_ms: Some(5_000),
        stale_after_seconds: 20,
        engine_job_id: 8_001,
        files_processed: 2,
        files_total: Some(10),
        progress_basis: "bacula_job_files_over_manifest_entries",
        archive_entries_per_second: Some(1.0),
        pipeline_bytes_processed: 2_048,
        pipeline_bytes_total: None,
        pipeline_bytes_per_second: Some(400),
        file_daemon_read_bytes_processed: None,
        file_daemon_read_bytes_per_second: None,
        current_file: None,
        file_daemon_basis: None,
        tape_read_bytes_per_second: None,
        tape_write_bytes_per_second: None,
        tape_read_counter_available: false,
        tape_write_counter_available: false,
        tape_counters_available: false,
        pipeline_basis: "bacula_job_bytes_delta",
        tape_basis: None,
        engine_state: None,
        activity: None,
        progress_idle_ms: None,
        no_progress_timeout_ms: None,
        spool_bytes: None,
        spool_capacity_bytes: None,
        spool_target_bytes: None,
        spool_file_count: None,
        spool_high_water_bytes: None,
        spool_low_water_bytes: None,
        spool_pack_count: None,
        spool_reclaimed_bytes: None,
        spool_fill_bytes_per_second: None,
        spool_drain_bytes_per_second: None,
        spool_basis: None,
    };
    assert!(store::record_single_tape_data_live_io(
        &pool,
        &continuation_claim,
        &continuation_live_sample,
    )
    .await
    .expect("spanning write progress should persist before the media wait"));
    let pending_write = store::ensure_data_run_media_request(
        &pool,
        &continuation_claim,
        "backup_write",
        2,
        None,
        None,
    )
    .await
    .expect("continuation media request should persist");
    let store::DataRunMediaRequestResult::Ready(pending_write) = pending_write else {
        panic!("volume two should be requested");
    };
    assert_eq!(pending_write.state, "pending");

    let pending_write_view = api_request(
        &replay_application,
        Method::GET,
        format!("/api/v1/backup-runs/{}", continuation_request.id),
        Value::Null,
    )
    .await;
    assert_eq!(pending_write_view.status(), StatusCode::OK);
    let pending_write_view = response_json(pending_write_view).await;
    assert_eq!(
        pending_write_view["run"]["allowed_actions"]["assign_media"],
        false
    );
    assert_eq!(
        pending_write_view["run"]["allowed_actions"]["assign_next_media"],
        true
    );
    assert_eq!(
        pending_write_view["run"]["media_request"]["state"],
        "pending"
    );
    assert!(pending_write_view["run"]["result"]["live_io"].is_null());
    assert_eq!(
        pending_write_view["run"]["result"]["transfer_progress"]["pipeline_bytes_processed"],
        2_048
    );
    assert_eq!(
        pending_write_view["run"]["result"]["transfer_progress"]["files_total"],
        10
    );

    let continuation_confirmation = format!(
        "OVERWRITE {} FOR BACKUP {} VOLUME 2",
        continuation_second.internal_label, continuation_request.id
    );
    let continuation_assignment = json!({
        "drive_id": drive.id,
        "expected_stable_id": drive.stable_id,
        "cartridge_id": continuation_second.id,
        "volume_sequence": 2,
        "typed_cartridge_label": continuation_second.internal_label,
        "loaded_media_confirmed": true,
        "destructive_overwrite_confirmed": true,
        "encryption_deviation_confirmed": true,
        "confirmation": continuation_confirmation
    });
    let authorized_write = api_request(
        &replay_application,
        Method::POST,
        format!("/api/v1/backup-runs/{}/media", continuation_request.id),
        continuation_assignment.clone(),
    )
    .await;
    assert_eq!(authorized_write.status(), StatusCode::ACCEPTED);
    let authorized_write = response_json(authorized_write).await;
    assert_eq!(authorized_write["media_request"]["state"], "authorized");
    assert_eq!(authorized_write["idempotent_replay"], false);

    let authorized_write_view = api_request(
        &replay_application,
        Method::GET,
        format!("/api/v1/backup-runs/{}", continuation_request.id),
        Value::Null,
    )
    .await;
    let authorized_write_view = response_json(authorized_write_view).await;
    assert_eq!(
        authorized_write_view["run"]["allowed_actions"]["assign_media"],
        false
    );
    assert_eq!(
        authorized_write_view["run"]["allowed_actions"]["assign_next_media"],
        false
    );
    assert_eq!(
        authorized_write_view["run"]["media_request"]["state"],
        "authorized"
    );
    let authorized_readiness = api_request(
        &replay_application,
        Method::GET,
        format!(
            "/api/v1/backup-runs/{}/media-readiness",
            continuation_request.id
        ),
        Value::Null,
    )
    .await;
    let authorized_readiness = response_json(authorized_readiness).await;
    assert_eq!(
        authorized_readiness["readiness"]["state"],
        "media_authorized"
    );
    assert_eq!(
        authorized_readiness["readiness"]["counts"]["candidates_returned"],
        0
    );

    assert!(
        store::mark_data_run_media_loaded(&pool, &continuation_claim, pending_write.id,)
            .await
            .expect("authorized volume two should become loaded")
    );
    let loaded_write_view = api_request(
        &replay_application,
        Method::GET,
        format!("/api/v1/backup-runs/{}", continuation_request.id),
        Value::Null,
    )
    .await;
    let loaded_write_view = response_json(loaded_write_view).await;
    assert_eq!(loaded_write_view["run"]["media_request"]["state"], "loaded");
    assert_eq!(
        loaded_write_view["run"]["allowed_actions"]["assign_media"],
        false
    );
    assert_eq!(
        loaded_write_view["run"]["allowed_actions"]["assign_next_media"],
        false
    );
    let second_engine_volume = "TS-INTEGRATION-SPAN-V2";
    assert!(store::mark_data_run_volume_write_started(
        &pool,
        &continuation_claim,
        2,
        second_engine_volume,
    )
    .await
    .expect("second spanning write boundary should persist"));
    assert!(store::complete_data_run_media_request(
        &pool,
        &continuation_claim,
        pending_write.id,
        true,
    )
    .await
    .expect("volume two request should complete"));
    assert!(
        store::open_data_run_media_request(&pool, continuation_run.id)
            .await
            .expect("open request lookup should succeed")
            .is_none()
    );

    let completed_write_replay = api_request(
        &replay_application,
        Method::POST,
        format!("/api/v1/backup-runs/{}/media", continuation_request.id),
        continuation_assignment,
    )
    .await;
    assert_eq!(completed_write_replay.status(), StatusCode::OK);
    let completed_write_replay = response_json(completed_write_replay).await;
    assert_eq!(completed_write_replay["idempotent_replay"], true);
    assert_eq!(
        completed_write_replay["media_request"]["state"],
        "completed"
    );

    sqlx::query(
        "UPDATE single_tape_data_run_volumes
            SET state = 'written', written_at = now(), updated_at = now()
          WHERE single_tape_data_run_id = $1 AND volume_order = 1
            AND state = 'write_started'",
    )
    .bind(continuation_run.id)
    .execute(&pool)
    .await
    .expect("integration fixture should record volume one as written");
    assert!(store::transition_single_tape_data_phase(
        &pool,
        &continuation_claim,
        store::SingleTapeDataPhase::Written,
        json!({"detail": "integration multi-volume write complete"}),
    )
    .await
    .expect("written phase should persist"));
    assert!(store::transition_single_tape_data_phase(
        &pool,
        &continuation_claim,
        store::SingleTapeDataPhase::Verifying,
        json!({"detail": "integration multi-volume readback"}),
    )
    .await
    .expect("verifying phase should persist"));
    let pending_readback = store::ensure_data_run_media_request(
        &pool,
        &continuation_claim,
        "full_readback",
        1,
        Some(continuation_first.id),
        Some(first_engine_volume),
    )
    .await
    .expect("ordered readback request should persist");
    let store::DataRunMediaRequestResult::Ready(pending_readback) = pending_readback else {
        panic!("archive volume one should be requested for readback");
    };

    let identity_observed_at = store::list_drives(&pool)
        .await
        .expect("loaded-drive observation should be queryable")
        .into_iter()
        .find(|candidate| candidate.id == drive.id)
        .and_then(|candidate| candidate.last_observed_at)
        .expect("loaded drive should retain its observation timestamp");
    let wrong_loaded_tape = store::record_loaded_tape_identity(
        &pool,
        drive.id,
        &drive.stable_id,
        drive
            .observed_path
            .as_deref()
            .expect("loaded drive should retain its device path"),
        Some(second_engine_volume),
        true,
        Some("integration probe intentionally observed archive volume two"),
        identity_observed_at,
    )
    .await
    .expect("wrong loaded-tape identity should resolve durably");
    assert!(matches!(
        wrong_loaded_tape,
        store::LoadedTapeIdentityResolution::Known(ref identity)
            if identity.cartridge_id == continuation_second.id
    ));
    let wrong_tape_readiness = store::backup_readback_media_readiness(
        &pool,
        continuation_profile.id,
        continuation_request.id,
        continuation_run.id,
        pending_readback.id,
    )
    .await
    .expect("wrong-tape readback readiness should be queryable");
    let store::BackupMediaReadinessResult::Readiness(wrong_tape_readiness) = wrong_tape_readiness
    else {
        panic!("pending readback should retain a readiness result");
    };
    assert_eq!(wrong_tape_readiness.state, "blocked");
    assert!(wrong_tape_readiness
        .blockers
        .iter()
        .any(|blocker| blocker.code == "wrong_tape_loaded"));

    let expected_loaded_tape = store::record_loaded_tape_identity(
        &pool,
        drive.id,
        &drive.stable_id,
        drive
            .observed_path
            .as_deref()
            .expect("loaded drive should retain its device path"),
        Some(first_engine_volume),
        true,
        Some("integration probe observed the requested archive volume"),
        identity_observed_at,
    )
    .await
    .expect("expected loaded-tape identity should resolve durably");
    assert!(matches!(
        expected_loaded_tape,
        store::LoadedTapeIdentityResolution::Known(ref identity)
            if identity.cartridge_id == continuation_first.id
    ));

    sqlx::query("UPDATE cartridges SET identity_status = 'mismatch' WHERE id = $1")
        .bind(continuation_first.id)
        .execute(&pool)
        .await
        .expect("identity-conflict fixture should persist");
    let conflicted_readiness = api_request(
        &replay_application,
        Method::GET,
        format!(
            "/api/v1/backup-runs/{}/media-readiness",
            continuation_request.id
        ),
        Value::Null,
    )
    .await;
    let conflicted_readiness = response_json(conflicted_readiness).await;
    assert_eq!(conflicted_readiness["readiness"]["state"], "blocked");
    assert!(conflicted_readiness["readiness"]["blockers"]
        .as_array()
        .expect("readback blockers should be an array")
        .iter()
        .any(|blocker| blocker["code"] == "media_identity_conflict"));
    sqlx::query("UPDATE cartridges SET identity_status = 'unknown' WHERE id = $1")
        .bind(continuation_first.id)
        .execute(&pool)
        .await
        .expect("post-write unknown identity fixture should be restored");

    let pending_readback_view = api_request(
        &replay_application,
        Method::GET,
        format!("/api/v1/backup-runs/{}", continuation_request.id),
        Value::Null,
    )
    .await;
    let pending_readback_view = response_json(pending_readback_view).await;
    assert_eq!(
        pending_readback_view["run"]["allowed_actions"]["assign_media"],
        true
    );
    assert_eq!(
        pending_readback_view["run"]["allowed_actions"]["assign_next_media"],
        false
    );
    let readback_confirmation = format!(
        "LOAD {} FOR BACKUP {} READBACK VOLUME 1",
        continuation_first.internal_label, continuation_request.id
    );
    let readback_assignment = json!({
        "drive_id": drive.id,
        "expected_stable_id": drive.stable_id,
        "cartridge_id": continuation_first.id,
        "volume_sequence": 1,
        "typed_cartridge_label": continuation_first.internal_label,
        "loaded_media_confirmed": true,
        "destructive_overwrite_confirmed": false,
        "encryption_deviation_confirmed": false,
        "confirmation": readback_confirmation
    });
    let authorized_readback = api_request(
        &replay_application,
        Method::POST,
        format!("/api/v1/backup-runs/{}/media", continuation_request.id),
        readback_assignment.clone(),
    )
    .await;
    assert_eq!(authorized_readback.status(), StatusCode::ACCEPTED);
    let authorized_readback = response_json(authorized_readback).await;
    assert_eq!(authorized_readback["media_request"]["state"], "authorized");
    assert_eq!(authorized_readback["destructive"], false);
    assert_eq!(
        authorized_readback["media_request"]["confirmation_evidence"]["drive_id"],
        json!(drive.id)
    );
    assert_eq!(
        authorized_readback["media_request"]["confirmation_evidence"]["expected_stable_id"],
        json!(drive.stable_id)
    );
    assert_eq!(
        authorized_readback["media_request"]["confirmation_evidence"]["typed_cartridge_label"],
        json!(continuation_first.internal_label)
    );
    let authorized_readback_view = api_request(
        &replay_application,
        Method::GET,
        format!("/api/v1/backup-runs/{}", continuation_request.id),
        Value::Null,
    )
    .await;
    let authorized_readback_view = response_json(authorized_readback_view).await;
    assert_eq!(
        authorized_readback_view["run"]["allowed_actions"]["assign_media"],
        false
    );
    assert_eq!(
        authorized_readback_view["run"]["allowed_actions"]["assign_next_media"],
        false
    );
    let mut changed_drive_replay = readback_assignment.clone();
    changed_drive_replay["drive_id"] = json!(Uuid::now_v7());
    changed_drive_replay["expected_stable_id"] = json!("scsi.naa.integration-other-drive");
    let changed_drive_replay = api_request(
        &replay_application,
        Method::POST,
        format!("/api/v1/backup-runs/{}/media", continuation_request.id),
        changed_drive_replay,
    )
    .await;
    assert_eq!(changed_drive_replay.status(), StatusCode::CONFLICT);
    assert!(
        store::mark_data_run_media_loaded(&pool, &continuation_claim, pending_readback.id,)
            .await
            .expect("authorized readback tape should become loaded")
    );
    let loaded_readback_view = api_request(
        &replay_application,
        Method::GET,
        format!("/api/v1/backup-runs/{}", continuation_request.id),
        Value::Null,
    )
    .await;
    let loaded_readback_view = response_json(loaded_readback_view).await;
    assert_eq!(
        loaded_readback_view["run"]["media_request"]["state"],
        "loaded"
    );
    assert_eq!(
        loaded_readback_view["run"]["allowed_actions"]["assign_media"],
        false
    );
    assert_eq!(
        loaded_readback_view["run"]["allowed_actions"]["assign_next_media"],
        false
    );
    assert!(store::complete_data_run_media_request(
        &pool,
        &continuation_claim,
        pending_readback.id,
        false,
    )
    .await
    .expect("readback request should complete"));
    let next_readback = store::ensure_data_run_media_request(
        &pool,
        &continuation_claim,
        "full_readback",
        2,
        Some(continuation_second.id),
        Some(second_engine_volume),
    )
    .await
    .expect("next ordered readback request should persist");
    let store::DataRunMediaRequestResult::Ready(next_readback) = next_readback else {
        panic!("archive volume two should be requested for readback");
    };
    assert_eq!(next_readback.state, "pending");
    assert_ne!(next_readback.id, pending_readback.id);
    let mut changed_readback_evidence = readback_assignment.clone();
    changed_readback_evidence["encryption_deviation_confirmed"] = json!(true);
    let changed_readback_replay = api_request(
        &replay_application,
        Method::POST,
        format!("/api/v1/backup-runs/{}/media", continuation_request.id),
        changed_readback_evidence,
    )
    .await;
    assert!(matches!(
        changed_readback_replay.status(),
        StatusCode::BAD_REQUEST | StatusCode::CONFLICT
    ));
    let completed_readback_replay = api_request(
        &replay_application,
        Method::POST,
        format!("/api/v1/backup-runs/{}/media", continuation_request.id),
        readback_assignment,
    )
    .await;
    assert_eq!(completed_readback_replay.status(), StatusCode::OK);
    let completed_readback_replay = response_json(completed_readback_replay).await;
    assert_eq!(completed_readback_replay["idempotent_replay"], true);
    assert_eq!(
        completed_readback_replay["media_request"]["state"],
        "completed"
    );
    assert_eq!(
        completed_readback_replay["run"]["media_request"]["id"],
        json!(next_readback.id)
    );
    assert_eq!(
        completed_readback_replay["run"]["media_request"]["state"],
        "pending"
    );
    assert!(store::finish_single_tape_data_run(
        &pool,
        &continuation_claim,
        store::SingleTapeDataTerminalState::Blocked,
        json!({
            "detail": "integration media-request state contract complete",
            "position_uncertain": false
        }),
    )
    .await
    .expect("integration spanning run should release its resources"));
    let terminal_continuation_result =
        sqlx::query_scalar::<_, Value>("SELECT result FROM single_tape_data_runs WHERE id = $1")
            .bind(continuation_run.id)
            .fetch_one(&pool)
            .await
            .expect("terminal spanning progress should remain queryable");
    assert_eq!(
        terminal_continuation_result["transfer_progress"]["phase"],
        "backup_write"
    );
    assert_eq!(
        terminal_continuation_result["transfer_progress"]["pipeline_bytes_processed"],
        2_048
    );
    assert!(terminal_continuation_result["live_io"].is_null());

    let decoy_scratch = store::create_media(
        &pool,
        &NewMedia {
            internal_label: "BACKUP-INTEGRATION-SCRATCH-002".to_owned(),
            barcode: Some("BISCR002".to_owned()),
            media_generation: 9,
            media_format: "LTO-9".to_owned(),
            worm: false,
        },
    )
    .await
    .expect("second eligible scratch cartridge should persist");
    qualify_backup_media(&pool, &drive, &decoy_scratch).await;

    let mut unloaded_observation = loaded_drive_observation();
    unloaded_observation.ready = Some(false);
    unloaded_observation.medium_loaded = Some(false);
    unloaded_observation.at_bot = None;
    unloaded_observation.status = "no_media".to_owned();
    unloaded_observation.status_detail = Some("integration fixture changed cartridges".to_owned());
    store::record_drive_observation(&pool, &unloaded_observation)
        .await
        .expect("unloaded observation should clear the prior loaded identity");
    let reloaded_observation = loaded_drive_observation();
    store::record_drive_observation(&pool, &reloaded_observation)
        .await
        .expect("reloaded observation should restore drive readiness");

    let unprobed_readiness = store::backup_media_readiness(&pool, assignment_profile.id, None)
        .await
        .expect("unprobed readiness should expose both eligible scratch cartridges");
    let store::BackupMediaReadinessResult::Readiness(unprobed_readiness) = unprobed_readiness
    else {
        panic!("the retained backup profile should have a readiness result");
    };
    assert_eq!(unprobed_readiness.state, "selection_required");
    let mut unprobed_candidates = unprobed_readiness
        .candidates
        .iter()
        .map(|candidate| candidate.cartridge_id)
        .collect::<Vec<_>>();
    unprobed_candidates.sort_unstable();
    let mut expected_candidates = vec![scratch.id, decoy_scratch.id];
    expected_candidates.sort_unstable();
    assert_eq!(unprobed_candidates, expected_candidates);

    let identity_observed_at = store::list_drives(&pool)
        .await
        .expect("drive identity observation should remain queryable")
        .into_iter()
        .find(|candidate| candidate.id == drive.id)
        .and_then(|candidate| candidate.last_observed_at)
        .expect("ready drive should retain an observation timestamp");
    let loaded_identity = store::record_loaded_tape_identity(
        &pool,
        drive.id,
        &drive.stable_id,
        drive
            .observed_path
            .as_deref()
            .expect("ready drive should retain its device path"),
        Some(&qualified_scratch_identity),
        true,
        Some("integration probe identified the first prepared scratch cartridge"),
        identity_observed_at,
    )
    .await
    .expect("known loaded scratch identity should persist");
    assert!(matches!(
        loaded_identity,
        store::LoadedTapeIdentityResolution::Known(ref identity)
            if identity.cartridge_id == scratch.id
                && identity.volume_name == qualified_scratch_identity
    ));
    let observed_drive = store::list_drives(&pool)
        .await
        .expect("loaded identity health should be queryable")
        .into_iter()
        .find(|candidate| candidate.id == drive.id)
        .expect("loaded drive should remain registered");
    assert_eq!(
        observed_drive.health["loaded_tape_identity_status"],
        json!("known")
    );
    assert_eq!(
        observed_drive.health["loaded_tape_identity_verified"],
        json!(true)
    );
    assert_eq!(
        observed_drive.health["loaded_cartridge_id"],
        json!(scratch.id)
    );

    // The preparation label embeds the cartridge UUID. If the physical write
    // succeeds immediately before a process/database failure, the next probe
    // must reconstruct identity without relying on the lost metadata commit.
    sqlx::query(
        "UPDATE cartridges
            SET on_tape_identity = NULL, identity_status = 'unknown'
          WHERE id = $1",
    )
    .bind(scratch.id)
    .execute(&pool)
    .await
    .expect("integration fixture should simulate a lost preparation-label commit");
    assert_eq!(
        store::media_on_tape_identity(&pool, scratch.id)
            .await
            .expect("expected identity fallback should be queryable"),
        Some((
            scratch.internal_label.clone(),
            qualified_scratch_identity.clone()
        ))
    );
    let recovered_identity = store::record_loaded_tape_identity(
        &pool,
        drive.id,
        &drive.stable_id,
        drive
            .observed_path
            .as_deref()
            .expect("ready drive should retain its device path"),
        Some(&qualified_scratch_identity),
        true,
        Some("integration probe recovered the deterministic preparation identity"),
        identity_observed_at,
    )
    .await
    .expect("deterministic preparation identity should recover without retained metadata");
    assert!(matches!(
        recovered_identity,
        store::LoadedTapeIdentityResolution::Known(ref identity)
            if identity.cartridge_id == scratch.id
                && identity.volume_name == qualified_scratch_identity
    ));
    sqlx::query(
        "UPDATE cartridges
            SET on_tape_identity = $1, identity_status = 'reconciled'
          WHERE id = $2",
    )
    .bind(&qualified_scratch_identity)
    .bind(scratch.id)
    .execute(&pool)
    .await
    .expect("integration fixture should restore prepared tape metadata");

    let detected_readiness = store::backup_media_readiness(&pool, assignment_profile.id, None)
        .await
        .expect("detected-tape readiness should be queryable");
    let store::BackupMediaReadinessResult::Readiness(detected_readiness) = detected_readiness
    else {
        panic!("the retained backup profile should have a readiness result");
    };
    assert_eq!(detected_readiness.state, "ready");
    assert_eq!(detected_readiness.candidates.len(), 1);
    assert_eq!(detected_readiness.candidates[0].cartridge_id, scratch.id);
    assert!(detected_readiness
        .candidates
        .iter()
        .all(|candidate| candidate.cartridge_id != decoy_scratch.id));

    sqlx::query(
        "INSERT INTO cartridges
            (internal_label, media_generation, media_format, worm)
         SELECT 'ZZZ-BACKUP-BOUND-' || lpad(series::text, 4, '0'), 9, 'LTO-9', false
         FROM generate_series(1, 300) AS series",
    )
    .execute(&pool)
    .await
    .expect("bounded-readiness inventory fixtures should persist");

    let profile_readiness = store::backup_media_readiness(&pool, assignment_profile.id, None)
        .await
        .expect("profile readiness should use the bounded set-based query");
    let store::BackupMediaReadinessResult::Readiness(profile_readiness) = profile_readiness else {
        panic!("the retained backup profile should have a readiness result");
    };
    assert_eq!(profile_readiness.state, "ready");
    assert_eq!(profile_readiness.counts.candidates_matched, 1);
    assert_eq!(profile_readiness.counts.candidates_returned, 1);
    assert!(profile_readiness.counts.pairs_evaluated <= profile_readiness.limits.pairs);
    assert!(!profile_readiness.truncated.drives);
    assert!(profile_readiness.truncated.cartridges);
    assert_eq!(
        profile_readiness.counts.cartridges_evaluated,
        profile_readiness.limits.cartridges
    );
    assert!(profile_readiness
        .blockers
        .iter()
        .any(|blocker| blocker.code == "readiness_scan_truncated"));
    assert_eq!(
        profile_readiness
            .recommended_candidate
            .as_ref()
            .map(|candidate| candidate.cartridge_id),
        Some(scratch.id)
    );
    assert!(profile_readiness
        .recommended_candidate
        .as_ref()
        .is_some_and(|candidate| candidate.required_confirmation.is_none()));

    let run_readiness = store::backup_media_readiness(
        &pool,
        assignment_profile.id,
        Some((assignment_request.id, assignment_request.job_run_id)),
    )
    .await
    .expect("request readiness should use the same bounded query");
    let store::BackupMediaReadinessResult::Readiness(run_readiness) = run_readiness else {
        panic!("the retained backup request should have a readiness result");
    };
    let candidate = run_readiness
        .recommended_candidate
        .as_ref()
        .expect("the one qualified pair should be recommended");
    assert_eq!(candidate.drive_id, drive.id);
    assert_eq!(candidate.cartridge_id, scratch.id);
    assert_eq!(
        candidate.qualification_run_id, current_qualification.id,
        "write readiness must retain the newer 256 KiB qualification, not the legacy pass"
    );
    let expected_confirmation = format!(
        "OVERWRITE {} FOR BACKUP {}",
        scratch.internal_label, assignment_request.id
    );
    assert_eq!(
        candidate.required_confirmation.as_deref(),
        Some(expected_confirmation.as_str())
    );

    let profile_preview = api_request(
        &replay_application,
        Method::GET,
        format!(
            "/api/v1/backup-profiles/{}/media-readiness",
            assignment_profile.id
        ),
        Value::Null,
    )
    .await;
    assert_eq!(profile_preview.status(), StatusCode::OK);
    assert_eq!(profile_preview.headers()[header::CACHE_CONTROL], "no-store");
    let profile_preview = response_json(profile_preview).await;
    assert_eq!(profile_preview["readiness"]["state"], "blocked");
    assert_eq!(
        profile_preview["readiness"]["blockers"][0]["code"],
        "snapshot_unavailable"
    );
    assert_eq!(
        profile_preview["readiness"]["blockers"][0]["action"],
        "choose_snapshot"
    );
    assert_eq!(
        profile_preview["readiness"]["counts"]["blockers_returned"],
        1
    );
    assert_eq!(profile_preview["readiness"]["limits"]["candidates"], 256);
    assert_eq!(
        profile_preview["readiness"]["truncated"]["candidates"],
        false
    );

    let source_blocked_preview = api_request(
        &replay_application,
        Method::GET,
        format!(
            "/api/v1/backup-runs/{}/media-readiness",
            assignment_request.id
        ),
        Value::Null,
    )
    .await;
    assert_eq!(source_blocked_preview.status(), StatusCode::OK);
    let source_blocked_preview = response_json(source_blocked_preview).await;
    assert_eq!(source_blocked_preview["readiness"]["state"], "blocked");
    assert_eq!(
        source_blocked_preview["readiness"]["blockers"][0]["code"],
        "snapshot_unavailable"
    );

    let assignment = store::attach_single_tape_backup_run(
        &pool,
        assignment_profile.id,
        assignment_request.id,
        assignment_request.job_run_id,
        &assignment_request.trigger_kind,
        assignment_request.scheduled_occurrence.as_deref(),
        assignment_profile.retention_days,
        drive.id,
        &drive.stable_id,
        scratch.id,
        &scratch.internal_label,
        true,
        true,
        true,
    )
    .await
    .expect("qualified exact media should atomically attach");
    let QueueSingleTapeDataRunResult::Queued(assignment) = assignment else {
        panic!("qualified exact backup assignment should queue");
    };
    assert_eq!(assignment.job_run_id, assignment_request.job_run_id);
    assert_eq!(assignment.workflow_kind, "backup");
    assert_eq!(assignment.trigger_kind, "manual");
    assert_eq!(assignment.requested_level, "full");
    assert_eq!(assignment.retention_days, assignment_profile.retention_days);
    assert_eq!(assignment.compression_mode, "auto");
    assert_eq!(assignment.resolved_compression, "zstd");
    assert!(assignment.spanning_enabled);
    assert_eq!(assignment.data_delivery, "disk_spool");
    let assignment_volumes = store::data_run_volumes(&pool, assignment.id)
        .await
        .expect("the ordered initial tape assignment should reload");
    assert_eq!(assignment_volumes.len(), 1);
    assert_eq!(assignment_volumes[0].volume_order, 1);
    assert_eq!(assignment_volumes[0].cartridge_id, scratch.id);
    assert_eq!(assignment_volumes[0].state, "authorized");
    let assigned_request = backup_ops::get_backup_request(&pool, assignment_request.id)
        .await
        .expect("assigned request should reload")
        .expect("assigned request remains durable");
    assert_eq!(assigned_request.state, "queued");
    assert_eq!(assigned_request.assigned_data_run_id, Some(assignment.id));
    let assigned_preview = api_request(
        &replay_application,
        Method::GET,
        format!(
            "/api/v1/backup-runs/{}/media-readiness",
            assignment_request.id
        ),
        Value::Null,
    )
    .await;
    assert_eq!(assigned_preview.status(), StatusCode::OK);
    let assigned_preview = response_json(assigned_preview).await;
    assert_eq!(assigned_preview["readiness"]["state"], "assigned");
    assert_eq!(
        assigned_preview["readiness"]["counts"]["candidates_returned"],
        0
    );
    assert_eq!(assigned_preview["readiness"]["truncated"]["pairs"], false);
    assert_eq!(
        backup_ops::backup_request_events(&pool, assignment_request.id)
            .await
            .expect("assignment evidence should reload")
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec!["requested", "waiting_for_media", "assignment_recorded"]
    );

    let web_directory = tempfile::tempdir().expect("temporary web directory should exist");
    let evidence_directory =
        tempfile::tempdir().expect("temporary evidence directory should exist");
    let application = api::router(
        AppState::new(
            Some(pool.clone()),
            false,
            true,
            true,
            true,
            PathBuf::from("/source"),
            PathBuf::from("/restore"),
        )
        .with_evidence_root(evidence_directory.path().to_path_buf()),
        web_directory.path().to_path_buf(),
    );
    let assignment_body = json!({
        "drive_id": drive.id,
        "expected_stable_id": drive.stable_id,
        "cartridge_id": scratch.id,
        "typed_cartridge_label": scratch.internal_label,
        "loaded_media_confirmed": true,
        "destructive_overwrite_confirmed": true,
        "encryption_deviation_confirmed": true,
        "confirmation": format!(
            "OVERWRITE {} FOR BACKUP {}",
            scratch.internal_label, assignment_request.id
        )
    });
    let replay = api_request(
        &application,
        Method::POST,
        format!("/api/v1/backup-runs/{}/media", assignment_request.id),
        assignment_body.clone(),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    let replay = response_json(replay).await;
    assert_eq!(replay["data_run_id"], json!(assignment.id));
    assert_eq!(replay["idempotent_replay"], true);
    assert_eq!(replay["audit"], "existing");
    assert_eq!(
        backup_ops::backup_request_events(&pool, assignment_request.id)
            .await
            .expect("assignment replay must preserve event history")
            .iter()
            .filter(|event| event.event_type == "assignment_recorded")
            .count(),
        1
    );

    let mut conflicting_assignment = assignment_body;
    conflicting_assignment["expected_stable_id"] = json!("serial:different-drive");
    let conflict = api_request(
        &application,
        Method::POST,
        format!("/api/v1/backup-runs/{}/media", assignment_request.id),
        conflicting_assignment,
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let cancellation_body = json!({
        "requested_by_label": "Backup cancellation operator",
        "reason": "cancel assigned integration backup before worker claim"
    });
    let cancelled = api_request(
        &application,
        Method::POST,
        format!("/api/v1/backup-runs/{}/cancel", assignment_request.id),
        cancellation_body.clone(),
    )
    .await;
    assert_eq!(cancelled.status(), StatusCode::OK);
    let cancelled = response_json(cancelled).await;
    assert_eq!(cancelled["idempotent_replay"], false);
    assert_eq!(cancelled["audit"], "appended");

    let cancellation_replay = api_request(
        &application,
        Method::POST,
        format!("/api/v1/backup-runs/{}/cancel", assignment_request.id),
        cancellation_body,
    )
    .await;
    assert_eq!(cancellation_replay.status(), StatusCode::OK);
    let cancellation_replay = response_json(cancellation_replay).await;
    assert_eq!(cancellation_replay["idempotent_replay"], true);
    assert_eq!(cancellation_replay["audit"], "existing");

    let cancelled_assignment = store::get_single_tape_data_run(&pool, assignment.id)
        .await
        .expect("cancelled data run should reload")
        .expect("cancelled data run should remain durable");
    assert_eq!(cancelled_assignment.state, "cancelled");
    assert!(!cancelled_assignment.media_write_started);
    let cancelled_request = backup_ops::get_backup_request(&pool, assignment_request.id)
        .await
        .expect("cancelled assigned request should reload")
        .expect("cancelled assigned request remains durable");
    assert_eq!(cancelled_request.state, "cancelled");
    assert_eq!(
        cancelled_request.cancelled_by_label.as_deref(),
        Some("Backup cancellation operator")
    );
    let cancellation_events = backup_ops::backup_request_events(&pool, assignment_request.id)
        .await
        .expect("assigned cancellation evidence should reload");
    assert_eq!(
        cancellation_events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        vec![
            "requested",
            "waiting_for_media",
            "assignment_recorded",
            "cancelled"
        ]
    );
    assert_eq!(
        cancellation_events.last().expect("cancel event").evidence["reason"],
        json!("cancel assigned integration backup before worker claim")
    );

    let final_request = backup_ops::create_manual_backup_request(
        &pool,
        assignment_profile.id,
        "integration-finalization-001",
        "Backup integration operator",
    )
    .await
    .expect("a new request should be allowed after cancellation");
    let final_request = final_request.record().clone();
    let final_run = store::attach_single_tape_backup_run(
        &pool,
        assignment_profile.id,
        final_request.id,
        final_request.job_run_id,
        &final_request.trigger_kind,
        final_request.scheduled_occurrence.as_deref(),
        assignment_profile.retention_days,
        drive.id,
        &drive.stable_id,
        scratch.id,
        &scratch.internal_label,
        true,
        true,
        true,
    )
    .await
    .expect("qualified scratch should be reusable because the cancelled run never touched tape");
    let QueueSingleTapeDataRunResult::Queued(final_run) = final_run else {
        panic!("the finalization fixture should queue");
    };
    assert_eq!(final_run.data_delivery, "disk_spool");
    let claimed = store::claim_next_single_tape_data_run(&pool, Uuid::now_v7())
        .await
        .expect("backup worker claim should succeed");
    let store::ClaimSingleTapeDataRunResult::Claimed(claimed) = claimed else {
        panic!("the exact qualified backup should be claimable");
    };
    assert_eq!(claimed.run.id, final_run.id);
    assert!(store::transition_single_tape_data_phase(
        &pool,
        &claimed,
        store::SingleTapeDataPhase::Inventorying,
        json!({"detail": "integration source inventory"}),
    )
    .await
    .expect("inventory phase should persist"));
    let manifest_source = tempfile::tempdir().expect("manifest source should exist");
    fs::write(
        manifest_source.path().join("payload.bin"),
        vec![0_u8; 1_024],
    )
    .expect("manifest payload should be written");
    let source_manifest = build_manifest(manifest_source.path()).expect("manifest should build");
    let source_manifest_artifact = persist_manifest(
        &evidence_directory.path().join(final_run.id.to_string()),
        "source-manifest",
        &source_manifest,
    )
    .expect("manifest artifact should persist");
    assert!(store::append_single_tape_data_artifact(
        &pool,
        &claimed,
        "source_manifest",
        source_manifest_artifact
            .storage_path
            .to_str()
            .expect("artifact path should be UTF-8"),
        &source_manifest_artifact.artifact_sha256,
        source_manifest_artifact.size_bytes,
        json!({
            "manifest_digest": source_manifest.digest_sha256,
            "entry_count": source_manifest.entry_count,
            "file_count": source_manifest.file_count,
            "logical_bytes": source_manifest.logical_bytes,
            "coverage": {"complete": true}
        }),
    )
    .await
    .expect("source manifest artifact should persist"));
    let preparation_sampled_at = Utc::now();
    let preparation_sample = store::SingleTapePreparationProgress {
        stage: store::SingleTapePreparationStage::SourceInventory,
        step: "digesting_manifest",
        started_at: preparation_sampled_at,
        sampled_at: preparation_sampled_at,
        elapsed_ms: 5_000,
        sample_window_ms: Some(5_000),
        stale_after_seconds: 20,
        entries_discovered: source_manifest.entry_count,
        files_discovered: source_manifest.file_count,
        files_hashed: source_manifest.file_count,
        logical_bytes_discovered: source_manifest.logical_bytes,
        bytes_hashed: source_manifest.logical_bytes,
        hash_bytes_per_second: Some(200),
        basis: "sha256_source_bytes_read",
    };
    assert!(store::record_single_tape_data_preparation_progress(
        &pool,
        &claimed,
        &preparation_sample,
    )
    .await
    .expect("the fenced preparation sample should persist"));
    let wrong_stage_sample = store::SingleTapePreparationProgress {
        stage: store::SingleTapePreparationStage::SourceStabilityCheck,
        ..preparation_sample.clone()
    };
    assert!(!store::record_single_tape_data_preparation_progress(
        &pool,
        &claimed,
        &wrong_stage_sample,
    )
    .await
    .expect("a preparation sample for the wrong durable phase should be rejected"));
    assert!(
        store::overview_counts(&pool)
            .await
            .expect("inventorying overview should query")
            .active_jobs
            >= 1
    );
    let inventory_view = api_request(
        &replay_application,
        Method::GET,
        format!("/api/v1/backup-runs/{}", final_request.id),
        Value::Null,
    )
    .await;
    assert_eq!(inventory_view.status(), StatusCode::OK);
    let inventory_view = response_json(inventory_view).await;
    assert_eq!(inventory_view["run"]["state"], "inventorying");
    assert_eq!(
        inventory_view["run"]["result"]["preparation_progress"]["bytes_hashed"],
        1_024
    );
    assert!(inventory_view["run"]["result"]["preparation_progress"]
        .get("path")
        .is_none());
    assert!(store::transition_single_tape_data_phase(
        &pool,
        &claimed,
        store::SingleTapeDataPhase::Writing,
        json!({"detail": "integration write phase"}),
    )
    .await
    .expect("write phase should persist"));
    let writing_view = api_request(
        &replay_application,
        Method::GET,
        format!("/api/v1/backup-runs/{}", final_request.id),
        Value::Null,
    )
    .await;
    let writing_view = response_json(writing_view).await;
    assert!(writing_view["run"]["result"]["preparation_progress"].is_null());
    let live_sampled_at = Utc::now();
    let live_sample = store::SingleTapeLiveIoSample {
        phase: store::SingleTapeLiveIoPhase::BackupWrite,
        sampled_at: live_sampled_at,
        sample_window_ms: Some(5_000),
        stale_after_seconds: 20,
        engine_job_id: 9_001,
        files_processed: 1,
        files_total: Some(1),
        progress_basis: "bacula_job_files_over_manifest_entries",
        archive_entries_per_second: Some(1.0),
        pipeline_bytes_processed: 1_024,
        pipeline_bytes_total: None,
        pipeline_bytes_per_second: Some(200),
        file_daemon_read_bytes_processed: Some(1_024),
        file_daemon_read_bytes_per_second: Some(200),
        current_file: Some("fixture.bin".to_owned()),
        file_daemon_basis: Some("bacula_file_daemon_read_bytes_delta"),
        tape_read_bytes_per_second: Some(0),
        tape_write_bytes_per_second: Some(180),
        tape_read_counter_available: true,
        tape_write_counter_available: true,
        tape_counters_available: true,
        pipeline_basis: "bacula_job_bytes_delta",
        tape_basis: Some("linux_scsi_tape_sysfs_counter_delta"),
        engine_state: Some("running"),
        activity: Some(store::SingleTapeLiveActivity::Streaming),
        progress_idle_ms: Some(0),
        no_progress_timeout_ms: Some(1_800_000),
        spool_bytes: Some(1_048_576),
        spool_capacity_bytes: Some(68_719_476_736),
        spool_target_bytes: Some(64_424_509_440),
        spool_file_count: Some(2),
        spool_high_water_bytes: None,
        spool_low_water_bytes: None,
        spool_pack_count: None,
        spool_reclaimed_bytes: None,
        spool_fill_bytes_per_second: None,
        spool_drain_bytes_per_second: Some(256),
        spool_basis: Some("run_scoped_bacula_spool_regular_file_bytes"),
    };
    assert!(
        store::record_single_tape_data_live_io(&pool, &claimed, &live_sample)
            .await
            .expect("the fenced live I/O sample should persist")
    );
    let regressed_sample = store::SingleTapeLiveIoSample {
        files_processed: 0,
        pipeline_bytes_processed: 128,
        ..live_sample.clone()
    };
    assert!(
        store::record_single_tape_data_live_io(&pool, &claimed, &regressed_sample)
            .await
            .expect("a transient engine-counter regression should be clamped")
    );
    let out_of_order_sample = store::SingleTapeLiveIoSample {
        sampled_at: live_sampled_at - chrono::Duration::seconds(1),
        ..live_sample.clone()
    };
    assert!(
        !store::record_single_tape_data_live_io(&pool, &claimed, &out_of_order_sample)
            .await
            .expect("an out-of-order sample should be rejected")
    );
    let invalid_sample = store::SingleTapeLiveIoSample {
        pipeline_basis: "untrusted_integration_fixture",
        ..live_sample.clone()
    };
    assert!(
        !store::record_single_tape_data_live_io(&pool, &claimed, &invalid_sample)
            .await
            .expect("an untrusted live I/O basis should be rejected")
    );
    let missing_total = store::SingleTapeLiveIoSample {
        files_total: None,
        ..live_sample.clone()
    };
    assert!(
        !store::record_single_tape_data_live_io(&pool, &claimed, &missing_total)
            .await
            .expect("a missing authoritative record total should be rejected")
    );
    let live_view = api_request(
        &replay_application,
        Method::GET,
        format!("/api/v1/backup-runs/{}", final_request.id),
        Value::Null,
    )
    .await;
    assert_eq!(live_view.status(), StatusCode::OK);
    let live_view = response_json(live_view).await;
    assert_eq!(
        live_view["run"]["result"]["live_io"]["phase"],
        "backup_write"
    );
    assert_eq!(
        live_view["run"]["result"]["live_io"]["pipeline_bytes_per_second"],
        200
    );
    assert_eq!(
        live_view["run"]["result"]["live_io"]["tape_write_bytes_per_second"],
        180
    );
    assert_eq!(
        live_view["run"]["result"]["live_io"]["activity"],
        "streaming"
    );
    assert_eq!(
        live_view["run"]["result"]["live_io"]["spool_bytes"],
        1_048_576
    );
    assert_eq!(live_view["run"]["result"]["live_io"]["spool_file_count"], 2);
    assert_eq!(
        live_view["run"]["result"]["live_io"]["pipeline_bytes_processed"],
        1_024
    );
    assert_eq!(live_view["run"]["result"]["live_io"]["files_total"], 1);
    assert_eq!(
        live_view["run"]["result"]["transfer_progress"]["phase"],
        "backup_write"
    );
    assert_eq!(
        live_view["run"]["result"]["transfer_progress"]["files_total"],
        1
    );
    let live_io = &live_view["run"]["result"]["live_io"];
    assert!(live_io.get("path").is_none());
    assert!(live_io.get("directory").is_none());
    assert!(!serde_json::to_string(live_io)
        .expect("live I/O should serialize")
        .contains("/var/lib/tapescale/spool"));
    let overrun_sample = store::SingleTapeLiveIoSample {
        files_processed: 2,
        pipeline_bytes_processed: 2_048,
        ..live_sample.clone()
    };
    assert!(
        store::record_single_tape_data_live_io(&pool, &claimed, &overrun_sample,)
            .await
            .expect("engine counters may legitimately exceed manifest totals")
    );
    let engine_volume = "TS-INTEGRATION-FINAL";
    assert!(
        store::mark_single_tape_media_write_started(&pool, &claimed, engine_volume)
            .await
            .expect("irreversible media boundary should persist")
    );
    assert!(store::append_single_tape_data_artifact(
        &pool,
        &claimed,
        "engine_write",
        "/integration/engine-write.json",
        &"b".repeat(64),
        256,
        json!({"finalized": true, "engine_volume_name": engine_volume}),
    )
    .await
    .expect("finalized engine artifact should persist"));
    assert!(store::transition_single_tape_data_phase(
        &pool,
        &claimed,
        store::SingleTapeDataPhase::Written,
        json!({"detail": "integration write finalized"}),
    )
    .await
    .expect("written phase should persist"));
    let transitioned_view = api_request(
        &replay_application,
        Method::GET,
        format!("/api/v1/backup-runs/{}", final_request.id),
        Value::Null,
    )
    .await;
    assert_eq!(transitioned_view.status(), StatusCode::OK);
    let transitioned_view = response_json(transitioned_view).await;
    assert!(transitioned_view["run"]["result"]["live_io"].is_null());
    assert_eq!(
        transitioned_view["run"]["result"]["transfer_progress"]["pipeline_bytes_processed"],
        2_048
    );
    assert_eq!(
        transitioned_view["run"]["result"]["transfer_progress"]["files_total"],
        1
    );
    assert!(
        !store::record_single_tape_data_live_io(&pool, &claimed, &live_sample)
            .await
            .expect("a stale write-phase sample should be rejected after transition")
    );
    assert!(store::transition_single_tape_data_phase(
        &pool,
        &claimed,
        store::SingleTapeDataPhase::Verifying,
        json!({"detail": "integration physical readback"}),
    )
    .await
    .expect("verify phase should persist"));
    let finalized = store::finalize_single_tape_backup_pass(
        &pool,
        &claimed,
        &store::SingleTapePassEvidence {
            engine_operation_id: "bacula-job:9001".to_owned(),
            engine_volume_name: engine_volume.to_owned(),
            engine_record_id: "bacula-media:91:job:9001".to_owned(),
            first_record: "file=0:block=0".to_owned(),
            last_record: Some("file=0:block=9".to_owned()),
            logical_bytes: 1024,
            encoded_bytes: 2048,
            file_count: 1,
            coverage: json!({"complete": true}),
            full_readback_evidence: json!({
                "physical_media_readback": true,
                "level": "Data",
                "catalog_proof": true
            }),
            restore_test_evidence: json!({"outcome": "not_run"}),
            result: json!({"integration_fixture": true}),
        },
    )
    .await
    .expect("atomic backup finalization should succeed");
    let store::FinalizeSingleTapeDataResult::Passed {
        recovery_point_id,
        volume_record_id,
    } = finalized
    else {
        panic!("complete physical-readback evidence should create a recovery point");
    };
    let finalized_request = backup_ops::get_backup_request(&pool, final_request.id)
        .await
        .expect("finalized request should reload")
        .expect("finalized request remains durable");
    assert_eq!(finalized_request.state, "verified");
    let finalized_view = api_request(
        &replay_application,
        Method::GET,
        format!("/api/v1/backup-runs/{}", final_request.id),
        Value::Null,
    )
    .await;
    assert_eq!(finalized_view.status(), StatusCode::OK);
    let finalized_view = response_json(finalized_view).await;
    assert!(finalized_view["run"]["result"]["live_io"].is_null());
    assert_eq!(
        finalized_view["run"]["result"]["transfer_progress"]["phase"],
        "backup_write"
    );
    assert_eq!(
        finalized_view["run"]["result"]["transfer_progress"]["pipeline_bytes_processed"],
        2_048
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM recovery_points WHERE id = $1 AND run_id = $2
             AND status = 'verified' AND level = 'full'",
        )
        .bind(recovery_point_id)
        .bind(final_request.job_run_id)
        .fetch_one(&pool)
        .await
        .expect("recovery point should be queryable"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM volume_records WHERE id = $1
             AND recovery_point_id = $2 AND cartridge_id = $3 AND finalized",
        )
        .bind(volume_record_id)
        .bind(recovery_point_id)
        .bind(scratch.id)
        .fetch_one(&pool)
        .await
        .expect("volume record should be queryable"),
        1
    );
    let verification_kinds = sqlx::query_scalar::<_, String>(
        "SELECT kind FROM verification_records WHERE recovery_point_id = $1 ORDER BY kind",
    )
    .bind(recovery_point_id)
    .fetch_all(&pool)
    .await
    .expect("verification records should be queryable");
    assert_eq!(verification_kinds, vec!["full_readback", "write_complete"]);
    let finalized_cartridge = store::list_media(&pool)
        .await
        .expect("media should reload")
        .into_iter()
        .find(|cartridge| cartridge.id == scratch.id)
        .expect("scratch cartridge should remain inventoried");
    assert_eq!(finalized_cartridge.health_status, "written_verified");
    assert_eq!(finalized_cartridge.reserved_by, None);
    let backup_audit_actions = sqlx::query_scalar::<_, String>(
        "SELECT action FROM audit_events
         WHERE object_type = 'single_tape_data_run' AND object_id = $1
         ORDER BY occurred_at, id",
    )
    .bind(final_run.id.to_string())
    .fetch_all(&pool)
    .await
    .expect("backup data-run audit actions should be queryable");
    assert!(!backup_audit_actions.is_empty());
    assert!(backup_audit_actions
        .iter()
        .all(|action| action.starts_with("backup.data_run.")));

    let manifest_response = api_request(
        &application,
        Method::GET,
        format!("/api/v1/recovery-points/{recovery_point_id}/files?path="),
        Value::Null,
    )
    .await;
    assert_eq!(manifest_response.status(), StatusCode::OK);
    assert_eq!(
        manifest_response.headers().get(header::CACHE_CONTROL),
        Some(&header::HeaderValue::from_static("no-store"))
    );
    let manifest_response = response_json(manifest_response).await;
    assert_eq!(manifest_response["browse_only"], false);
    assert_eq!(manifest_response["path_selective_restore_supported"], true);
    assert_eq!(
        manifest_response["selection_capabilities"]["multi_volume_selected_execution"],
        false
    );
    assert_eq!(manifest_response["current_path"], "");
    assert_eq!(manifest_response["entries"][0]["name"], "payload.bin");
    assert_eq!(manifest_response["entries"][0]["kind"], "file");
    assert_eq!(manifest_response["entries"][0]["size_bytes"], 1_024);
    assert!(manifest_response["entries"][0]
        .get("content_sha256")
        .is_none());

    // A verified operational backup must be able to establish its first
    // restore-test record. Requiring a prior restore test here would make that
    // evidence impossible to create.
    let restore_location_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO locations (id, name, kind)
         VALUES ($1, 'Operational restore integration shelf', 'onsite')",
    )
    .bind(restore_location_id)
    .execute(&pool)
    .await
    .expect("restore fixture location should persist");
    sqlx::query(
        "UPDATE cartridges SET confirmed_location_id = $1, updated_at = now() WHERE id = $2",
    )
    .bind(restore_location_id)
    .bind(scratch.id)
    .execute(&pool)
    .await
    .expect("verified backup media should have confirmed custody");
    sqlx::query(
        "UPDATE cartridges
         SET confirmed_location_id = $1, identity_status = 'reconciled',
             health_status = 'written_verified', on_tape_identity = $2,
             reserved_by = NULL, updated_at = now()
         WHERE id = $3",
    )
    .bind(restore_location_id)
    .bind(second_engine_volume)
    .bind(continuation_second.id)
    .execute(&pool)
    .await
    .expect("second restore volume should have verified identity and custody");
    sqlx::query(
        "INSERT INTO volume_records
            (id, recovery_point_id, cartridge_id, volume_order, first_record,
             last_record, encoded_bytes, finalized, engine_record_id)
         VALUES ($1, $2, $3, 2, 'bacula-file:2', 'bacula-file:2', 2048, true,
                 'bacula-media:92:job:9001')",
    )
    .bind(Uuid::now_v7())
    .bind(recovery_point_id)
    .bind(continuation_second.id)
    .execute(&pool)
    .await
    .expect("second finalized recovery volume should persist");
    // Verification history is append-only. Retain the original one-volume
    // proof and append the later ordered two-volume proof used by this restore
    // fixture instead of rewriting historical evidence.
    sqlx::query(
        "INSERT INTO verification_records
            (id, recovery_point_id, kind, scope, outcome, drive_id, media_ids,
             evidence, correlation_id, started_at, finished_at)
         SELECT $1, recovery_point_id, kind,
                scope || jsonb_build_object('volume_count', 2), outcome,
                drive_id, ARRAY[$2, $3]::uuid[], evidence, $4, now(), now()
         FROM verification_records
         WHERE recovery_point_id = $5 AND kind = 'full_readback'
           AND outcome = 'passed'
         ORDER BY started_at DESC, id DESC
         LIMIT 1",
    )
    .bind(Uuid::now_v7())
    .bind(scratch.id)
    .bind(continuation_second.id)
    .bind(Uuid::now_v7())
    .bind(recovery_point_id)
    .execute(&pool)
    .await
    .expect("full-readback proof should bind the complete ordered media set");
    sqlx::query("UPDATE drives SET last_observed_at = now() WHERE id = $1")
        .bind(drive.id)
        .execute(&pool)
        .await
        .expect("restore fixture drive observation should remain fresh");

    let restore_plan_response = api_request(
        &application,
        Method::POST,
        "/api/v1/restores/plan".to_owned(),
        json!({
            "recovery_point_id": recovery_point_id,
            "mode": "file_archive",
            "destination": "planned-restores/operational-multi-volume",
            "conflict_policy": "skip",
            "destination_approved": true,
            "is_new_destination": true,
            "acl_semantics_supported": true,
            "identity_mapping_complete": true,
            "estimated_required_bytes": 1024,
            "available_bytes": 1048576,
            "required_free_reserve_bytes": 4096,
            "automatic_publication_requested": false
        }),
    )
    .await;
    assert_eq!(restore_plan_response.status(), StatusCode::CREATED);
    let restore_plan = response_json(restore_plan_response).await;
    assert_eq!(restore_plan["plan"]["ready"], true);
    assert_eq!(restore_plan["plan"]["execution_supported"], false);
    assert_eq!(
        restore_plan["plan"]["execution"]["native_restore_pending"],
        true
    );
    assert_eq!(
        restore_plan["plan"]["execution"]["offline_recovery_utility"],
        "tapescale-recovery"
    );
    assert_eq!(
        restore_plan["plan"]["execution"]["availability"],
        "pending_native_restore"
    );
    assert_eq!(restore_plan["plan"]["execution"]["volume_count"], 2);
    assert_eq!(restore_plan["plan"]["execution"]["multi_volume"], true);
    assert_eq!(restore_plan["plan"]["selection"]["kind"], "complete_point");
    assert_eq!(
        restore_plan["plan"]["execution"]["origin_workflow_kind"],
        "backup"
    );
    assert_eq!(
        restore_plan["plan"]["verification_prerequisites"]["full_readback_passed"],
        true
    );
    assert_eq!(
        restore_plan["plan"]["verification_prerequisites"]["restore_test_passed"],
        false
    );
    assert!(restore_plan["plan"]["warnings"]
        .as_array()
        .is_some_and(|warnings| warnings
            .iter()
            .any(|warning| warning["code"] == "restore_test_not_passed")));

    let selected_plan_response = api_request(
        &application,
        Method::POST,
        "/api/v1/restores/plan".to_owned(),
        json!({
            "recovery_point_id": recovery_point_id,
            "mode": "file_archive",
            "selection": {
                "kind": "selected_paths",
                "paths": ["payload.bin"]
            },
            "destination": "planned-restores/operational-multi-volume-selection",
            "conflict_policy": "skip",
            "destination_approved": true,
            "is_new_destination": true,
            "acl_semantics_supported": true,
            "identity_mapping_complete": true,
            "estimated_required_bytes": 1024,
            "available_bytes": 1048576,
            "required_free_reserve_bytes": 4096,
            "automatic_publication_requested": false
        }),
    )
    .await;
    assert_eq!(selected_plan_response.status(), StatusCode::CREATED);
    let selected_plan = response_json(selected_plan_response).await;
    assert_eq!(selected_plan["plan"]["selection"]["kind"], "selected_paths");
    assert_eq!(selected_plan["plan"]["ready"], false);
    assert_eq!(selected_plan["plan"]["execution_supported"], false);
    assert!(selected_plan["plan"]["blockers"]
        .as_array()
        .is_some_and(|blockers| blockers.iter().any(|blocker| {
            blocker["code"] == "selected_restore_multivolume_unavailable"
                && blocker["volume_count"] == 2
        })));

    let restore_plan_id = Uuid::parse_str(
        restore_plan["plan"]["id"]
            .as_str()
            .expect("native restore plan should have an id"),
    )
    .expect("native restore plan id should parse");
    let queue_response = api_request(
        &application,
        Method::POST,
        "/api/v1/restore-rehearsals".to_owned(),
        json!({
            "plan_id": restore_plan_id,
            "preview_hash": restore_plan["plan"]["preview_hash"],
            "drive_id": drive.id,
            "confirmation": format!("RESTORE PLAN {restore_plan_id} TO ISOLATED DESTINATION"),
            "tape_loaded_confirmed": true,
            "isolated_destination_confirmed": true,
            "attended_confirmed": true,
            "actor": "Native restore integration operator"
        }),
    )
    .await;
    let queue_status = queue_response.status();
    let queue_body = response_json(queue_response).await;
    assert_eq!(
        queue_status,
        StatusCode::CONFLICT,
        "native in-console restore must remain unavailable: {queue_body}"
    );
    assert!(queue_body["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("not ready or still contains blockers")));

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("isolated integration schema should clean up");
    admin.close().await;
}
