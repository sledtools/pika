use anyhow::Context;
use axum::http::{header, HeaderMap};
use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use nostr_sdk::prelude::{Event, TagKind, Timestamp, ToBech32};

const NIP98_KIND: u16 = 27235;
const MAX_SKEW_PAST_SECS: u64 = 300;
const MAX_SKEW_FUTURE_SECS: u64 = 60;

fn decode_base64_event(input: &str) -> anyhow::Result<Vec<u8>> {
    STANDARD
        .decode(input)
        .or_else(|_| URL_SAFE_NO_PAD.decode(input))
        .or_else(|_| URL_SAFE.decode(input))
        .with_context(|| "decode Nostr authorization payload")
}

pub fn event_from_authorization_header(headers: &HeaderMap) -> anyhow::Result<Event> {
    let auth = headers
        .get(header::AUTHORIZATION)
        .ok_or_else(|| anyhow::anyhow!("missing Authorization header"))?;
    let auth = auth
        .to_str()
        .ok()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("invalid Authorization header value"))?;

    let payload = auth
        .strip_prefix("Nostr ")
        .or_else(|| auth.strip_prefix("nostr "))
        .ok_or_else(|| anyhow::anyhow!("Authorization header must use Nostr scheme"))?;

    let decoded = decode_base64_event(payload)?;
    let event: Event =
        serde_json::from_slice(&decoded).context("decode signed Nostr event JSON")?;
    Ok(event)
}

fn normalize_signed_url_path(value: &str) -> String {
    if value.starts_with('/') {
        return value.to_string();
    }

    if let Some((_, after_scheme)) = value.split_once("://") {
        if let Some((_, path_with_query)) = after_scheme.split_once('/') {
            return format!("/{path_with_query}");
        }
        return "/".to_string();
    }

    value.to_string()
}

fn tag_content(event: &Event, tag_name: &str) -> Option<String> {
    event.tags.iter().find_map(|tag| {
        if tag.kind() == TagKind::custom(tag_name) {
            tag.content().map(|v| v.to_string())
        } else {
            None
        }
    })
}

pub fn verify_nip98_event(
    event: &Event,
    expected_method: &str,
    expected_path: &str,
    expected_content: Option<&str>,
) -> anyhow::Result<String> {
    event.verify().context("invalid nostr event signature")?;
    anyhow::ensure!(
        event.kind.as_u16() == NIP98_KIND,
        "unexpected event kind {}; expected {}",
        event.kind.as_u16(),
        NIP98_KIND
    );

    let now = Timestamp::now().as_secs();
    let created = event.created_at.as_secs();
    anyhow::ensure!(
        created + MAX_SKEW_PAST_SECS >= now,
        "nostr auth event is too old"
    );
    anyhow::ensure!(
        created <= now + MAX_SKEW_FUTURE_SECS,
        "nostr auth event is from the future"
    );

    let method =
        tag_content(event, "method").ok_or_else(|| anyhow::anyhow!("missing method tag"))?;
    anyhow::ensure!(
        method.eq_ignore_ascii_case(expected_method),
        "method mismatch"
    );

    let signed_url = tag_content(event, "u").ok_or_else(|| anyhow::anyhow!("missing u tag"))?;
    let signed_path = normalize_signed_url_path(&signed_url);
    anyhow::ensure!(
        signed_path == expected_path || signed_path.starts_with(&format!("{expected_path}?")),
        "u tag path mismatch"
    );

    if let Some(expected_content) = expected_content {
        anyhow::ensure!(
            event.content.as_str() == expected_content,
            "content mismatch"
        );
    }

    event.pubkey.to_bech32().context("encode requester npub")
}
