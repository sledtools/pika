mod config;
mod manager;
mod models;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderName, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::Utc;
use tracing::{error, info, warn};
use uuid::Uuid;

use config::Config;
use manager::VmManager;
use models::{CapacityResponse, CreateVmRequest, PersistedVm, VmResponse};

const REQUEST_ID_HEADER: &str = "x-request-id";

#[derive(Clone, Debug)]
struct RequestContext {
    request_id: String,
}

fn request_id_from_headers<B>(request: &Request<B>) -> Option<String> {
    request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn generate_request_id() -> String {
    Uuid::new_v4().simple().to_string()
}

async fn trace_http_request<B>(mut request: Request<B>, next: Next<B>) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let request_id = request_id_from_headers(&request).unwrap_or_else(generate_request_id);
    let request_id_header = HeaderName::from_static(REQUEST_ID_HEADER);
    let request_id_value =
        HeaderValue::from_str(&request_id).expect("generated request id must be a valid header");
    request
        .headers_mut()
        .insert(request_id_header.clone(), request_id_value.clone());
    request.extensions_mut().insert(RequestContext {
        request_id: request_id.clone(),
    });

    let started_at = std::time::Instant::now();
    let mut response = next.run(request).await;
    let status = response.status();
    let latency_ms = started_at.elapsed().as_millis() as u64;
    response
        .headers_mut()
        .insert(request_id_header, request_id_value);
    info!(
        request_id = %request_id,
        method = %method,
        path = %path,
        status = status.as_u16(),
        latency_ms,
        "http request"
    );
    response
}

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
            let count = health_manager.list().await.len();
            info!(vm_count = count, "vm-spawner health tick");
        }
    });

    let app = Router::new()
        .route("/healthz", get(health))
        .route("/vms", post(create_vm).get(list_vms))
        .route("/vms/:id", get(get_vm).delete(delete_vm))
        .route("/vms/:id/recover", post(recover_vm))
        .route("/vms/:id/exec", post(exec_vm))
        .route("/capacity", get(capacity))
        .layer(middleware::from_fn(trace_http_request))
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
    Extension(request_context): Extension<RequestContext>,
    Json(payload): Json<CreateVmRequest>,
) -> Result<Json<VmResponse>, ApiError> {
    let vm = manager
        .create(payload)
        .await
        .map_err(|err| ApiError::internal(&request_context, err))?;
    Ok(Json(vm))
}

async fn list_vms(
    State(manager): State<Arc<VmManager>>,
    Extension(_request_context): Extension<RequestContext>,
) -> Result<Json<Vec<PersistedVm>>, ApiError> {
    Ok(Json(manager.list().await))
}

async fn get_vm(
    State(manager): State<Arc<VmManager>>,
    Extension(request_context): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<PersistedVm>, ApiError> {
    match manager.get(&id).await {
        Some(vm) => Ok(Json(vm)),
        None => Err(ApiError::not_found(
            &request_context,
            format!("vm not found: {id}"),
        )),
    }
}

async fn delete_vm(
    State(manager): State<Arc<VmManager>>,
    Extension(request_context): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    manager
        .destroy(&id)
        .await
        .map_err(|err| ApiError::internal(&request_context, err))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn recover_vm(
    State(manager): State<Arc<VmManager>>,
    Extension(request_context): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<VmResponse>, ApiError> {
    let vm = manager
        .recover(&id)
        .await
        .map_err(|err| ApiError::internal(&request_context, err))?;
    Ok(Json(vm))
}

#[derive(serde::Serialize)]
struct ExecResponse {
    error: &'static str,
}

async fn exec_vm(
    State(_manager): State<Arc<VmManager>>,
    Extension(_request_context): Extension<RequestContext>,
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
    Extension(request_context): Extension<RequestContext>,
) -> Result<Json<CapacityResponse>, ApiError> {
    Ok(Json(manager.capacity().await.map_err(|err| {
        ApiError::internal(&request_context, err)
    })?))
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
    request_id: String,
}

impl ApiError {
    fn new(status: StatusCode, request_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            request_id: request_id.into(),
        }
    }

    fn not_found(request_context: &RequestContext, message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            request_context.request_id.clone(),
            message,
        )
    }

    fn internal(request_context: &RequestContext, err: anyhow::Error) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            request_context.request_id.clone(),
            err.to_string(),
        )
    }
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        if self.status.is_server_error() {
            error!(
                request_id = %self.request_id,
                status = self.status.as_u16(),
                error = %self.message,
                "vm-spawner request failed"
            );
        } else {
            warn!(
                request_id = %self.request_id,
                status = self.status.as_u16(),
                error = %self.message,
                "vm-spawner request failed"
            );
        }
        let body = Json(serde_json::json!({
            "error": self.message,
            "request_id": self.request_id,
        }));
        (self.status, body).into_response()
    }
}
