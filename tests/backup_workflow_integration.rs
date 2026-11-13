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
    backup_ops::create_backup_profile(pool, &request)
        .await
        .expect("full-file backup profile should persist")
}

async fn create_loaded_drive(pool: &PgPool) -> store::DriveRecord {
    let observation = SasDriveObservation {
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
    };
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
            "engine_version": "15.0.3"
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
    assert!(!manual_profile.production_eligible);

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
    empty_pool_request.actor_label = "Backup integration operator".to_owned();
    let empty_pool_profile = backup_ops::create_backup_profile(&pool, &empty_pool_request)
        .await
        .expect("fixed-pool backup profile should persist");
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
            "detail": "backup integration exact qualification",
            "position_uncertain": false,
            "engine_version": "15.0.3"
        }),
    )
    .await
    .expect("qualification pass should persist"));

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
    assert_eq!(candidate.qualification_run_id, qualification.id);
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
    assert_eq!(assignment.resolved_compression, "gzip");
    assert!(assignment.spanning_enabled);
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
    let claimed = store::claim_next_single_tape_data_run(&pool, Uuid::now_v7())
        .await
        .expect("backup worker claim should succeed");
    let store::ClaimSingleTapeDataRunResult::Claimed(claimed) = claimed else {
        panic!("the exact qualified backup should be claimable");
    };
    assert_eq!(claimed.run.id, final_run.id);
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
    assert!(store::transition_single_tape_data_phase(
        &pool,
        &claimed,
        store::SingleTapeDataPhase::Writing,
        json!({"detail": "integration write phase"}),
    )
    .await
    .expect("write phase should persist"));
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
    assert_eq!(manifest_response["browse_only"], true);
    assert_eq!(manifest_response["path_selective_restore_supported"], false);
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
            "destination": "planned-restores/operational-single-volume",
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
    assert_eq!(restore_plan["plan"]["execution_supported"], true);
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

    let restore_plan_id = Uuid::parse_str(
        restore_plan["plan"]["id"]
            .as_str()
            .expect("restore plan should have an id"),
    )
    .expect("restore plan id should parse");
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
            "actor": "Operational restore integration operator"
        }),
    )
    .await;
    let queue_status = queue_response.status();
    let queued_restore = response_json(queue_response).await;
    assert_eq!(
        queue_status,
        StatusCode::ACCEPTED,
        "operational restore queue response: {queued_restore}"
    );
    let restore_rehearsal_id = Uuid::parse_str(
        queued_restore["rehearsal"]["id"]
            .as_str()
            .expect("queued operational restore should have an id"),
    )
    .expect("restore rehearsal id should parse");

    let restore_claim = store::claim_next_restore_rehearsal_run(&pool, Uuid::now_v7())
        .await
        .expect("operational restore rehearsal claim should query");
    let store::ClaimRestoreRehearsalRunResult::Claimed(restore_claim) = restore_claim else {
        panic!("verified operational single-volume restore should be claimable");
    };
    assert_eq!(restore_claim.run.id, restore_rehearsal_id);
    assert!(store::transition_restore_rehearsal_phase(
        &pool,
        &restore_claim,
        store::RestoreRehearsalPhase::Restoring,
        json!({"detail": "operational integration restore started"}),
    )
    .await
    .expect("operational restore should enter restoring"));
    let restore_job_id = 9002_i64;
    let restore_destination =
        format!("/restore/planned-restores/operational-single-volume/{restore_rehearsal_id}");
    assert!(store::append_restore_rehearsal_artifact(
        &pool,
        &restore_claim,
        "engine_restore",
        "/integration/operational-engine-restore.json",
        &"c".repeat(64),
        512,
        json!({
            "completed": true,
            "finalized": true,
            "read_only_media_operation": true,
            "physical_media_readback": true,
            "backup_job_id": 9001,
            "restore_job_id": restore_job_id,
            "engine_media_id": 91,
            "media_id": 91,
            "volume": engine_volume,
            "destination": restore_destination
        }),
    )
    .await
    .expect("operational engine restore evidence should append"));
    assert!(store::transition_restore_rehearsal_phase(
        &pool,
        &restore_claim,
        store::RestoreRehearsalPhase::Comparing,
        json!({"detail": "operational integration restore hashing complete"}),
    )
    .await
    .expect("operational restore should enter comparing"));
    let restored_artifact_sha256 = "d".repeat(64);
    for (kind, storage_ref, sha256, metadata) in [
        (
            "restored_manifest",
            "/integration/operational-restored-manifest.json",
            restored_artifact_sha256.clone(),
            json!({
                "manifest_digest": source_manifest.digest_sha256,
                "entry_count": source_manifest.entry_count,
                "file_count": source_manifest.file_count,
                "logical_bytes": source_manifest.logical_bytes
            }),
        ),
        (
            "comparison",
            "/integration/operational-comparison.json",
            "e".repeat(64),
            json!({
                "matched": true,
                "expected_digest": source_manifest.digest_sha256,
                "actual_digest": source_manifest.digest_sha256,
                "differences": 0
            }),
        ),
        (
            "evidence_report",
            "/integration/operational-restore-report.json",
            "f".repeat(64),
            json!({
                "complete": true,
                "matched": true,
                "read_only_media_operation": true,
                "source_manifest_digest": source_manifest.digest_sha256,
                "restored_manifest_digest": source_manifest.digest_sha256
            }),
        ),
    ] {
        assert!(store::append_restore_rehearsal_artifact(
            &pool,
            &restore_claim,
            kind,
            storage_ref,
            &sha256,
            256,
            metadata,
        )
        .await
        .expect("operational restore artifact should append"));
    }
    let finalized_restore = store::finalize_restore_rehearsal_pass(
        &pool,
        &restore_claim,
        json!({
            "scope": "integration attended operational single-volume restore",
            "production_eligible": false,
            "media_operation": "read_only",
            "source_backup_job_id": 9001,
            "engine_operation_id": format!("bacula-job:{restore_job_id}"),
            "restore_job_id": restore_job_id,
            "restore_job": {"job_id": restore_job_id, "job_status": "T", "job_errors": 0},
            "restore_catalog": {
                "source_backup_job_id": 9001,
                "job": {"job_id": restore_job_id}
            },
            "source_manifest_digest": source_manifest.digest_sha256,
            "restored_manifest_digest": source_manifest.digest_sha256,
            "restored_manifest_sha256": restored_artifact_sha256,
            "comparison": {
                "matched": true,
                "expected_digest": source_manifest.digest_sha256,
                "actual_digest": source_manifest.digest_sha256,
                "differences": []
            },
            "restore_destination": restore_destination,
            "cleanup": {"postcondition": "ready_at_bot", "position_uncertain": false},
            "media_mutation_commands_dispatched": false
        }),
    )
    .await
    .expect("operational restore evidence should finalize");
    let store::FinalizeRestoreRehearsalResult::Passed {
        verification_record_id,
    } = finalized_restore
    else {
        panic!("complete operational restore evidence should pass");
    };
    let (restore_scope, pass_result) = sqlx::query_as::<_, (Value, Value)>(
        "SELECT verification.scope, rehearsal.result
         FROM verification_records verification
         JOIN restore_rehearsals rehearsal ON rehearsal.id = $2
         WHERE verification.id = $1
           AND verification.kind = 'restore_test' AND verification.outcome = 'passed'",
    )
    .bind(verification_record_id)
    .bind(restore_rehearsal_id)
    .fetch_one(&pool)
    .await
    .expect("operational restore-test verification should persist");
    assert_eq!(
        restore_scope["scope"],
        "complete attended single-volume file recovery replay"
    );
    assert_eq!(
        pass_result["execution_scope"],
        "attended_single_volume_file_restore"
    );

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("isolated integration schema should clean up");
    admin.close().await;
}
