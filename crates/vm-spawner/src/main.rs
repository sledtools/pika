mod config;
mod manager;
mod models;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::Utc;
use std::sync::Arc;
use tracing::{error, info};

use config::Config;
use manager::{InvalidVmIdError, VmManager, VmNotFoundError};
use models::{CreateVmRequest, VmResponse};

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

    let app = Router::new()
        .route("/healthz", get(health))
        .route("/vms", post(create_vm))
        .route("/vms/:id/recover", post(recover_vm))
        .route("/vms/:id", delete(delete_vm))
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
}

async fn health(State(_manager): State<Arc<VmManager>>) -> Result<Json<HealthResponse>, ApiError> {
    Ok(Json(HealthResponse {
        ok: true,
        now: Utc::now(),
    }))
}

async fn create_vm(
    State(manager): State<Arc<VmManager>>,
    Json(payload): Json<CreateVmRequest>,
) -> Result<Json<VmResponse>, ApiError> {
    let vm = manager.create(payload).await?;
    Ok(Json(vm))
}

async fn delete_vm(
    State(manager): State<Arc<VmManager>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    manager.destroy(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn recover_vm(
    State(manager): State<Arc<VmManager>>,
    Path(id): Path<String>,
) -> Result<Json<VmResponse>, ApiError> {
    let vm = manager.recover(&id).await?;
    Ok(Json(vm))
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
}

impl From<anyhow::Error> for ApiError {
    fn from(value: anyhow::Error) -> Self {
        let status = if value.downcast_ref::<InvalidVmIdError>().is_some() {
            StatusCode::BAD_REQUEST
        } else if value.downcast_ref::<VmNotFoundError>().is_some() {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        Self::new(status, value.to_string())
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
