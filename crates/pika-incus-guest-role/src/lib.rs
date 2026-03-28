use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IncusGuestRole {
    ManagedOpenclaw,
    JerichoRunner,
}

impl IncusGuestRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ManagedOpenclaw => "managed-openclaw",
            Self::JerichoRunner => "jericho-runner",
        }
    }

    pub const fn default_image_alias(self) -> &'static str {
        match self {
            Self::ManagedOpenclaw => "pika-agent/dev",
            Self::JerichoRunner => "jericho/dev",
        }
    }

    pub fn uses_default_image_alias(self, alias: &str) -> bool {
        alias.trim() == self.default_image_alias()
    }

    pub const fn flake_package_attr(self) -> &'static str {
        match self {
            Self::ManagedOpenclaw => "managed-openclaw-incus-image",
            Self::JerichoRunner => "jericho-runner-incus-image",
        }
    }
}

impl FromStr for IncusGuestRole {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "managed-openclaw" => Ok(Self::ManagedOpenclaw),
            "jericho-runner" => Ok(Self::JerichoRunner),
            other => Err(format!(
                "expected `managed-openclaw` or `jericho-runner`, got {other:?}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_uses_kebab_case_role_ids() {
        assert_eq!(
            serde_json::to_string(&IncusGuestRole::ManagedOpenclaw).unwrap(),
            "\"managed-openclaw\""
        );
        assert_eq!(
            serde_json::to_string(&IncusGuestRole::JerichoRunner).unwrap(),
            "\"jericho-runner\""
        );
        assert_eq!(
            serde_json::from_str::<IncusGuestRole>("\"managed-openclaw\"").unwrap(),
            IncusGuestRole::ManagedOpenclaw
        );
        assert_eq!(
            serde_json::from_str::<IncusGuestRole>("\"jericho-runner\"").unwrap(),
            IncusGuestRole::JerichoRunner
        );
    }

    #[test]
    fn roles_pin_expected_default_aliases_and_packages() {
        assert_eq!(
            IncusGuestRole::ManagedOpenclaw.default_image_alias(),
            "pika-agent/dev"
        );
        assert_eq!(
            IncusGuestRole::ManagedOpenclaw.flake_package_attr(),
            "managed-openclaw-incus-image"
        );
        assert_eq!(
            IncusGuestRole::JerichoRunner.default_image_alias(),
            "jericho/dev"
        );
        assert_eq!(
            IncusGuestRole::JerichoRunner.flake_package_attr(),
            "jericho-runner-incus-image"
        );
    }

    #[test]
    fn uses_default_image_alias_ignores_surrounding_whitespace() {
        assert!(IncusGuestRole::ManagedOpenclaw.uses_default_image_alias(" pika-agent/dev "));
        assert!(!IncusGuestRole::ManagedOpenclaw.uses_default_image_alias("jericho/dev"));
    }
}
