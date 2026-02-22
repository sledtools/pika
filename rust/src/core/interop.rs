use base64::Engine as _;
use nostr_sdk::prelude::*;

pub(super) fn extract_relays_from_key_package_event(event: &Event) -> Option<Vec<RelayUrl>> {
    for t in event.tags.iter() {
        if t.kind() == TagKind::Relays {
            let mut out = Vec::new();
            for s in t.as_slice().iter().skip(1) {
                if let Ok(u) = RelayUrl::parse(s) {
                    out.push(u);
                }
            }
            if !out.is_empty() {
                return Some(out);
            }
        }
    }
    None
}

pub(super) fn extract_relays_from_key_package_relays_event(event: &Event) -> Vec<RelayUrl> {
    let mut out = Vec::new();
    for t in event.tags.iter() {
        let values = t.as_slice();
        if values.first().map(|s| s.as_str()) != Some("relay") {
            continue;
        }
        if let Some(url) = values.get(1) {
            if let Ok(u) = RelayUrl::parse(url) {
                out.push(u);
            }
        }
    }
    out
}

// Best-effort compatibility for peers publishing legacy/interop keypackages:
// - protocol version "1" instead of "1.0"
// - ciphersuite "1" instead of "0x0001"
// - missing encoding tag + hex-encoded content instead of base64
//
// This does NOT re-sign the event; MDK doesn't require Nostr signature verification for
// keypackage parsing, but it does validate the credential identity matches `event.pubkey`.
pub(super) fn normalize_peer_key_package_event_for_mdk(event: &Event) -> Event {
    let mut out = event.clone();

    // Determine if content looks like hex. Some interop stacks omit the encoding tag and use hex.
    let content_is_hex = {
        let s = out.content.trim();
        !s.is_empty() && s.len().is_multiple_of(2) && s.bytes().all(|b| b.is_ascii_hexdigit())
    };

    let mut encoding_value: Option<String> = None;
    for t in out.tags.iter() {
        if t.kind() == TagKind::Custom("encoding".into()) {
            if let Some(v) = t.as_slice().get(1) {
                encoding_value = Some(v.to_string());
            }
        }
    }

    let mut tags: Vec<Tag> = Vec::new();
    let mut saw_encoding = false;
    for t in out.tags.iter() {
        let kind = t.kind();
        if kind == TagKind::MlsProtocolVersion {
            let v = t.as_slice().get(1).map(|s| s.as_str()).unwrap_or("");
            if v == "1" {
                tags.push(Tag::custom(TagKind::MlsProtocolVersion, ["1.0"]));
                continue;
            }
        }
        if kind == TagKind::MlsCiphersuite {
            let v = t.as_slice().get(1).map(|s| s.as_str()).unwrap_or("");
            if v == "1" {
                tags.push(Tag::custom(TagKind::MlsCiphersuite, ["0x0001"]));
                continue;
            }
        }
        if kind == TagKind::Custom("encoding".into()) {
            saw_encoding = true;
            // We'll rewrite to base64 if we convert from hex below.
            // Otherwise keep the original tag.
            tags.push(t.clone());
            continue;
        }
        tags.push(t.clone());
    }

    // Convert legacy hex -> base64 and force encoding tag.
    // Prefer explicit encoding=hex, but also accept missing encoding when content looks hex.
    let encoding_is_hex = encoding_value
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("hex"))
        .unwrap_or(false);
    if encoding_is_hex || (!saw_encoding && content_is_hex) {
        if let Ok(bytes) = hex::decode(out.content.trim()) {
            out.content = base64::engine::general_purpose::STANDARD.encode(bytes);

            // Replace/insert encoding tag to base64.
            tags.retain(|t| t.kind() != TagKind::Custom("encoding".into()));
            tags.push(Tag::custom(TagKind::Custom("encoding".into()), ["base64"]));
        }
    } else if !saw_encoding {
        // MDK requires an explicit encoding tag; default to base64 for modern clients.
        tags.push(Tag::custom(TagKind::Custom("encoding".into()), ["base64"]));
    }

    out.tags = tags.into_iter().collect();
    out
}

pub(super) fn referenced_key_package_event_id(rumor: &UnsignedEvent) -> Option<EventId> {
    rumor
        .tags
        .find(TagKind::e())
        .and_then(|t| t.content())
        .and_then(|s| EventId::from_hex(s).ok())
}
