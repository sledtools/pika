mod config;
mod manager;
mod models;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use tracing::{error, info};

use config::Config;
use manager::VmManager;
use models::{CapacityResponse, CreateVmRequest, PersistedVm, VmResponse};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = Config::from_env()?;
    let manager = Arc::new(VmManager::new(config.clone()).await?);

    if let Err(err) = manager.prewarm_defaults_if_enabled().await {
        error!(error = %err, "vm-spawner prewarm failed");
    }

    let reaper_manager = manager.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            if let Err(err) = reaper_manager.reap_expired().await {
                error!(error = %err, "ttl reaper failed");
            }
        }
    });

    let health_manager = manager.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            match health_manager.list().await.len() {
                count => info!(vm_count = count, "vm-spawner health tick"),
            }
        }
    });

    let app = Router::new()
        .route("/healthz", get(health))
        .route("/vms", post(create_vm).get(list_vms))
        .route("/vms/:id", get(get_vm).delete(delete_vm))
        .route("/vms/:id/exec", post(exec_vm))
        .route("/capacity", get(capacity))
        .with_state(manager.clone());

    info!(bind = %config.bind, "vm-spawner starting");

    axum::Server::bind(&config.bind)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

#[derive(serde::Serialize)]
struct HealthResponse {
    ok: bool,
    now: chrono::DateTime<Utc>,
    vm_count: usize,
}

async fn health(State(manager): State<Arc<VmManager>>) -> Result<Json<HealthResponse>, ApiError> {
    let vms = manager.list().await;
    Ok(Json(HealthResponse {
        ok: true,
        now: Utc::now(),
        vm_count: vms.len(),
    }))
}

async fn create_vm(
    State(manager): State<Arc<VmManager>>,
    Json(payload): Json<CreateVmRequest>,
) -> Result<Json<VmResponse>, ApiError> {
    let vm = manager.create(payload).await?;
    Ok(Json(vm))
}

async fn list_vms(
    State(manager): State<Arc<VmManager>>,
) -> Result<Json<Vec<PersistedVm>>, ApiError> {
    Ok(Json(manager.list().await))
}

async fn get_vm(
    State(manager): State<Arc<VmManager>>,
    Path(id): Path<String>,
) -> Result<Json<PersistedVm>, ApiError> {
    match manager.get(&id).await {
        Some(vm) => Ok(Json(vm)),
        None => Err(ApiError::not_found(format!("vm not found: {id}"))),
    }
}

async fn delete_vm(
    State(manager): State<Arc<VmManager>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    manager.destroy(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Serialize)]
struct ExecResponse {
    error: &'static str,
}

async fn exec_vm(
    State(_manager): State<Arc<VmManager>>,
    Path(_id): Path<String>,
) -> Result<(StatusCode, Json<ExecResponse>), ApiError> {
    Ok((
        StatusCode::NOT_IMPLEMENTED,
        Json(ExecResponse {
            error: "use SSH for v1; /exec websocket not implemented",
        }),
    ))
}

async fn capacity(
    State(manager): State<Arc<VmManager>>,
) -> Result<Json<CapacityResponse>, ApiError> {
    Ok(Json(manager.capacity().await?))
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(value: anyhow::Error) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, value.to_string())
    }
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let body = Json(serde_json::json!({
            "error": self.message,
        }));
        (self.status, body).into_response()
    }
}
