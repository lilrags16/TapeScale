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

#[tokio::test]
async fn operational_backup_requests_schedule_and_assign_media_durably() {
    let Some((pool, admin, schema)) = isolated_database().await else {
        return;
    };

    let manual_profile =
        create_backup_profile(&pool, "Manual backup integration", "manual-source", false).await;
    assert_eq!(manual_profile.representation, "file");
    assert_eq!(manual_profile.verification_mode, "full_readback");
    assert!(!manual_profile.production_eligible);

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
    let assigned_request = backup_ops::get_backup_request(&pool, assignment_request.id)
        .await
        .expect("assigned request should reload")
        .expect("assigned request remains durable");
    assert_eq!(assigned_request.state, "queued");
    assert_eq!(assigned_request.assigned_data_run_id, Some(assignment.id));
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

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("isolated integration schema should clean up");
    admin.close().await;
}
