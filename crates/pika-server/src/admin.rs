use std::collections::HashSet;

use anyhow::Context;
use axum::extract::{Form, Path};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::{Extension, Json};
use base64::Engine;
use hmac::{Hmac, Mac};
use nostr_sdk::prelude::{Keys, PublicKey};
use nostr_sdk::ToBech32;
use rand::Rng;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::models::agent_allowlist::AgentAllowlistEntry;
use crate::nostr_auth::verify_nip98_event;
use crate::State;

type HmacSha256 = Hmac<Sha256>;

const ADMIN_BOOTSTRAP_ENV: &str = "PIKA_ADMIN_BOOTSTRAP_NPUBS";
const ADMIN_SESSION_SECRET_ENV: &str = "PIKA_ADMIN_SESSION_SECRET";
const ADMIN_DEV_MODE_ENV: &str = "PIKA_ADMIN_DEV_MODE";
const ADMIN_COOKIE_SECURE_ENV: &str = "PIKA_ADMIN_COOKIE_SECURE";
const TEST_NSEC_ENV: &str = "PIKA_TEST_NSEC";

const ADMIN_SESSION_COOKIE: &str = "pika_admin_session";
const ADMIN_SESSION_TTL_SECS: i64 = 8 * 60 * 60;
const ADMIN_CHALLENGE_TTL_SECS: i64 = 120;

#[derive(Clone, Debug)]
pub struct AdminConfig {
    pub bootstrap_admins: HashSet<String>,
    session_secret: Vec<u8>,
    pub dev_mode: bool,
    dev_npub: Option<String>,
    cookie_secure: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionPayload {
    kind: String,
    npub: String,
    exp: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChallengePayload {
    kind: String,
    nonce: String,
    exp: i64,
}

#[derive(Debug, Deserialize)]
pub struct VerifyRequest {
    event: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct AllowlistUpsertForm {
    npub: String,
    note: Option<String>,
    active: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AllowlistToggleForm {
    active: String,
}

impl AdminConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let raw = std::env::var(ADMIN_BOOTSTRAP_ENV)
            .context("missing PIKA_ADMIN_BOOTSTRAP_NPUBS (comma-separated npubs)")?;
        let mut admins = parse_npub_csv(&raw)?;

        let dev_mode = env_truthy(ADMIN_DEV_MODE_ENV);
        let dev_npub = if dev_mode {
            std::env::var(TEST_NSEC_ENV)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .map(|nsec| {
                    Keys::parse(&nsec)
                        .context("parse PIKA_TEST_NSEC")
                        .and_then(|keys| {
                            keys.public_key()
                                .to_bech32()
                                .context("derive npub from PIKA_TEST_NSEC")
                        })
                })
                .transpose()?
        } else {
            None
        };

        if let Some(ref npub) = dev_npub {
            admins.insert(npub.to_lowercase());
        }

        let session_secret = std::env::var(ADMIN_SESSION_SECRET_ENV)
            .context("missing PIKA_ADMIN_SESSION_SECRET")?
            .into_bytes();
        anyhow::ensure!(
            session_secret.len() >= 16,
            "PIKA_ADMIN_SESSION_SECRET must be at least 16 bytes"
        );

        let cookie_secure = std::env::var(ADMIN_COOKIE_SECURE_ENV)
            .ok()
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(!dev_mode);

        Ok(Self {
            bootstrap_admins: admins,
            session_secret,
            dev_mode,
            dev_npub,
            cookie_secure,
        })
    }

    fn is_bootstrap_admin(&self, npub: &str) -> bool {
        self.bootstrap_admins.contains(&npub.trim().to_lowercase())
    }

    fn issue_challenge(&self) -> anyhow::Result<String> {
        let payload = ChallengePayload {
            kind: "admin_challenge".to_string(),
            nonce: hex::encode(rand::thread_rng().gen::<[u8; 16]>()),
            exp: now_unix() + ADMIN_CHALLENGE_TTL_SECS,
        };
        sign_token(&self.session_secret, &payload)
    }

    fn verify_challenge(&self, token: &str) -> anyhow::Result<()> {
        let payload: ChallengePayload = verify_token(&self.session_secret, token)?;
        anyhow::ensure!(
            payload.kind == "admin_challenge",
            "invalid challenge payload kind"
        );
        anyhow::ensure!(payload.exp >= now_unix(), "challenge expired");
        Ok(())
    }

    fn issue_session_token(&self, npub: &str) -> anyhow::Result<String> {
        let payload = SessionPayload {
            kind: "admin_session".to_string(),
            npub: npub.to_string(),
            exp: now_unix() + ADMIN_SESSION_TTL_SECS,
        };
        sign_token(&self.session_secret, &payload)
    }

    fn verify_session_token(&self, token: &str) -> anyhow::Result<String> {
        let payload: SessionPayload = verify_token(&self.session_secret, token)?;
        anyhow::ensure!(
            payload.kind == "admin_session",
            "invalid session payload kind"
        );
        anyhow::ensure!(payload.exp >= now_unix(), "session expired");
        anyhow::ensure!(
            self.is_bootstrap_admin(&payload.npub),
            "session admin is not bootstrap-authorized"
        );
        Ok(payload.npub)
    }

    fn set_session_cookie(&self, response: &mut Response, token: &str) -> anyhow::Result<()> {
        let secure = if self.cookie_secure { "; Secure" } else { "" };
        let value = format!(
            "{ADMIN_SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age={ADMIN_SESSION_TTL_SECS}"
        );
        response.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_str(&value).context("build Set-Cookie header")?,
        );
        Ok(())
    }

    fn clear_session_cookie(&self, response: &mut Response) -> anyhow::Result<()> {
        let secure = if self.cookie_secure { "; Secure" } else { "" };
        let value =
            format!("{ADMIN_SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age=0");
        response.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_str(&value).context("build clear Set-Cookie header")?,
        );
        Ok(())
    }

    fn session_npub_from_headers(&self, headers: &HeaderMap) -> Option<String> {
        let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
        for pair in cookie.split(';') {
            let (name, value) = pair.trim().split_once('=')?;
            if name == ADMIN_SESSION_COOKIE {
                if let Ok(npub) = self.verify_session_token(value) {
                    return Some(npub);
                }
            }
        }
        None
    }
}

pub fn admin_healthcheck() -> anyhow::Result<()> {
    let _ = AdminConfig::from_env()?;
    Ok(())
}

pub fn parse_npub_csv(raw: &str) -> anyhow::Result<HashSet<String>> {
    let mut out = HashSet::new();
    for token in raw.split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        anyhow::ensure!(
            trimmed.starts_with("npub1"),
            "bootstrap admin must be npub: {trimmed}"
        );
        let normalized = PublicKey::parse(trimmed)
            .with_context(|| format!("invalid bootstrap npub: {trimmed}"))?
            .to_bech32()
            .context("normalize bootstrap npub")?
            .to_lowercase();
        out.insert(normalized);
    }
    anyhow::ensure!(!out.is_empty(), "bootstrap admin npub set is empty");
    Ok(out)
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

fn sign_token<T: Serialize>(secret: &[u8], payload: &T) -> anyhow::Result<String> {
    let body = serde_json::to_vec(payload)?;
    let body_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(body);
    let mut mac = HmacSha256::new_from_slice(secret).context("init hmac")?;
    mac.update(body_b64.as_bytes());
    let sig_b64 =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(format!("{body_b64}.{sig_b64}"))
}

fn verify_token<T: DeserializeOwned>(secret: &[u8], token: &str) -> anyhow::Result<T> {
    let (body_b64, sig_b64) = token
        .split_once('.')
        .ok_or_else(|| anyhow::anyhow!("invalid token format"))?;
    let mut mac = HmacSha256::new_from_slice(secret).context("init hmac")?;
    mac.update(body_b64.as_bytes());
    let expected = mac.finalize().into_bytes();
    let actual = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(sig_b64)
        .context("decode token signature")?;
    anyhow::ensure!(actual == expected.as_slice(), "invalid token signature");

    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body_b64)
        .context("decode token payload")?;
    let parsed = serde_json::from_slice(&payload).context("decode token JSON payload")?;
    Ok(parsed)
}

fn normalize_npub(input: &str) -> anyhow::Result<String> {
    let normalized = PublicKey::parse(input.trim())
        .context("invalid npub")?
        .to_bech32()
        .context("normalize npub")?
        .to_lowercase();
    Ok(normalized)
}

fn parse_form_bool(value: &str) -> anyhow::Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Ok(true),
        "0" | "false" | "off" | "no" => Ok(false),
        _ => anyhow::bail!("invalid bool value"),
    }
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn render_login_html(dev_mode: bool) -> String {
    let dev_button = if dev_mode {
        r#"<form method=\"post\" action=\"/admin/dev-login\" style=\"margin-top:12px\"><button type=\"submit\">Dev Login (PIKA_TEST_NSEC)</button></form>"#
    } else {
        ""
    };

    format!(
        r#"<!doctype html>
<html>
<head>
  <meta charset=\"utf-8\" />
  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />
  <title>Pika Admin Login</title>
</head>
<body style=\"font-family: -apple-system,BlinkMacSystemFont,Segoe UI,Helvetica,Arial,sans-serif; max-width: 700px; margin: 32px auto; padding: 0 16px;\">
  <h1>Pika Admin</h1>
  <p>Sign in with a Nostr extension (NIP-07).</p>
  <button id=\"login\">Sign in with Nostr</button>
  {dev_button}
  <p id=\"status\" style=\"margin-top:10px;color:#555\"></p>
  <script>
  (function() {{
    const btn = document.getElementById('login');
    const status = document.getElementById('status');
    async function login() {{
      if (!window.nostr) {{
        alert('No Nostr extension detected.');
        return;
      }}
      try {{
        btn.disabled = true;
        status.textContent = 'Requesting challenge...';
        const challengeRes = await fetch('/admin/challenge', {{ method: 'POST' }});
        if (!challengeRes.ok) throw new Error('challenge request failed');
        const {{ challenge }} = await challengeRes.json();

        status.textContent = 'Signing event...';
        const pubkey = await window.nostr.getPublicKey();
        const event = {{
          kind: 27235,
          created_at: Math.floor(Date.now()/1000),
          tags: [['u', window.location.origin + '/admin/verify'], ['method', 'POST']],
          content: challenge,
          pubkey
        }};
        const signed = await window.nostr.signEvent(event);

        status.textContent = 'Verifying...';
        const verifyRes = await fetch('/admin/verify', {{
          method: 'POST',
          headers: {{ 'Content-Type': 'application/json' }},
          body: JSON.stringify({{ event: signed }})
        }});
        if (!verifyRes.ok) {{
          const body = await verifyRes.json().catch(() => ({{}}));
          throw new Error(body.error || 'verify failed');
        }}
        window.location.assign('/admin');
      }} catch (err) {{
        btn.disabled = false;
        status.textContent = '';
        alert('Login failed: ' + (err.message || err));
      }}
    }}
    btn.addEventListener('click', login);
  }})();
  </script>
</body>
</html>"#
    )
}

fn render_admin_html(current_admin_npub: &str, rows: &[AgentAllowlistEntry]) -> String {
    let mut row_html = String::new();
    for row in rows {
        let (next_active, label) = if row.active {
            ("0", "Disable")
        } else {
            ("1", "Enable")
        };
        row_html.push_str(&format!(
            "<tr><td><code>{}</code></td><td>{}</td><td>{}</td><td><code>{}</code></td><td>{}</td><td><form method=\"post\" action=\"/admin/allowlist/{}/toggle\"><input type=\"hidden\" name=\"active\" value=\"{}\" /><button type=\"submit\">{}</button></form></td></tr>",
            html_escape(&row.npub),
            if row.active { "active" } else { "inactive" },
            html_escape(row.note.as_deref().unwrap_or("")),
            html_escape(&row.updated_by),
            row.updated_at,
            html_escape(&row.npub),
            next_active,
            label,
        ));
    }

    format!(
        r#"<!doctype html>
<html>
<head>
  <meta charset=\"utf-8\" />
  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />
  <title>Pika Admin</title>
</head>
<body style=\"font-family: -apple-system,BlinkMacSystemFont,Segoe UI,Helvetica,Arial,sans-serif; margin: 24px;\">
  <h1>Agent Allowlist</h1>
  <p>Signed in as <code>{}</code></p>
  <form method=\"post\" action=\"/admin/logout\" style=\"margin-bottom: 20px;\">
    <button type=\"submit\">Logout</button>
  </form>

  <h2>Add / Update</h2>
  <form method=\"post\" action=\"/admin/allowlist\" style=\"display:grid; gap:8px; max-width: 620px;\">
    <label>Npub <input name=\"npub\" required style=\"width:100%\" /></label>
    <label>Note <input name=\"note\" style=\"width:100%\" /></label>
    <label><input type=\"checkbox\" name=\"active\" checked /> Active</label>
    <button type=\"submit\">Save</button>
  </form>

  <h2 style=\"margin-top:24px\">Current Entries</h2>
  <table border=\"1\" cellpadding=\"6\" cellspacing=\"0\" style=\"border-collapse:collapse; width:100%; max-width:1200px\">
    <thead>
      <tr><th>Npub</th><th>Status</th><th>Note</th><th>Updated By</th><th>Updated At (UTC)</th><th>Action</th></tr>
    </thead>
    <tbody>{}</tbody>
  </table>
</body>
</html>"#,
        html_escape(current_admin_npub),
        row_html
    )
}

fn admin_config(state: &State) -> &AdminConfig {
    &state.admin_config
}

pub async fn login_page(
    Extension(state): Extension<State>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    if admin_config(&state)
        .session_npub_from_headers(&headers)
        .is_some()
    {
        return Ok(Redirect::to("/admin").into_response());
    }
    Ok(Html(render_login_html(admin_config(&state).dev_mode)).into_response())
}

pub async fn challenge(
    Extension(state): Extension<State>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let challenge = admin_config(&state)
        .issue_challenge()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Json(serde_json::json!({ "challenge": challenge })))
}

pub async fn verify(
    Extension(state): Extension<State>,
    Json(payload): Json<VerifyRequest>,
) -> Result<Response, (StatusCode, String)> {
    let event: nostr_sdk::prelude::Event =
        serde_json::from_value(payload.event).map_err(|err| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid event JSON: {err}"),
            )
        })?;

    let npub = verify_nip98_event(&event, "POST", "/admin/verify", None)
        .map_err(|err| (StatusCode::UNAUTHORIZED, err.to_string()))?;

    admin_config(&state)
        .verify_challenge(event.content.as_str())
        .map_err(|err| (StatusCode::UNAUTHORIZED, err.to_string()))?;

    if !admin_config(&state).is_bootstrap_admin(&npub) {
        return Err((
            StatusCode::FORBIDDEN,
            "npub is not an authorized bootstrap admin".to_string(),
        ));
    }

    let token = admin_config(&state)
        .issue_session_token(&npub)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    let mut response = Json(serde_json::json!({ "ok": true, "npub": npub })).into_response();
    admin_config(&state)
        .set_session_cookie(&mut response, &token)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(response)
}

pub async fn dashboard(
    Extension(state): Extension<State>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    let Some(admin_npub) = admin_config(&state).session_npub_from_headers(&headers) else {
        return Ok(Redirect::to("/admin/login").into_response());
    };

    let mut conn = state
        .db_pool
        .get()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let rows = AgentAllowlistEntry::list(&mut conn)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Html(render_admin_html(&admin_npub, &rows)).into_response())
}

pub async fn upsert_allowlist(
    Extension(state): Extension<State>,
    headers: HeaderMap,
    Form(form): Form<AllowlistUpsertForm>,
) -> Result<Response, (StatusCode, String)> {
    let Some(admin_npub) = admin_config(&state).session_npub_from_headers(&headers) else {
        return Ok(Redirect::to("/admin/login").into_response());
    };

    let npub = normalize_npub(&form.npub)
        .map_err(|err| (StatusCode::BAD_REQUEST, format!("invalid npub: {err}")))?;
    let active = form.active.is_some();
    let note = form
        .note
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let mut conn = state
        .db_pool
        .get()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    AgentAllowlistEntry::upsert(&mut conn, &npub, active, note.as_deref(), &admin_npub)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    AgentAllowlistEntry::record_audit(
        &mut conn,
        &admin_npub,
        &npub,
        if active {
            "upsert_active"
        } else {
            "upsert_inactive"
        },
        note.as_deref(),
    )
    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    Ok(Redirect::to("/admin").into_response())
}

pub async fn toggle_allowlist(
    Extension(state): Extension<State>,
    headers: HeaderMap,
    Path(npub): Path<String>,
    Form(form): Form<AllowlistToggleForm>,
) -> Result<Response, (StatusCode, String)> {
    let Some(admin_npub) = admin_config(&state).session_npub_from_headers(&headers) else {
        return Ok(Redirect::to("/admin/login").into_response());
    };

    let normalized = normalize_npub(&npub)
        .map_err(|err| (StatusCode::BAD_REQUEST, format!("invalid npub: {err}")))?;
    let active =
        parse_form_bool(&form.active).map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;

    let mut conn = state
        .db_pool
        .get()
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    AgentAllowlistEntry::set_active(&mut conn, &normalized, active, &admin_npub)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    AgentAllowlistEntry::record_audit(
        &mut conn,
        &admin_npub,
        &normalized,
        if active { "enabled" } else { "disabled" },
        None,
    )
    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    Ok(Redirect::to("/admin").into_response())
}

pub async fn logout(Extension(state): Extension<State>) -> Result<Response, (StatusCode, String)> {
    let mut response = Redirect::to("/admin/login").into_response();
    admin_config(&state)
        .clear_session_cookie(&mut response)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(response)
}

pub async fn dev_login(
    Extension(state): Extension<State>,
) -> Result<Response, (StatusCode, String)> {
    if !admin_config(&state).dev_mode {
        return Err((StatusCode::NOT_FOUND, "dev mode disabled".to_string()));
    }
    let npub = admin_config(&state).dev_npub.clone().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "PIKA_TEST_NSEC is missing".to_string(),
        )
    })?;

    let token = admin_config(&state)
        .issue_session_token(&npub)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    let mut response = Redirect::to("/admin").into_response();
    admin_config(&state)
        .set_session_cookie(&mut response, &token)
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_npub_csv_rejects_invalid_items() {
        let err = parse_npub_csv("not-a-npub").expect_err("invalid npub must fail");
        assert!(err.to_string().contains("bootstrap admin must be npub"));
    }

    #[test]
    fn parse_npub_csv_normalizes_and_dedupes() {
        let set = parse_npub_csv(
            "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y,npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y",
        )
        .expect("valid set");
        assert_eq!(set.len(), 1);
    }
}
