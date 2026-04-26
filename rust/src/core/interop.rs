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
