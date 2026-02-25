#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayProfileId {
    Production,
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

pub const PRODUCTION: RelayProfile = RelayProfile {
    id: RelayProfileId::Production,
    name: "production",
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

pub fn default_profile() -> RelayProfile {
    PRODUCTION
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_profile_contains_expected_defaults() {
        let profile = default_profile();
        assert_eq!(profile.id, RelayProfileId::Production);
        assert_eq!(profile.name, "production");
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
    fn helper_accessors_match_profile_values() {
        let profile = default_profile();
        assert_eq!(default_message_relays(), profile.message_relays_vec());
        assert_eq!(
            default_key_package_relays(),
            profile.key_package_relays_vec()
        );
        assert_eq!(
            default_primary_blossom_server(),
            profile.primary_blossom_server()
        );
    }
}
