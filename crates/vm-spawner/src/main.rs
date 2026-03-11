mod config;
mod manager;

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes, HttpBody};
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, FromRequestParts, Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as TungsteniteCloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame as TungsteniteCloseFrame;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tracing::{error, info, warn};
use uuid::Uuid;

use config::Config;
use manager::{VmManager, VmNotFound};
use pika_agent_control_plane::{
    SpawnerCreateVmRequest as CreateVmRequest, SpawnerVmBackupStatus,
    SpawnerVmResponse as VmResponse,
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
        .route("/vms/:id/restore", post(restore_vm))
        .route("/vms/:id/backup-status", get(get_vm_backup_status))
        .route("/vms/:id/openclaw", any(proxy_openclaw_root))
        .route("/vms/:id/openclaw/*path", any(proxy_openclaw_path))
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

async fn restore_vm(
    State(manager): State<Arc<VmManager>>,
    Extension(request_context): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<VmResponse>, ApiError> {
    validate_vm_id(&id).map_err(|err| err.with_request_id(request_context.request_id.clone()))?;
    let vm = manager
        .restore(&id)
        .await
        .map_err(map_manager_error_to_api)
        .map_err(|err| err.with_request_id(request_context.request_id))?;
    Ok(Json(vm))
}

async fn get_vm_backup_status(
    State(manager): State<Arc<VmManager>>,
    Extension(request_context): Extension<RequestContext>,
    Path(id): Path<String>,
) -> Result<Json<SpawnerVmBackupStatus>, ApiError> {
    validate_vm_id(&id).map_err(|err| err.with_request_id(request_context.request_id.clone()))?;
    let status = manager
        .backup_status(&id)
        .map_err(map_manager_error_to_api)
        .map_err(|err| err.with_request_id(request_context.request_id))?;
    Ok(Json(status))
}

async fn proxy_openclaw_root(
    State(manager): State<Arc<VmManager>>,
    Extension(request_context): Extension<RequestContext>,
    Path(id): Path<String>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    proxy_openclaw_request(
        manager,
        OpenClawProxyRequest {
            request_context,
            id,
            upstream_path: "/".to_string(),
            request,
        },
    )
    .await
}

async fn proxy_openclaw_path(
    State(manager): State<Arc<VmManager>>,
    Extension(request_context): Extension<RequestContext>,
    Path((id, path)): Path<(String, String)>,
    request: Request<Body>,
) -> Result<Response, ApiError> {
    proxy_openclaw_request(
        manager,
        OpenClawProxyRequest {
            request_context,
            id,
            upstream_path: format!("/{}", path),
            request,
        },
    )
    .await
}

struct OpenClawProxyRequest {
    request_context: RequestContext,
    id: String,
    upstream_path: String,
    request: Request<Body>,
}

async fn proxy_openclaw_request(
    manager: Arc<VmManager>,
    request: OpenClawProxyRequest,
) -> Result<Response, ApiError> {
    let OpenClawProxyRequest {
        request_context,
        id,
        upstream_path,
        request,
    } = request;
    let method = request.method().clone();
    let uri = request.uri().clone();
    let headers = request.headers().clone();
    validate_vm_id(&id).map_err(|err| err.with_request_id(request_context.request_id.clone()))?;
    let base_url = manager
        .openclaw_proxy_target(&id)
        .map_err(ApiError::from)
        .map_err(|err| err.with_request_id(request_context.request_id.clone()))?;
    let upstream_url = if let Some(query) = uri.query() {
        format!("{base_url}{upstream_path}?{query}")
    } else {
        format!("{base_url}{upstream_path}")
    };

    if request_is_websocket_upgrade(&method, &headers) {
        let upstream_ws_url = websocket_proxy_url(&upstream_url).map_err(|err| {
            ApiError::internal(format!(
                "proxy openclaw websocket request for vm {id} failed: {err}"
            ))
            .with_request_id(request_context.request_id.clone())
        })?;
        let upstream_request =
            build_websocket_proxy_request(&upstream_ws_url, &headers).map_err(|err| {
                ApiError::internal(format!(
                    "proxy openclaw websocket request for vm {id} failed: {err}"
                ))
                .with_request_id(request_context.request_id.clone())
            })?;
        let (mut parts, _body) = request.into_parts();
        let websocket = WebSocketUpgrade::from_request_parts(&mut parts, &())
            .await
            .map_err(|err| {
                ApiError::new(StatusCode::BAD_REQUEST, err.to_string())
                    .with_request_id(request_context.request_id.clone())
            })?;
        let request_id = request_context.request_id.clone();
        let vm_id = id.clone();
        return Ok(websocket.on_upgrade(move |socket| async move {
            if let Err(err) = proxy_openclaw_websocket(socket, upstream_request).await {
                warn!(
                    request_id = %request_id,
                    vm_id = %vm_id,
                    error = %err,
                    "openclaw websocket proxy ended with error"
                );
            }
        }));
    }

    let body = read_request_body(request.into_body())
        .await
        .map_err(|err| err.with_request_id(request_context.request_id.clone()))?;
    let client = reqwest::Client::new();
    let upstream_method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).map_err(|err| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("invalid proxied method: {err}"),
            )
            .with_request_id(request_context.request_id.clone())
        })?;
    let mut upstream = client.request(upstream_method, &upstream_url);
    for (name, value) in forwardable_request_headers(&headers) {
        let reqwest_name = reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes())
            .map_err(|err| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    format!("invalid proxied header name: {err}"),
                )
                .with_request_id(request_context.request_id.clone())
            })?;
        let reqwest_value =
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()).map_err(|err| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    format!("invalid proxied header value: {err}"),
                )
                .with_request_id(request_context.request_id.clone())
            })?;
        upstream = upstream.header(reqwest_name, reqwest_value);
    }
    if !body.is_empty() {
        upstream = upstream.body(body.to_vec());
    }
    let upstream = upstream.send().await.map_err(|err| {
        ApiError::internal(format!("proxy openclaw request for vm {id} failed: {err}"))
            .with_request_id(request_context.request_id.clone())
    })?;

    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let response_headers = upstream.headers().clone();
    let response_body = upstream.bytes().await.map_err(|err| {
        ApiError::internal(format!(
            "read proxied openclaw response for vm {id} failed: {err}"
        ))
        .with_request_id(request_context.request_id.clone())
    })?;

    let mut response = (status, response_body).into_response();
    copy_response_headers(response.headers_mut(), &response_headers, false);
    Ok(response)
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

fn header_is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

fn forwardable_request_headers(headers: &HeaderMap) -> Vec<(&HeaderName, &HeaderValue)> {
    headers
        .iter()
        .filter(|(name, _)| {
            let name = name.as_str();
            !header_is_hop_by_hop(name) && name != "host" && name != "content-length"
        })
        .collect()
}

fn request_is_websocket_upgrade(method: &Method, headers: &HeaderMap) -> bool {
    method == Method::GET
        && headers
            .get(axum::http::header::CONNECTION)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.to_ascii_lowercase().contains("upgrade"))
            .unwrap_or(false)
        && headers
            .get(axum::http::header::UPGRADE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.eq_ignore_ascii_case("websocket"))
            .unwrap_or(false)
}

fn websocket_proxy_header_should_forward(name: &str) -> bool {
    matches!(
        name,
        "origin"
            | "user-agent"
            | "sec-websocket-protocol"
            | "x-forwarded-for"
            | "x-forwarded-host"
            | "x-forwarded-proto"
            | "x-real-ip"
    )
}

fn build_websocket_proxy_request(
    upstream_url: &str,
    downstream_headers: &HeaderMap,
) -> anyhow::Result<tokio_tungstenite::tungstenite::http::Request<()>> {
    let mut request = upstream_url.into_client_request()?;
    for (name, value) in downstream_headers {
        if websocket_proxy_header_should_forward(name.as_str()) {
            let request_name =
                reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes())?;
            let request_value = reqwest::header::HeaderValue::from_bytes(value.as_bytes())?;
            request.headers_mut().insert(request_name, request_value);
        }
    }
    Ok(request)
}

fn websocket_proxy_url(upstream_url: &str) -> anyhow::Result<String> {
    let mut url = reqwest::Url::parse(upstream_url)?;
    match url.scheme() {
        "http" => url.set_scheme("ws").expect("replace http scheme with ws"),
        "https" => url
            .set_scheme("wss")
            .expect("replace https scheme with wss"),
        "ws" | "wss" => {}
        other => anyhow::bail!("unsupported websocket upstream scheme: {other}"),
    }
    Ok(url.to_string())
}

async fn read_request_body(body: Body) -> Result<Bytes, ApiError> {
    let mut body = body;
    let mut bytes = Vec::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.map_err(|err| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("failed to read proxied request body: {err}"),
            )
        })?;
        bytes.extend_from_slice(chunk.as_ref());
    }
    Ok(Bytes::from(bytes))
}

fn axum_message_to_tungstenite(message: AxumWsMessage) -> Option<TungsteniteMessage> {
    match message {
        AxumWsMessage::Text(text) => Some(TungsteniteMessage::Text(text.into())),
        AxumWsMessage::Binary(binary) => Some(TungsteniteMessage::Binary(binary.into())),
        AxumWsMessage::Ping(ping) => Some(TungsteniteMessage::Ping(ping.into())),
        AxumWsMessage::Pong(pong) => Some(TungsteniteMessage::Pong(pong.into())),
        AxumWsMessage::Close(Some(close)) => {
            Some(TungsteniteMessage::Close(Some(TungsteniteCloseFrame {
                code: TungsteniteCloseCode::from(close.code),
                reason: close.reason.into_owned().into(),
            })))
        }
        AxumWsMessage::Close(None) => Some(TungsteniteMessage::Close(None)),
    }
}

fn tungstenite_message_to_axum(message: TungsteniteMessage) -> Option<AxumWsMessage> {
    match message {
        TungsteniteMessage::Text(text) => Some(AxumWsMessage::Text(text.to_string())),
        TungsteniteMessage::Binary(binary) => Some(AxumWsMessage::Binary(binary.to_vec())),
        TungsteniteMessage::Ping(ping) => Some(AxumWsMessage::Ping(ping.to_vec())),
        TungsteniteMessage::Pong(pong) => Some(AxumWsMessage::Pong(pong.to_vec())),
        TungsteniteMessage::Close(Some(close)) => {
            Some(AxumWsMessage::Close(Some(axum::extract::ws::CloseFrame {
                code: close.code.into(),
                reason: close.reason.to_string().into(),
            })))
        }
        TungsteniteMessage::Close(None) => Some(AxumWsMessage::Close(None)),
        TungsteniteMessage::Frame(_) => None,
    }
}

async fn proxy_openclaw_websocket(
    downstream: WebSocket,
    upstream_request: tokio_tungstenite::tungstenite::http::Request<()>,
) -> anyhow::Result<()> {
    let (upstream, _) = connect_async(upstream_request).await?;
    let (mut upstream_sink, mut upstream_stream) = upstream.split();
    let (mut downstream_sink, mut downstream_stream) = downstream.split();

    let downstream_to_upstream = async {
        while let Some(message) = downstream_stream.next().await {
            let message = message?;
            let Some(message) = axum_message_to_tungstenite(message) else {
                continue;
            };
            upstream_sink.send(message).await?;
        }
        anyhow::Ok(())
    };

    let upstream_to_downstream = async {
        while let Some(message) = upstream_stream.next().await {
            let message = message?;
            let Some(message) = tungstenite_message_to_axum(message) else {
                continue;
            };
            downstream_sink.send(message).await?;
        }
        anyhow::Ok(())
    };

    let _ = tokio::join!(downstream_to_upstream, upstream_to_downstream);
    Ok(())
}

fn copy_response_headers(
    target: &mut HeaderMap,
    source: &reqwest::header::HeaderMap,
    allow_set_cookie: bool,
) {
    for (name, value) in source {
        let name_str = name.as_str();
        if header_is_hop_by_hop(name_str) {
            continue;
        }
        if !allow_set_cookie && name_str == "set-cookie" {
            continue;
        }
        let Ok(header_name) = HeaderName::from_bytes(name.as_str().as_bytes()) else {
            continue;
        };
        let Ok(header_value) = HeaderValue::from_bytes(value.as_bytes()) else {
            continue;
        };
        if response_header_should_replace_existing(name_str) {
            target.insert(header_name, header_value);
        } else {
            target.append(header_name, header_value);
        }
    }
}

fn response_header_should_replace_existing(name: &str) -> bool {
    matches!(
        name,
        "content-type" | "content-length" | "content-disposition" | "location"
    )
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

    #[test]
    fn copy_response_headers_replaces_content_type_instead_of_appending() {
        let mut target = HeaderMap::new();
        target.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        let mut source = reqwest::header::HeaderMap::new();
        source.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("text/html; charset=utf-8"),
        );

        copy_response_headers(&mut target, &source, false);

        let values = target.get_all(axum::http::header::CONTENT_TYPE);
        let collected: Vec<_> = values.iter().collect();
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0], "text/html; charset=utf-8");
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

    #[test]
    fn build_websocket_proxy_request_preserves_origin_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::ORIGIN,
            HeaderValue::from_static("https://openclaw.api.pikachat.org"),
        );
        headers.insert(
            axum::http::header::USER_AGENT,
            HeaderValue::from_static("agent-test"),
        );

        let request = build_websocket_proxy_request("ws://192.168.83.17:18789/", &headers)
            .expect("build websocket proxy request");

        assert_eq!(
            request
                .headers()
                .get("origin")
                .and_then(|value| value.to_str().ok()),
            Some("https://openclaw.api.pikachat.org")
        );
        assert_eq!(
            request
                .headers()
                .get("user-agent")
                .and_then(|value| value.to_str().ok()),
            Some("agent-test")
        );
    }
}
