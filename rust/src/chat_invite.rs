use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedChatInvite {
    pub peer_key: String,
    pub server_url: Option<String>,
}

pub(crate) fn parse_chat_invite(input: &str) -> Option<ParsedChatInvite> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let without_nostr = trimmed
        .strip_prefix("nostr:")
        .or_else(|| trimmed.strip_prefix("NOSTR:"))
        .unwrap_or(trimmed)
        .trim();

    if let Some(peer_key) = canonical_peer_key(without_nostr) {
        return Some(ParsedChatInvite {
            peer_key,
            server_url: None,
        });
    }

    let url = Url::parse(without_nostr).ok()?;
    if !url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("chat"))
    {
        return None;
    }

    let peer_key = url
        .path_segments()
        .and_then(|mut segments| segments.next())
        .and_then(canonical_peer_key)?;
    let server_url = url.query_pairs().find_map(|(key, value)| {
        if key.eq_ignore_ascii_case("server") {
            normalize_server_url(value.as_ref())
        } else {
            None
        }
    });

    Some(ParsedChatInvite {
        peer_key,
        server_url,
    })
}

pub(crate) fn normalize_server_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut url = Url::parse(trimmed).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if url.path() == "/" {
        url.set_path("");
    }
    Some(url.to_string().trim_end_matches('/').to_string())
}

pub(crate) fn build_chat_invite_code(peer_npub: &str, server_url: Option<&str>) -> String {
    let peer_npub = peer_npub.trim();
    let Some(server_url) = server_url.and_then(normalize_server_url) else {
        return peer_npub.to_ascii_lowercase();
    };

    let mut url = Url::parse(&format!("pika://chat/{peer_npub}")).expect("valid pika deep link");
    url.query_pairs_mut().append_pair("server", &server_url);
    url.to_string()
}

pub(crate) fn normalize_peer_key_input(input: &str) -> String {
    if let Some(invite) = parse_chat_invite(input) {
        return invite.peer_key;
    }

    let mut normalized = input.trim().to_ascii_lowercase();
    if let Some(stripped) = normalized.strip_prefix("nostr:") {
        normalized = stripped.to_string();
    }
    if let Some(idx) = normalized.find("://chat/") {
        normalized = normalized[idx + "://chat/".len()..]
            .trim_matches('/')
            .split('?')
            .next()
            .unwrap_or_default()
            .to_string();
    }
    normalized
}

pub(crate) fn is_valid_peer_key_input(input: &str) -> bool {
    parse_chat_invite(input).is_some()
}

fn canonical_peer_key(input: &str) -> Option<String> {
    let normalized = input.trim().trim_matches('/').to_ascii_lowercase();
    if normalized.len() == 64 && normalized.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Some(normalized);
    }
    if !normalized.starts_with("npub1") {
        return None;
    }
    nostr_sdk::prelude::PublicKey::parse(&normalized)
        .ok()
        .map(|_| normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::prelude::ToBech32;

    #[test]
    fn parse_chat_invite_accepts_raw_npub() {
        let keys = nostr_sdk::prelude::Keys::generate();
        let npub = keys.public_key().to_bech32().expect("npub");
        let invite = parse_chat_invite(&npub);
        assert_eq!(
            invite,
            Some(ParsedChatInvite {
                peer_key: npub,
                server_url: None,
            })
        );
    }

    #[test]
    fn parse_chat_invite_accepts_deep_link_with_server() {
        let keys = nostr_sdk::prelude::Keys::generate();
        let npub = keys.public_key().to_bech32().expect("npub");
        let invite = parse_chat_invite(&format!(
            "pika://chat/{npub}?server=https%3A%2F%2Fchat.example"
        ))
        .expect("parsed invite");

        assert_eq!(invite.peer_key, npub);
        assert_eq!(invite.server_url.as_deref(), Some("https://chat.example"));
    }

    #[test]
    fn normalize_server_url_trims_bare_host_slash() {
        assert_eq!(
            normalize_server_url("https://chat.example/"),
            Some("https://chat.example".to_string())
        );
    }

    #[test]
    fn build_chat_invite_code_uses_server_query_when_present() {
        let code = build_chat_invite_code("npub1abc", Some("https://chat.example/"));
        assert_eq!(
            code,
            "pika://chat/npub1abc?server=https%3A%2F%2Fchat.example"
        );
    }
}
