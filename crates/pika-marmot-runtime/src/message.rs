use hypernote_protocol as hn;
use nostr_sdk::prelude::{Kind, Tag, TagKind};

pub const TYPING_INDICATOR_KIND_NUM: u16 = 20_067;
pub const TYPING_INDICATOR_KIND: Kind = Kind::Custom(TYPING_INDICATOR_KIND_NUM);
pub const CALL_SIGNAL_KIND_NUM: u16 = 10;
pub const CALL_SIGNAL_KIND: Kind = Kind::Custom(CALL_SIGNAL_KIND_NUM);
pub const HYPERNOTE_KIND: Kind = Kind::Custom(hn::HYPERNOTE_KIND);
pub const HYPERNOTE_ACTION_RESPONSE_KIND: Kind = Kind::Custom(hn::HYPERNOTE_ACTION_RESPONSE_KIND);

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum MessageClassification {
    TypingIndicator,
    CallSignal,
    Chat,
    Reaction,
    Hypernote,
    HypernoteResponse,
    GroupProfile,
}

impl MessageClassification {
    pub fn increments_unread(self) -> bool {
        matches!(self, Self::Chat | Self::Hypernote)
    }

    pub fn increments_loaded(self) -> bool {
        matches!(self, Self::Chat | Self::Reaction | Self::Hypernote)
    }

    pub fn is_chat_visible(self) -> bool {
        matches!(
            self,
            Self::Chat | Self::Reaction | Self::Hypernote | Self::HypernoteResponse
        )
    }
}

pub fn is_pika_typing_indicator<'a>(
    content: &str,
    tags: impl IntoIterator<Item = &'a Tag>,
) -> bool {
    content == "typing"
        && tags.into_iter().any(|tag| {
            tag.kind() == TagKind::d()
                && tag
                    .content()
                    .map(|content| content == "pika")
                    .unwrap_or(false)
        })
}

pub fn classify_message<'a>(
    kind: Kind,
    content: &str,
    tags: impl IntoIterator<Item = &'a Tag>,
) -> Option<MessageClassification> {
    match kind {
        Kind::ChatMessage => Some(MessageClassification::Chat),
        Kind::Reaction => Some(MessageClassification::Reaction),
        Kind::Custom(TYPING_INDICATOR_KIND_NUM) => is_pika_typing_indicator(content, tags)
            .then_some(MessageClassification::TypingIndicator),
        Kind::Custom(CALL_SIGNAL_KIND_NUM) => Some(MessageClassification::CallSignal),
        Kind::Custom(hn::HYPERNOTE_KIND) => Some(MessageClassification::Hypernote),
        Kind::Custom(hn::HYPERNOTE_ACTION_RESPONSE_KIND) => {
            Some(MessageClassification::HypernoteResponse)
        }
        Kind::Metadata => Some(MessageClassification::GroupProfile),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use nostr_sdk::prelude::{Tag, Tags};

    fn pika_tags() -> Tags {
        vec![Tag::parse(["d", "pika"]).expect("pika d tag")]
            .into_iter()
            .collect()
    }

    #[test]
    fn typing_indicator_requires_pika_marker() {
        assert!(is_pika_typing_indicator("typing", pika_tags().iter()));
        assert!(!is_pika_typing_indicator("typing", Tags::new().iter()));
        assert!(!is_pika_typing_indicator("hello", pika_tags().iter()));
    }

    #[test]
    fn classify_message_maps_shared_kinds() {
        assert_eq!(
            classify_message(Kind::ChatMessage, "hello", Tags::new().iter()),
            Some(MessageClassification::Chat)
        );
        assert_eq!(
            classify_message(Kind::Reaction, "+", Tags::new().iter()),
            Some(MessageClassification::Reaction)
        );
        assert_eq!(
            classify_message(CALL_SIGNAL_KIND, "{}", Tags::new().iter()),
            Some(MessageClassification::CallSignal)
        );
        assert_eq!(
            classify_message(HYPERNOTE_KIND, "# Poll", Tags::new().iter()),
            Some(MessageClassification::Hypernote)
        );
        assert_eq!(
            classify_message(HYPERNOTE_ACTION_RESPONSE_KIND, "{}", Tags::new().iter()),
            Some(MessageClassification::HypernoteResponse)
        );
        assert_eq!(
            classify_message(Kind::Metadata, "{}", Tags::new().iter()),
            Some(MessageClassification::GroupProfile)
        );
        assert_eq!(
            classify_message(TYPING_INDICATOR_KIND, "typing", pika_tags().iter()),
            Some(MessageClassification::TypingIndicator)
        );
    }

    #[test]
    fn classify_message_rejects_unknown_and_unmarked_typing() {
        assert_eq!(
            classify_message(Kind::Custom(59_999), "x", Tags::new().iter()),
            None
        );
        assert_eq!(
            classify_message(TYPING_INDICATOR_KIND, "typing", Tags::new().iter()),
            None
        );
    }

    #[test]
    fn message_classification_visibility_matches_app_behavior() {
        assert!(MessageClassification::Chat.increments_unread());
        assert!(MessageClassification::Hypernote.increments_unread());
        assert!(!MessageClassification::Reaction.increments_unread());
        assert!(!MessageClassification::GroupProfile.increments_unread());

        assert!(MessageClassification::Chat.increments_loaded());
        assert!(MessageClassification::Reaction.increments_loaded());
        assert!(MessageClassification::Hypernote.increments_loaded());
        assert!(!MessageClassification::TypingIndicator.increments_loaded());
        assert!(!MessageClassification::HypernoteResponse.increments_loaded());

        assert!(MessageClassification::Chat.is_chat_visible());
        assert!(MessageClassification::Reaction.is_chat_visible());
        assert!(MessageClassification::Hypernote.is_chat_visible());
        assert!(MessageClassification::HypernoteResponse.is_chat_visible());
        assert!(!MessageClassification::TypingIndicator.is_chat_visible());
        assert!(!MessageClassification::CallSignal.is_chat_visible());
        assert!(!MessageClassification::GroupProfile.is_chat_visible());
    }
}
