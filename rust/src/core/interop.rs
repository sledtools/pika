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

// Best-effort tag normalization for peers publishing legacy/interop keypackages:
// - protocol version "1" instead of "1.0"
// - ciphersuite "1" instead of "0x0001"
//
// Encoding validation (MIP-00: encoding tag MUST be "base64", hex MUST be
// rejected) is handled by MDK's parse_key_package() — not duplicated here.
//
// This does NOT re-sign the event; MDK doesn't require Nostr signature verification for
// keypackage parsing, but it does validate the credential identity matches `event.pubkey`.
pub(super) fn normalize_peer_key_package_event_for_mdk(event: &Event) -> Event {
    let mut out = event.clone();

    let mut tags: Vec<Tag> = Vec::new();
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
        tags.push(t.clone());
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

    /// Build a signed kind-443 event with the given tags and content.
    fn make_kp_event(content: &str, extra_tags: Vec<Tag>) -> Event {
        let keys = Keys::generate();
        let mut tags = vec![
            Tag::custom(TagKind::MlsProtocolVersion, ["1.0"]),
            Tag::custom(TagKind::MlsCiphersuite, ["0x0001"]),
        ];
        tags.extend(extra_tags);
        EventBuilder::new(Kind::MlsKeyPackage, content)
            .tags(tags)
            .sign_with_keys(&keys)
            .expect("sign")
    }

    #[test]
    fn passes_through_valid_event_unchanged() {
        let ev = make_kp_event(
            "AQID",
            vec![Tag::custom(TagKind::Custom("encoding".into()), ["base64"])],
        );
        let normalized = normalize_peer_key_package_event_for_mdk(&ev);
        // Tags preserved as-is (no rewriting needed)
        assert_eq!(ev.tags.as_slice().len(), normalized.tags.as_slice().len());
    }

    #[test]
    fn does_not_convert_hex_content() {
        // Hex content with hex encoding tag — normalizer must NOT convert it.
        // MDK's parse_key_package will reject this event.
        let ev = make_kp_event(
            "deadbeef",
            vec![Tag::custom(TagKind::Custom("encoding".into()), ["hex"])],
        );
        let normalized = normalize_peer_key_package_event_for_mdk(&ev);
        // Content left as-is — no hex→base64 conversion
        assert_eq!(normalized.content, "deadbeef");
        // Encoding tag left as-is — no rewriting to base64
        let enc = normalized
            .tags
            .iter()
            .find(|t| t.kind() == TagKind::Custom("encoding".into()))
            .and_then(|t| t.as_slice().get(1).map(|s| s.to_string()));
        assert_eq!(enc.as_deref(), Some("hex"));
    }

    #[test]
    fn does_not_inject_encoding_tag_when_missing() {
        // No encoding tag at all — normalizer must NOT add one.
        // MDK's parse_key_package will reject this event.
        let ev = make_kp_event("deadbeef", vec![]);
        let normalized = normalize_peer_key_package_event_for_mdk(&ev);
        let has_encoding = normalized
            .tags
            .iter()
            .any(|t| t.kind() == TagKind::Custom("encoding".into()));
        assert!(!has_encoding, "should not inject encoding tag");
    }

    #[test]
    fn normalizes_legacy_version_and_ciphersuite() {
        let keys = Keys::generate();
        let tags = vec![
            Tag::custom(TagKind::Custom("encoding".into()), ["base64"]),
            Tag::custom(TagKind::MlsProtocolVersion, ["1"]),
            Tag::custom(TagKind::MlsCiphersuite, ["1"]),
        ];
        let ev = EventBuilder::new(Kind::MlsKeyPackage, "AQID")
            .tags(tags)
            .sign_with_keys(&keys)
            .expect("sign");

        let normalized = normalize_peer_key_package_event_for_mdk(&ev);

        let version = normalized
            .tags
            .iter()
            .find(|t| t.kind() == TagKind::MlsProtocolVersion)
            .and_then(|t| t.as_slice().get(1).map(|s| s.to_string()));
        assert_eq!(version.as_deref(), Some("1.0"));

        let suite = normalized
            .tags
            .iter()
            .find(|t| t.kind() == TagKind::MlsCiphersuite)
            .and_then(|t| t.as_slice().get(1).map(|s| s.to_string()));
        assert_eq!(suite.as_deref(), Some("0x0001"));
    }
}
