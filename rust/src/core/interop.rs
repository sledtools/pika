use base64::Engine as _;
use nostr_sdk::prelude::*;

pub(super) fn extract_relays_from_key_package_event(event: &Event) -> Option<Vec<RelayUrl>> {
    for tag in event.tags.iter() {
        if tag.kind() == TagKind::Relays {
            let mut out = Vec::new();
            for value in tag.as_slice().iter().skip(1) {
                if let Ok(url) = RelayUrl::parse(value) {
                    out.push(url);
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
    for tag in event.tags.iter() {
        let values = tag.as_slice();
        if values.first().map(|value| value.as_str()) != Some("relay") {
            continue;
        }
        if let Some(url) = values.get(1) {
            if let Ok(parsed) = RelayUrl::parse(url) {
                out.push(parsed);
            }
        }
    }
    out
}

pub fn normalize_peer_key_package_event_for_mls(event: &Event) -> Event {
    let mut out = event.clone();

    let content_is_hex = {
        let content = out.content.trim();
        !content.is_empty()
            && content.len().is_multiple_of(2)
            && content.bytes().all(|byte| byte.is_ascii_hexdigit())
    };

    let mut encoding_value: Option<String> = None;
    for tag in out.tags.iter() {
        if tag.kind() == TagKind::Custom("encoding".into()) {
            if let Some(value) = tag.as_slice().get(1) {
                encoding_value = Some(value.to_string());
            }
        }
    }

    let mut tags: Vec<Tag> = Vec::new();
    let mut saw_encoding = false;
    for tag in out.tags.iter() {
        let kind = tag.kind();
        if kind == TagKind::MlsProtocolVersion {
            let value = tag.as_slice().get(1).map(|s| s.as_str()).unwrap_or("");
            if value == "1" {
                tags.push(Tag::custom(TagKind::MlsProtocolVersion, ["1.0"]));
                continue;
            }
        }
        if kind == TagKind::MlsCiphersuite {
            let value = tag.as_slice().get(1).map(|s| s.as_str()).unwrap_or("");
            if value == "1" {
                tags.push(Tag::custom(TagKind::MlsCiphersuite, ["0x0001"]));
                continue;
            }
        }
        if kind == TagKind::Custom("encoding".into()) {
            saw_encoding = true;
            tags.push(tag.clone());
            continue;
        }
        tags.push(tag.clone());
    }

    let encoding_is_hex = encoding_value
        .as_deref()
        .map(|value| value.eq_ignore_ascii_case("hex"))
        .unwrap_or(false);
    if encoding_is_hex || (!saw_encoding && content_is_hex) {
        if let Ok(bytes) = hex::decode(out.content.trim()) {
            out.content = base64::engine::general_purpose::STANDARD.encode(bytes);
            tags.retain(|tag| tag.kind() != TagKind::Custom("encoding".into()));
            tags.push(Tag::custom(TagKind::Custom("encoding".into()), ["base64"]));
        }
    } else if !saw_encoding {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key_package_event(content: &str, tags: Vec<Tag>) -> Event {
        EventBuilder::new(Kind::MlsKeyPackage, content)
            .tags(tags)
            .sign_with_keys(&Keys::generate())
            .expect("sign key package event")
    }

    fn tag_values(event: &Event, tag_name: &str) -> Vec<Vec<String>> {
        event
            .tags
            .iter()
            .filter(|tag| tag.as_slice().first().map(|value| value.as_str()) == Some(tag_name))
            .map(|tag| tag.as_slice().iter().map(ToString::to_string).collect())
            .collect()
    }

    #[test]
    fn normalize_rewrites_legacy_hex_key_package_tags_and_content() {
        let event = key_package_event(
            "68656c6c6f",
            vec![
                Tag::custom(TagKind::MlsProtocolVersion, ["1"]),
                Tag::custom(TagKind::MlsCiphersuite, ["1"]),
            ],
        );

        let normalized = normalize_peer_key_package_event_for_mls(&event);

        assert_eq!(normalized.content, "aGVsbG8=");
        assert_eq!(
            tag_values(&normalized, "mls_protocol_version"),
            vec![vec!["mls_protocol_version".to_string(), "1.0".to_string()]]
        );
        assert_eq!(
            tag_values(&normalized, "mls_ciphersuite"),
            vec![vec!["mls_ciphersuite".to_string(), "0x0001".to_string()]]
        );
        assert_eq!(
            tag_values(&normalized, "encoding"),
            vec![vec!["encoding".to_string(), "base64".to_string()]]
        );
    }

    #[test]
    fn normalize_rewrites_explicit_hex_encoding_to_base64() {
        let event = key_package_event(
            "68656c6c6f",
            vec![Tag::custom(TagKind::Custom("encoding".into()), ["hex"])],
        );

        let normalized = normalize_peer_key_package_event_for_mls(&event);

        assert_eq!(normalized.content, "aGVsbG8=");
        assert_eq!(
            tag_values(&normalized, "encoding"),
            vec![vec!["encoding".to_string(), "base64".to_string()]]
        );
    }

    #[test]
    fn normalize_adds_default_base64_encoding_for_modern_key_packages() {
        let event = key_package_event("aGVsbG8=", vec![]);

        let normalized = normalize_peer_key_package_event_for_mls(&event);

        assert_eq!(normalized.content, "aGVsbG8=");
        assert_eq!(
            tag_values(&normalized, "encoding"),
            vec![vec!["encoding".to_string(), "base64".to_string()]]
        );
    }

    #[test]
    fn extract_relays_from_key_package_event_ignores_invalid_entries() {
        let event = key_package_event(
            "aGVsbG8=",
            vec![Tag::custom(
                TagKind::Relays,
                ["wss://relay.one", "invalid relay", "wss://relay.two"],
            )],
        );

        let relays =
            extract_relays_from_key_package_event(&event).expect("key package relays present");

        assert_eq!(
            relays,
            vec![
                RelayUrl::parse("wss://relay.one").unwrap(),
                RelayUrl::parse("wss://relay.two").unwrap()
            ]
        );
    }

    #[test]
    fn extract_relays_from_key_package_relays_event_ignores_other_tags() {
        let event = EventBuilder::new(Kind::MlsKeyPackageRelays, "")
            .tags([
                Tag::custom(TagKind::Custom("relay".into()), ["wss://relay.one"]),
                Tag::custom(TagKind::Custom("relay".into()), ["not a relay"]),
                Tag::custom(TagKind::Custom("alt".into()), ["wss://relay.two"]),
            ])
            .sign_with_keys(&Keys::generate())
            .expect("sign relay event");

        let relays = extract_relays_from_key_package_relays_event(&event);

        assert_eq!(relays, vec![RelayUrl::parse("wss://relay.one").unwrap()]);
    }
}
