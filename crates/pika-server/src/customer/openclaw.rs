use super::*;
use axum::body::{to_bytes, Body, Bytes};
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Form, FromRequestParts, Query};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Extension;
use futures::{SinkExt, StreamExt};
use pika_agent_microvm::MicrovmSpawnerClient;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as TungsteniteCloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame as TungsteniteCloseFrame;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tracing::warn;

use crate::agent_api::{load_launchable_managed_environment, spawner_base_url};
use crate::nostr_auth::expected_host_from_headers;

const OPENCLAW_UI_HOST_PREFIX: &str = "openclaw.";
pub(crate) const OPENCLAW_LAUNCH_TICKET_KIND: &str = "openclaw_ui_launch_ticket";
pub(crate) const OPENCLAW_UI_SESSION_KIND: &str = "openclaw_ui_session";
pub(crate) const OPENCLAW_UI_SESSION_COOKIE: &str = "pika_openclaw_ui_session";
const OPENCLAW_LAUNCH_TTL_SECS: i64 = 60;
const OPENCLAW_UI_SESSION_TTL_SECS: i64 = 15 * 60;
pub(crate) const OPENCLAW_INTERNAL_LAUNCH_PATH: &str = "/_openclaw_launch";
pub(crate) const OPENCLAW_INTERNAL_PROXY_PREFIX: &str = "/_openclaw_proxy";
pub(crate) const OPENCLAW_INTERNAL_PROXY_PATH: &str = "/_openclaw_proxy/*path";

#[derive(Debug, serde::Deserialize)]
pub(crate) struct LaunchTicketQuery {
    pub(crate) ticket: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub(crate) struct OpenClawLaunchTicket {
    pub(crate) kind: String,
    pub(crate) npub: String,
    pub(crate) agent_id: String,
    pub(crate) vm_id: String,
    pub(crate) ui_host: String,
    pub(crate) exp: i64,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub(crate) struct OpenClawUiSession {
    pub(crate) kind: String,
    pub(crate) npub: String,
    pub(crate) agent_id: String,
    pub(crate) vm_id: String,
    pub(crate) ui_host: String,
    pub(crate) exp: i64,
}

fn request_host(state: &State, headers: &HeaderMap) -> Result<String, (StatusCode, String)> {
    expected_host_from_headers(headers, state.trust_forwarded_host)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing host header".to_string()))
}

fn external_request_scheme(state: &State, headers: &HeaderMap) -> &'static str {
    if state.trust_forwarded_host {
        if let Some(proto) = headers
            .get("x-forwarded-proto")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
        {
            if proto.eq_ignore_ascii_case("http") {
                return "http";
            }
            if proto.eq_ignore_ascii_case("https") {
                return "https";
            }
        }
    }
    match headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
    {
        Some(host)
            if host.starts_with("localhost")
                || host.starts_with("127.0.0.1")
                || host.starts_with("[::1]") =>
        {
            "http"
        }
        _ => "https",
    }
}

fn openclaw_ui_host_for_dashboard_host(host: &str) -> String {
    if host.starts_with(OPENCLAW_UI_HOST_PREFIX) {
        host.to_string()
    } else {
        format!("{OPENCLAW_UI_HOST_PREFIX}{host}")
    }
}

fn cookie_value_from_headers(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    let cookie = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for pair in cookie.split(';') {
        let (name, value) = pair.trim().split_once('=')?;
        if name == cookie_name {
            return Some(value.to_string());
        }
    }
    None
}

fn issue_openclaw_launch_ticket(
    state: &State,
    npub: &str,
    agent_id: &str,
    vm_id: &str,
    ui_host: &str,
) -> Result<String, (StatusCode, String)> {
    browser_auth(state)
        .sign_payload(&OpenClawLaunchTicket {
            kind: OPENCLAW_LAUNCH_TICKET_KIND.to_string(),
            npub: npub.to_string(),
            agent_id: agent_id.to_string(),
            vm_id: vm_id.to_string(),
            ui_host: ui_host.to_string(),
            exp: chrono::Utc::now().timestamp() + OPENCLAW_LAUNCH_TTL_SECS,
        })
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

fn verify_openclaw_launch_ticket(
    state: &State,
    token: &str,
    actual_host: &str,
) -> Result<OpenClawLaunchTicket, (StatusCode, String)> {
    let ticket: OpenClawLaunchTicket = browser_auth(state).verify_payload(token).map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            "invalid launch ticket".to_string(),
        )
    })?;
    if ticket.kind != OPENCLAW_LAUNCH_TICKET_KIND {
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid launch ticket".to_string(),
        ));
    }
    if ticket.exp < chrono::Utc::now().timestamp() {
        return Err((
            StatusCode::UNAUTHORIZED,
            "launch ticket expired".to_string(),
        ));
    }
    if ticket.ui_host != actual_host {
        return Err((
            StatusCode::UNAUTHORIZED,
            "launch ticket host mismatch".to_string(),
        ));
    }
    Ok(ticket)
}

fn issue_openclaw_ui_session(
    state: &State,
    ticket: &OpenClawLaunchTicket,
) -> Result<String, (StatusCode, String)> {
    browser_auth(state)
        .sign_payload(&OpenClawUiSession {
            kind: OPENCLAW_UI_SESSION_KIND.to_string(),
            npub: ticket.npub.clone(),
            agent_id: ticket.agent_id.clone(),
            vm_id: ticket.vm_id.clone(),
            ui_host: ticket.ui_host.clone(),
            exp: chrono::Utc::now().timestamp() + OPENCLAW_UI_SESSION_TTL_SECS,
        })
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
}

fn openclaw_ui_session_from_headers(
    state: &State,
    headers: &HeaderMap,
    actual_host: &str,
) -> Result<Option<OpenClawUiSession>, (StatusCode, String)> {
    let Some(token) = cookie_value_from_headers(headers, OPENCLAW_UI_SESSION_COOKIE) else {
        return Ok(None);
    };
    let session: OpenClawUiSession = browser_auth(state).verify_payload(&token).map_err(|_| {
        (
            StatusCode::UNAUTHORIZED,
            "invalid openclaw ui session".to_string(),
        )
    })?;
    if session.kind != OPENCLAW_UI_SESSION_KIND {
        return Err((
            StatusCode::UNAUTHORIZED,
            "invalid openclaw ui session".to_string(),
        ));
    }
    if session.exp < chrono::Utc::now().timestamp() {
        return Err((
            StatusCode::UNAUTHORIZED,
            "openclaw ui session expired".to_string(),
        ));
    }
    if session.ui_host != actual_host {
        return Err((
            StatusCode::UNAUTHORIZED,
            "openclaw ui session host mismatch".to_string(),
        ));
    }
    Ok(Some(session))
}

fn origin_matches_host(origin: &str, actual_host: &str) -> bool {
    let Ok(origin_url) = reqwest::Url::parse(origin) else {
        return false;
    };
    let Some(origin_host) = origin_url.host_str() else {
        return false;
    };
    let origin_authority = match origin_url.port() {
        Some(port) => format!("{origin_host}:{port}"),
        None => origin_host.to_string(),
    };
    origin_authority.eq_ignore_ascii_case(actual_host)
}

fn require_same_origin_ui_request(
    headers: &HeaderMap,
    method: &Method,
    actual_host: &str,
) -> Result<(), (StatusCode, String)> {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return Ok(());
    }
    if let Some(origin) = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        return origin_matches_host(origin, actual_host)
            .then_some(())
            .ok_or_else(|| {
                (
                    StatusCode::FORBIDDEN,
                    "cross-origin OpenClaw UI mutation is not allowed".to_string(),
                )
            });
    }
    let same_origin_fetch = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .map(|value| value.eq_ignore_ascii_case("same-origin") || value == "none")
        .unwrap_or(false);
    if same_origin_fetch {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            "missing same-origin proof for OpenClaw UI mutation".to_string(),
        ))
    }
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

pub(crate) fn copy_proxy_response_headers(
    target: &mut HeaderMap,
    source: &reqwest::header::HeaderMap,
) {
    for (name, value) in source {
        let name_str = name.as_str();
        if header_is_hop_by_hop(name_str) || name_str == "set-cookie" {
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

pub(crate) fn openclaw_proxy_upstream_path(internal_path: &str, websocket_request: bool) -> &str {
    match internal_path.strip_prefix(OPENCLAW_INTERNAL_PROXY_PREFIX) {
        Some("") | Some("/") if websocket_request => "/",
        Some("") | Some("/") => "/app",
        Some(path) if !path.is_empty() => path,
        _ if websocket_request => "/",
        _ => "/app",
    }
}

fn response_header_should_replace_existing(name: &str) -> bool {
    matches!(
        name,
        "content-type" | "content-length" | "content-disposition" | "location"
    )
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

pub(crate) fn build_websocket_proxy_request(
    upstream_url: &str,
    downstream_headers: &HeaderMap,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, (StatusCode, String)> {
    let mut request = upstream_url.into_client_request().map_err(|err| {
        (
            StatusCode::BAD_GATEWAY,
            format!("invalid openclaw websocket upstream request: {err}"),
        )
    })?;
    for (name, value) in downstream_headers {
        if websocket_proxy_header_should_forward(name.as_str()) {
            let request_name = reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes())
                .map_err(|err| {
                    (
                        StatusCode::BAD_GATEWAY,
                        format!("invalid openclaw websocket upstream header name: {err}"),
                    )
                })?;
            let request_value = reqwest::header::HeaderValue::from_bytes(value.as_bytes())
                .map_err(|err| {
                    (
                        StatusCode::BAD_GATEWAY,
                        format!("invalid openclaw websocket upstream header value: {err}"),
                    )
                })?;
            request.headers_mut().insert(request_name, request_value);
        }
    }
    Ok(request)
}

fn websocket_proxy_url(upstream_url: &str) -> Result<String, (StatusCode, String)> {
    let mut url = reqwest::Url::parse(upstream_url).map_err(|err| {
        (
            StatusCode::BAD_GATEWAY,
            format!("invalid openclaw websocket upstream url: {err}"),
        )
    })?;
    match url.scheme() {
        "http" => url.set_scheme("ws").expect("replace http scheme with ws"),
        "https" => url
            .set_scheme("wss")
            .expect("replace https scheme with wss"),
        "ws" | "wss" => {}
        other => {
            return Err((
                StatusCode::BAD_GATEWAY,
                format!("unsupported openclaw websocket upstream scheme: {other}"),
            ));
        }
    }
    Ok(url.to_string())
}

async fn read_request_body(body: Body) -> Result<Bytes, (StatusCode, String)> {
    to_bytes(body, usize::MAX).await.map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            format!("failed to read proxied request body: {err}"),
        )
    })
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

pub(crate) async fn openclaw_launch(
    Extension(state): Extension<State>,
    Extension(request_context): Extension<RequestContext>,
    headers: HeaderMap,
    Form(form): Form<ActionForm>,
) -> Result<Response, (StatusCode, String)> {
    let Some(authenticated) = allowlisted_customer_from_session(&state, &headers).await? else {
        return redirect_to_login(&state, true);
    };
    verify_action_csrf(&authenticated, &form)?;

    let launch_target = load_launchable_managed_environment(
        &state,
        &authenticated.npub,
        &request_context.request_id,
    )
    .await
    .map_err(map_agent_api_error)?;
    let dashboard_host = request_host(&state, &headers)?;
    let ui_host = openclaw_ui_host_for_dashboard_host(&dashboard_host);
    let origin = format!(
        "{}://{}",
        external_request_scheme(&state, &headers),
        ui_host
    );
    let ticket = issue_openclaw_launch_ticket(
        &state,
        &launch_target.owner_npub,
        &launch_target.agent_id,
        &launch_target.vm_id,
        &ui_host,
    )?;
    let mut launch_url =
        reqwest::Url::parse(&format!("{origin}/launch")).expect("openclaw launch url");
    launch_url.query_pairs_mut().append_pair("ticket", &ticket);
    Ok(Redirect::to(launch_url.as_str()).into_response())
}

pub(crate) async fn openclaw_launch_exchange(
    Extension(state): Extension<State>,
    Extension(request_context): Extension<RequestContext>,
    headers: HeaderMap,
    Query(query): Query<LaunchTicketQuery>,
) -> Result<Response, (StatusCode, String)> {
    let actual_host = request_host(&state, &headers)?;
    let ticket = verify_openclaw_launch_ticket(&state, &query.ticket, &actual_host)?;

    let mut conn = state
        .db_pool
        .get()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    if !AgentAllowlistEntry::is_active(&mut conn, &ticket.npub)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "npub is not allowlisted".to_string()));
    }
    drop(conn);

    let launch_target =
        load_launchable_managed_environment(&state, &ticket.npub, &request_context.request_id)
            .await
            .map_err(map_agent_api_error)?;
    if launch_target.agent_id != ticket.agent_id || launch_target.vm_id != ticket.vm_id {
        return Err((
            StatusCode::CONFLICT,
            "managed openclaw environment changed before launch".to_string(),
        ));
    }

    let spawner_url = spawner_base_url(&request_context.request_id).map_err(map_agent_api_error)?;
    let spawner = MicrovmSpawnerClient::new(spawner_url);
    let launch_auth = spawner
        .get_openclaw_launch_auth_with_request_id(&ticket.vm_id, Some(&request_context.request_id))
        .await
        .map_err(|err| {
            warn!(
                request_id = %request_context.request_id,
                vm_id = %ticket.vm_id,
                error = %err,
                "failed to load managed openclaw launch auth"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load managed openclaw launch auth".to_string(),
            )
        })?;
    let ui_session = issue_openclaw_ui_session(&state, &ticket)?;
    let redirect_target = launch_auth
        .gateway_auth_token
        .map(|token| format!("/#token={token}"))
        .unwrap_or_else(|| "/".to_string());
    let mut response = Redirect::to(&redirect_target).into_response();
    browser_auth(&state)
        .set_session_cookie_with_path(
            &mut response,
            OPENCLAW_UI_SESSION_COOKIE,
            &ui_session,
            OPENCLAW_UI_SESSION_TTL_SECS,
            "/",
        )
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(response)
}

pub(crate) async fn openclaw_proxy(
    Extension(state): Extension<State>,
    Extension(request_context): Extension<RequestContext>,
    request: Request<Body>,
) -> Result<Response, (StatusCode, String)> {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let headers = request.headers().clone();
    let actual_host = request_host(&state, &headers)?;
    let Some(ui_session) = openclaw_ui_session_from_headers(&state, &headers, &actual_host)? else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "missing openclaw ui session".to_string(),
        ));
    };
    require_same_origin_ui_request(&headers, &method, &actual_host)?;

    let mut conn = state
        .db_pool
        .get()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    if !AgentAllowlistEntry::is_active(&mut conn, &ui_session.npub)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "npub is not allowlisted".to_string()));
    }
    drop(conn);

    let current = match load_launchable_managed_environment(
        &state,
        &ui_session.npub,
        &request_context.request_id,
    )
    .await
    {
        Ok(current) => current,
        Err(err)
            if err.status_code() == StatusCode::NOT_FOUND
                || err.status_code() == StatusCode::BAD_REQUEST =>
        {
            return Err((
                StatusCode::CONFLICT,
                "managed openclaw environment is not launchable".to_string(),
            ));
        }
        Err(err) => return Err(map_agent_api_error(err)),
    };
    if current.agent_id != ui_session.agent_id || current.vm_id != ui_session.vm_id {
        return Err((
            StatusCode::CONFLICT,
            "managed openclaw environment changed; relaunch from the dashboard".to_string(),
        ));
    }

    let websocket_request = request_is_websocket_upgrade(&method, &headers);
    let internal_path = uri.path();
    let upstream_path = openclaw_proxy_upstream_path(internal_path, websocket_request);
    let spawner_url = spawner_base_url(&request_context.request_id).map_err(map_agent_api_error)?;
    let spawner_proxy_path = if websocket_request && upstream_path == "/" {
        ""
    } else {
        upstream_path
    };
    let upstream_url = if let Some(query) = uri.query() {
        format!(
            "{spawner_url}/vms/{}/openclaw{spawner_proxy_path}?{query}",
            current.vm_id
        )
    } else {
        format!(
            "{spawner_url}/vms/{}/openclaw{spawner_proxy_path}",
            current.vm_id
        )
    };

    if websocket_request {
        let upstream_ws_url = websocket_proxy_url(&upstream_url)?;
        let upstream_request = build_websocket_proxy_request(&upstream_ws_url, &headers)?;
        let (mut parts, _body) = request.into_parts();
        let websocket = WebSocketUpgrade::from_request_parts(&mut parts, &())
            .await
            .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
        let request_id = request_context.request_id.clone();
        let vm_id = current.vm_id.clone();
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

    let body = read_request_body(request.into_body()).await?;
    let client = reqwest::Client::new();
    let upstream_method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).map_err(|err| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid proxied method: {err}"),
            )
        })?;
    let mut upstream = client.request(upstream_method, &upstream_url);
    for (name, value) in &headers {
        let name_str = name.as_str();
        if header_is_hop_by_hop(name_str)
            || matches!(name_str, "host" | "cookie" | "content-length")
        {
            continue;
        }
        let reqwest_name = reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes())
            .map_err(|err| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("invalid proxied header name: {err}"),
                )
            })?;
        let reqwest_value =
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()).map_err(|err| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("invalid proxied header value: {err}"),
                )
            })?;
        upstream = upstream.header(reqwest_name, reqwest_value);
    }
    if !body.is_empty() {
        upstream = upstream.body(body.to_vec());
    }
    let upstream = upstream.send().await.map_err(|err| {
        (
            StatusCode::BAD_GATEWAY,
            format!("openclaw proxy upstream failed: {err}"),
        )
    })?;

    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let upstream_headers = upstream.headers().clone();
    let upstream_body = upstream.bytes().await.map_err(|err| {
        (
            StatusCode::BAD_GATEWAY,
            format!("openclaw proxy upstream body failed: {err}"),
        )
    })?;
    let mut response = (status, upstream_body).into_response();
    copy_proxy_response_headers(response.headers_mut(), &upstream_headers);
    Ok(response)
}
