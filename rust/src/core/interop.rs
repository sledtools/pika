use nostr_sdk::prelude::*;

pub use pika_marmot_runtime::key_package::normalize_peer_key_package_event_for_mdk;
pub(super) use pika_marmot_runtime::key_package::{
    extract_relays_from_key_package_event, extract_relays_from_key_package_relays_event,
};

pub(super) fn referenced_key_package_event_id(rumor: &UnsignedEvent) -> Option<EventId> {
    rumor
        .tags
        .find(TagKind::e())
        .and_then(|t| t.content())
        .and_then(|s| EventId::from_hex(s).ok())
}
