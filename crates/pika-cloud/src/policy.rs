use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    Never,
    OnFailure,
    Always,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionPolicy {
    DestroyOnCompletion,
    KeepUntilStopped,
    DebugKeep,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputCollectionMode {
    None,
    FailureOnly,
    Always,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OutputCollectionPolicy {
    pub mode: OutputCollectionMode,
    #[serde(default = "default_true")]
    pub include_logs: bool,
    #[serde(default = "default_true")]
    pub include_result: bool,
    #[serde(default)]
    pub artifact_globs: Vec<String>,
}

const fn default_true() -> bool {
    true
}

impl Default for OutputCollectionPolicy {
    fn default() -> Self {
        Self {
            mode: OutputCollectionMode::Always,
            include_logs: true,
            include_result: true,
            artifact_globs: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimePolicies {
    pub restart_policy: RestartPolicy,
    pub retention_policy: RetentionPolicy,
    #[serde(default)]
    pub output_collection: OutputCollectionPolicy,
}

impl Default for RuntimePolicies {
    fn default() -> Self {
        Self {
            restart_policy: RestartPolicy::Never,
            retention_policy: RetentionPolicy::DestroyOnCompletion,
            output_collection: OutputCollectionPolicy::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_collection_policy_defaults_to_collecting_logs_and_result() {
        let decoded: OutputCollectionPolicy =
            serde_json::from_str(r#"{ "mode": "always" }"#).expect("decode");
        assert!(decoded.include_logs);
        assert!(decoded.include_result);
        assert!(decoded.artifact_globs.is_empty());
    }

    #[test]
    fn runtime_policies_round_trip() {
        let policies = RuntimePolicies {
            restart_policy: RestartPolicy::Never,
            retention_policy: RetentionPolicy::DestroyOnCompletion,
            output_collection: OutputCollectionPolicy::default(),
        };
        let encoded = serde_json::to_value(&policies).expect("encode");
        let decoded: RuntimePolicies = serde_json::from_value(encoded).expect("decode");
        assert_eq!(decoded, policies);
    }
}
