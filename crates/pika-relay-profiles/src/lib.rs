#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayProfileId {
    PikachatProduction,
    PublicNostrApp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelayProfile {
    pub id: RelayProfileId,
    pub name: &'static str,
    pub message_relays: &'static [&'static str],
    pub key_package_relays: &'static [&'static str],
    pub blossom_servers: &'static [&'static str],
}

impl RelayProfile {
    pub fn message_relays_vec(self) -> Vec<String> {
        self.message_relays
            .iter()
            .map(|v| (*v).to_string())
            .collect()
    }

    pub fn key_package_relays_vec(self) -> Vec<String> {
        self.key_package_relays
            .iter()
            .map(|v| (*v).to_string())
            .collect()
    }

    pub fn primary_blossom_server(self) -> &'static str {
        self.blossom_servers[0]
    }
}

pub const PIKACHAT_PRODUCTION: RelayProfile = RelayProfile {
    id: RelayProfileId::PikachatProduction,
    name: "pikachat-production",
    message_relays: &[
        "wss://us-east.nostr.pikachat.org",
        "wss://eu.nostr.pikachat.org",
    ],
    key_package_relays: &[
        "wss://nostr-pub.wellorder.net",
        "wss://nostr-01.yakihonne.com",
        "wss://nostr-02.yakihonne.com",
    ],
    blossom_servers: &[
        "https://us-east.nostr.pikachat.org",
        "https://eu.nostr.pikachat.org",
    ],
};

pub const PUBLIC_NOSTR_APP: RelayProfile = RelayProfile {
    id: RelayProfileId::PublicNostrApp,
    name: "public-nostr-app",
    message_relays: &[
        "wss://relay.primal.net",
        "wss://nos.lol",
        "wss://relay.damus.io",
        "wss://us-east.nostr.pikachat.org",
        "wss://eu.nostr.pikachat.org",
    ],
    key_package_relays: &[
        "wss://nostr-pub.wellorder.net",
        "wss://nostr-01.yakihonne.com",
        "wss://nostr-02.yakihonne.com",
    ],
    blossom_servers: &[
        "https://us-east.nostr.pikachat.org",
        "https://eu.nostr.pikachat.org",
    ],
};

pub fn default_profile() -> RelayProfile {
    PIKACHAT_PRODUCTION
}

pub fn app_profile() -> RelayProfile {
    PUBLIC_NOSTR_APP
}

pub fn default_message_relays() -> Vec<String> {
    default_profile().message_relays_vec()
}

pub fn default_key_package_relays() -> Vec<String> {
    default_profile().key_package_relays_vec()
}

pub fn default_primary_blossom_server() -> &'static str {
    default_profile().primary_blossom_server()
}

pub fn app_default_message_relays() -> Vec<String> {
    app_profile().message_relays_vec()
}

pub fn app_default_key_package_relays() -> Vec<String> {
    app_profile().key_package_relays_vec()
}

pub fn app_default_blossom_servers() -> Vec<String> {
    app_profile()
        .blossom_servers
        .iter()
        .map(|v| (*v).to_string())
        .collect()
}

/// Filter valid URLs from user-provided values, falling back to given defaults.
pub fn resolve_blossom_servers(values: &[String], defaults: &[&str]) -> Vec<String> {
    let parsed: Vec<String> = values
        .iter()
        .filter_map(|raw| {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return None;
            }
            url::Url::parse(trimmed).ok().map(|_| trimmed.to_string())
        })
        .collect();
    if !parsed.is_empty() {
        return parsed;
    }
    defaults
        .iter()
        .filter_map(|u| url::Url::parse(u).ok().map(|_| (*u).to_string()))
        .collect()
}

/// Resolve blossom servers using the pikachat production profile defaults.
pub fn blossom_servers_or_default(values: &[String]) -> Vec<String> {
    resolve_blossom_servers(values, default_profile().blossom_servers)
}

/// Resolve blossom servers using the app (public nostr) profile defaults.
pub fn app_blossom_servers_or_default(values: &[String]) -> Vec<String> {
    resolve_blossom_servers(values, app_profile().blossom_servers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pikachat_profile_contains_expected_defaults() {
        let profile = default_profile();
        assert_eq!(profile.id, RelayProfileId::PikachatProduction);
        assert_eq!(profile.name, "pikachat-production");
        assert_eq!(
            profile.message_relays,
            &[
                "wss://us-east.nostr.pikachat.org",
                "wss://eu.nostr.pikachat.org",
            ]
        );
        assert_eq!(
            profile.key_package_relays,
            &[
                "wss://nostr-pub.wellorder.net",
                "wss://nostr-01.yakihonne.com",
                "wss://nostr-02.yakihonne.com",
            ]
        );
        assert_eq!(
            profile.primary_blossom_server(),
            "https://us-east.nostr.pikachat.org"
        );
    }

    #[test]
    fn app_profile_contains_expected_defaults() {
        let profile = app_profile();
        assert_eq!(profile.id, RelayProfileId::PublicNostrApp);
        assert_eq!(profile.name, "public-nostr-app");
        assert_eq!(
            profile.message_relays,
            &[
                "wss://relay.primal.net",
                "wss://nos.lol",
                "wss://relay.damus.io",
                "wss://us-east.nostr.pikachat.org",
                "wss://eu.nostr.pikachat.org",
            ]
        );
        assert_eq!(
            profile.key_package_relays,
            &[
                "wss://nostr-pub.wellorder.net",
                "wss://nostr-01.yakihonne.com",
                "wss://nostr-02.yakihonne.com",
            ]
        );
        assert_eq!(
            profile.primary_blossom_server(),
            "https://us-east.nostr.pikachat.org"
        );
    }

    #[test]
    fn helper_accessors_match_profile_values() {
        let pikachat = default_profile();
        assert_eq!(default_message_relays(), pikachat.message_relays_vec());
        assert_eq!(
            default_key_package_relays(),
            pikachat.key_package_relays_vec()
        );
        assert_eq!(
            default_primary_blossom_server(),
            pikachat.primary_blossom_server()
        );

        let app = app_profile();
        assert_eq!(app_default_message_relays(), app.message_relays_vec());
        assert_eq!(
            app_default_key_package_relays(),
            app.key_package_relays_vec()
        );
        assert_eq!(
            app_default_blossom_servers(),
            app.blossom_servers
                .iter()
                .map(|v| (*v).to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn blossom_servers_or_default_uses_provided() {
        let servers = vec!["https://example.com".to_string()];
        let result = blossom_servers_or_default(&servers);
        assert_eq!(result, vec!["https://example.com"]);
    }

    #[test]
    fn blossom_servers_or_default_falls_back() {
        let result = blossom_servers_or_default(&[]);
        assert!(!result.is_empty());
        assert!(result[0].starts_with("https://"));
    }

    #[test]
    fn blossom_servers_or_default_skips_empty_and_invalid() {
        let servers = vec!["".to_string(), "not-a-url".to_string()];
        let result = blossom_servers_or_default(&servers);
        assert!(!result.is_empty());
        assert!(result[0].starts_with("https://"));
    }

    #[test]
    fn blossom_servers_or_default_filters_invalid_keeps_valid() {
        let servers = vec![
            "not-a-url".to_string(),
            "https://valid.example.com".to_string(),
        ];
        let result = blossom_servers_or_default(&servers);
        assert_eq!(result, vec!["https://valid.example.com"]);
    }

    #[test]
    fn app_blossom_servers_or_default_falls_back_to_app_profile() {
        let result = app_blossom_servers_or_default(&[]);
        assert_eq!(result, app_default_blossom_servers());
    }
}
