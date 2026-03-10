mod config;
mod manager;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Extension, Path, State};
use axum::http::{HeaderName, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use tracing::{error, info, warn};
use uuid::Uuid;

use config::Config;
use manager::{VmManager, VmNotFound};
use pika_agent_control_plane::{
    SpawnerCreateVmRequest as CreateVmRequest, SpawnerVmResponse as VmResponse,
};

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
    let latency_ms = started_at.elapsed().as_millis() as u64;
    let status = response.status();
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

    let health_manager = manager.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let count = health_manager.vm_count().await;
            info!(vm_count = count, "vm-spawner health tick");
        }
    });

    let app = Router::new()
        .route("/healthz", get(health))
        .route("/vms", post(create_vm))
        .route("/vms/:id", get(get_vm).delete(delete_vm))
        .route("/vms/:id/recover", post(recover_vm))
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
    Ok(Json(HealthResponse {
        ok: true,
        now: Utc::now(),
        vm_count: manager.vm_count().await,
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
        .map_err(|err| ApiError::from(err).with_request_id(request_context.request_id))?;
    Ok(Json(vm))
}

async fn delete_vm(
    State(manager): State<Arc<VmManager>>,
    Extension(request_context): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    validate_vm_id(&id).map_err(|err| err.with_request_id(request_context.request_id.clone()))?;
    manager
        .destroy(&id)
        .await
        .map_err(map_manager_error_to_api)
        .map_err(|err| err.with_request_id(request_context.request_id))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_vm(
    State(manager): State<Arc<VmManager>>,
    Extension(request_context): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<VmResponse>, ApiError> {
    validate_vm_id(&id).map_err(|err| err.with_request_id(request_context.request_id.clone()))?;
    let vm = manager
        .status(&id)
        .await
        .map_err(map_manager_error_to_api)
        .map_err(|err| err.with_request_id(request_context.request_id))?;
    Ok(Json(vm))
}

async fn recover_vm(
    State(manager): State<Arc<VmManager>>,
    Extension(request_context): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<VmResponse>, ApiError> {
    validate_vm_id(&id).map_err(|err| err.with_request_id(request_context.request_id.clone()))?;
    let vm = manager
        .recover(&id)
        .await
        .map_err(map_manager_error_to_api)
        .map_err(|err| err.with_request_id(request_context.request_id))?;
    Ok(Json(vm))
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
    log_detail: Option<String>,
    request_id: Option<String>,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            log_detail: None,
            request_id: None,
        }
    }

    fn internal(detail: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal server error".to_string(),
            log_detail: Some(detail.into()),
            request_id: None,
        }
    }

    fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(value: anyhow::Error) -> Self {
        Self::internal(value.to_string())
    }
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        if let Some(request_id) = self.request_id.as_deref() {
            if self.status.is_server_error() {
                error!(
                    request_id,
                    status = self.status.as_u16(),
                    error = %self.log_detail.as_deref().unwrap_or(&self.message),
                    "vm-spawner request failed"
                );
            } else {
                warn!(
                    request_id,
                    status = self.status.as_u16(),
                    error = %self.log_detail.as_deref().unwrap_or(&self.message),
                    "vm-spawner request failed"
                );
            }
        }
        let body = Json(serde_json::json!({
            "error": self.message,
            "request_id": self.request_id,
        }));
        (self.status, body).into_response()
    }
}

fn validate_vm_id(id: &str) -> Result<(), ApiError> {
    let valid = !id.is_empty()
        && id != "."
        && id.len() <= 128
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        && !id.contains("..");
    if valid {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("invalid vm_id: {id}"),
        ))
    }
}

fn map_manager_error_to_api(err: anyhow::Error) -> ApiError {
    if let Some(not_found) = err.downcast_ref::<VmNotFound>() {
        return ApiError::new(StatusCode::NOT_FOUND, not_found.to_string());
    }
    ApiError::internal(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::HttpBody;
    use axum::response::IntoResponse;

    #[test]
    fn validate_vm_id_rejects_malformed_values() {
        let err = validate_vm_id("../escape").unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);

        let err = validate_vm_id("vm with space").unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);

        let err = validate_vm_id(".").unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn map_manager_error_maps_not_found_to_404() {
        let err = map_manager_error_to_api(anyhow::Error::new(VmNotFound {
            id: "vm-123".to_string(),
        }));
        assert_eq!(err.status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn map_manager_error_keeps_internal_failures_as_500() {
        let err = map_manager_error_to_api(anyhow::anyhow!("restart microvm service failed"));
        assert_eq!(err.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(err.message, "internal server error");
    }

    #[tokio::test]
    async fn api_error_response_includes_request_id() {
        let response = ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "boom")
            .with_request_id("req-123")
            .into_response();
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(chunk_result) = body.data().await {
            let chunk = chunk_result.expect("read response chunk");
            bytes.extend_from_slice(chunk.as_ref());
        }
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("parse error body");
        assert_eq!(json["error"], "boom");
        assert_eq!(json["request_id"], "req-123");
    }

    #[tokio::test]
    async fn internal_error_response_redacts_raw_details() {
        let response = ApiError::from(anyhow::anyhow!("systemctl failed for /nix/store/secret"))
            .with_request_id("req-456")
            .into_response();
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(chunk_result) = body.data().await {
            let chunk = chunk_result.expect("read response chunk");
            bytes.extend_from_slice(chunk.as_ref());
        }
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("parse error body");
        assert_eq!(json["error"], "internal server error");
        assert_eq!(json["request_id"], "req-456");
    }
}
