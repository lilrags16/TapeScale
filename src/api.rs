use std::{collections::HashMap, path::PathBuf, sync::Arc};

use axum::{
    extract::{Path, State},
    http::{HeaderName, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio::sync::RwLock;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use uuid::Uuid;

use crate::{
    demo,
    domain::path::validate_archive_path,
    engine::{simulator::SimulatorEngine, TapeEngine},
    error::AppError,
    store, VERSION,
};

#[derive(Clone)]
pub struct AppState {
    pub pool: Option<PgPool>,
    pub demo_mode: bool,
    pub engine: Arc<dyn TapeEngine>,
    confirmations: Arc<RwLock<HashMap<String, Vec<MovementConfirmation>>>>,
}

impl AppState {
    #[must_use]
    pub fn new(pool: Option<PgPool>, demo_mode: bool) -> Self {
        Self {
            pool,
            demo_mode,
            engine: Arc::new(SimulatorEngine::new()),
            confirmations: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

pub fn router(state: AppState, web_dir: PathBuf) -> Router {
    let request_id = HeaderName::from_static("x-request-id");
    let api = Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/api/v1/system", get(system))
        .route("/api/v1/overview", get(overview))
        .route("/api/v1/jobs", get(jobs))
        .route("/api/v1/jobs/{id}", get(job))
        .route("/api/v1/media", get(media))
        .route("/api/v1/drives", get(drives))
        .route("/api/v1/movements", get(movements))
        .route("/api/v1/movements/{id}/confirm", post(confirm_movement))
        .route("/api/v1/restores/plan", post(plan_restore))
        .route("/api/v1/preflight", get(preflight))
        .route("/api/v1/qualification", get(qualification));

    let static_files = ServeDir::new(&web_dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(web_dir.join("index.html")));

    api.fallback_service(static_files)
        .layer(PropagateRequestIdLayer::new(request_id.clone()))
        .layer(SetRequestIdLayer::new(request_id, MakeRequestUuid))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn live() -> impl IntoResponse {
    Json(json!({"status":"alive","version":VERSION}))
}

async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let database_ready = match &state.pool {
        Some(pool) => store::ready(pool).await,
        None => state.demo_mode,
    };
    let status = if database_ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "status": if database_ready {"ready"} else {"not_ready"},
            "database": if state.pool.is_some() {"connected"} else {"demo_not_configured"},
            "mode": if state.demo_mode {"simulation"} else {"production"},
            "physical_qualification": "not_established"
        })),
    )
}

async fn system(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let identity = state
        .engine
        .identity()
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?;
    let capabilities = state
        .engine
        .capabilities()
        .await
        .map_err(|error| AppError::Internal(error.to_string()))?;
    Ok(Json(json!({
        "product":"TapeScale",
        "version":VERSION,
        "mode":if state.demo_mode{"simulation"}else{"production"},
        "backend_language":"Rust",
        "engine":identity,
        "capabilities":capabilities
    })))
}

async fn overview() -> Json<Value> {
    Json(demo::overview())
}

async fn jobs() -> Json<Value> {
    Json(demo::jobs())
}

async fn job(Path(id): Path<String>) -> Result<Json<Value>, AppError> {
    demo::job_detail(&id).map(Json).ok_or(AppError::NotFound)
}

async fn media() -> Json<Value> {
    Json(demo::media())
}

async fn drives() -> Json<Value> {
    Json(demo::drives())
}

async fn movements(State(state): State<AppState>) -> Json<Value> {
    let mut response = demo::movements();
    let confirmations = state.confirmations.read().await;
    if let Some(tasks) = response["tasks"].as_array_mut() {
        for task in tasks {
            if let Some(recorded) = confirmations.get(task["id"].as_str().unwrap_or_default()) {
                let newly_confirmed: std::collections::HashSet<&str> = recorded
                    .iter()
                    .flat_map(|confirmation| confirmation.tape_ids.iter().map(String::as_str))
                    .collect();
                let mut confirmed_count = 0_u64;
                if let Some(tapes) = task["tapes"].as_array_mut() {
                    for tape in tapes {
                        if tape["confirmed"].as_bool().unwrap_or(false)
                            || tape["id"]
                                .as_str()
                                .is_some_and(|id| newly_confirmed.contains(id))
                        {
                            tape["confirmed"] = json!(true);
                            confirmed_count += 1;
                        }
                    }
                }
                let total = task["total_count"].as_u64().unwrap_or(0);
                let complete = total > 0 && confirmed_count == total;
                task["state"] = json!(if complete {
                    "confirmed"
                } else {
                    "partially_confirmed"
                });
                task["state_label"] = json!(if complete {
                    "Arrival confirmed"
                } else {
                    "Partial arrival · incomplete set"
                });
                task["confirmations"] = serde_json::to_value(recorded).unwrap_or(Value::Null);
                task["confirmed_count"] = json!(confirmed_count);
            }
        }
    }
    Json(response)
}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
struct MovementConfirmation {
    actor: String,
    destination: String,
    tape_ids: Vec<String>,
    #[serde(default)]
    note: String,
    #[serde(default = "Uuid::now_v7")]
    idempotency_key: Uuid,
    #[serde(default = "chrono::Utc::now")]
    confirmed_at: chrono::DateTime<chrono::Utc>,
}

async fn confirm_movement(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(confirmation): Json<MovementConfirmation>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if confirmation.actor.trim().is_empty() || confirmation.destination.trim().is_empty() {
        return Err(AppError::Invalid(
            "actor and destination are required".to_owned(),
        ));
    }
    if confirmation.tape_ids.is_empty() {
        return Err(AppError::Blocked(
            "custody cannot advance without physically confirmed tape identities".to_owned(),
        ));
    }
    if id != "move-weekly-45" && id != "move-recall-l7m" {
        return Err(AppError::NotFound);
    }

    let mut confirmations = state.confirmations.write().await;
    let recorded = confirmations.entry(id.clone()).or_default();
    if let Some(existing) = recorded
        .iter()
        .find(|existing| existing.idempotency_key == confirmation.idempotency_key)
    {
        return Ok((
            StatusCode::OK,
            Json(json!({"status":"already_confirmed","task_id":id,"confirmation":existing})),
        ));
    }

    if let Some(pool) = &state.pool {
        sqlx::query(
            "INSERT INTO custody_events (id, movement_task_external_id, actor_label, destination_label, tape_ids, note, occurred_at, correlation_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(Uuid::now_v7())
        .bind(&id)
        .bind(&confirmation.actor)
        .bind(&confirmation.destination)
        .bind(&confirmation.tape_ids)
        .bind(&confirmation.note)
        .bind(confirmation.confirmed_at)
        .bind(confirmation.idempotency_key)
        .execute(pool)
        .await?;
    }
    recorded.push(confirmation.clone());
    let required: &[&str] = match id.as_str() {
        "move-weekly-45" => &["TS-L9-004216"],
        "move-recall-l7m" => &["TS-L7M-000112", "TS-L7M-000113"],
        _ => &[],
    };
    let confirmed: std::collections::HashSet<&str> = recorded
        .iter()
        .flat_map(|item| item.tape_ids.iter().map(String::as_str))
        .collect();
    let complete = required.iter().all(|tape_id| confirmed.contains(tape_id));
    let status = if complete {
        StatusCode::CREATED
    } else {
        StatusCode::ACCEPTED
    };
    let durability = if state.pool.is_some() {
        "postgresql"
    } else {
        "simulation_memory"
    };
    Ok((
        status,
        Json(json!({
            "status":if complete{"confirmed"}else{"partially_confirmed"},
            "task_id":id,
            "confirmation":confirmation,
            "remaining_tape_ids":required.iter().filter(|tape_id| !confirmed.contains(**tape_id)).collect::<Vec<_>>(),
            "audit":if state.pool.is_some(){"appended"}else{"simulated"},
            "durability":durability,
            "physical_custody_evidence":false
        })),
    ))
}

#[derive(Debug, Deserialize)]
struct RestorePlanRequest {
    #[serde(default)]
    destination: Option<String>,
    #[serde(default)]
    key_available: bool,
}

async fn plan_restore(Json(request): Json<RestorePlanRequest>) -> Result<Json<Value>, AppError> {
    if let Some(destination) = &request.destination {
        if validate_archive_path(std::path::Path::new(destination)).is_err() {
            return Err(AppError::Invalid(
                "destination must be a contained dataset-relative path".to_owned(),
            ));
        }
    }
    Ok(Json(demo::restore_plan(
        request.destination.as_deref(),
        request.key_available,
    )))
}

async fn preflight(State(state): State<AppState>) -> Json<Value> {
    Json(demo::preflight(state.pool.is_some()))
}

async fn qualification() -> Json<Value> {
    Json(demo::qualification())
}
