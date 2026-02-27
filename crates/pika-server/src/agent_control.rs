use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use anyhow::Context;
use async_trait::async_trait;
use nostr_sdk::prelude::*;
use pika_agent_control_plane::{
    AgentControlCmdEnvelope, AgentControlCommand, AgentControlErrorEnvelope,
    AgentControlResultEnvelope, AgentControlStatusEnvelope, GetRuntimeCommand, ListRuntimesCommand,
    ProcessWelcomeCommand, ProtocolKind, ProviderKind, ProvisionCommand, RuntimeDescriptor,
    RuntimeLifecyclePhase, TeardownCommand, CMD_SCHEMA_V1, CONTROL_CMD_KIND, CONTROL_ERROR_KIND,
    CONTROL_RESULT_KIND, CONTROL_STATUS_KIND, ERROR_SCHEMA_V1, RESULT_SCHEMA_V1, STATUS_SCHEMA_V1,
};
use pika_agent_microvm::{
    build_create_vm_request, resolve_params, spawner_create_error, MicrovmSpawnerClient,
};
use pika_relay_profiles::default_message_relays;
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::agent_clients::fly_machines::FlyClient;

const DEFAULT_CONTROL_STATE_PATH: &str = ".pika-agent-control-state.json";
const DEFAULT_CONTROL_LOOKBACK_SECS: u64 = 600;
const DEFAULT_IDEMPOTENCY_MAX_ENTRIES: usize = 8192;
const EVENT_DEDUP_WINDOW: usize = 8192;

#[derive(Clone)]
pub struct AgentControlRuntime {
    client: Client,
    keys: Keys,
    relays: Vec<RelayUrl>,
    service: AgentControlService,
}

impl AgentControlRuntime {
    pub async fn from_env() -> anyhow::Result<Option<Self>> {
        let explicit_enabled = env_bool("PIKA_AGENT_CONTROL_ENABLED");
        let maybe_secret = std::env::var("PIKA_AGENT_CONTROL_NOSTR_SECRET")
            .ok()
            .or_else(|| std::env::var("NOSTR_SECRET_KEY").ok())
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        let enabled = explicit_enabled.unwrap_or(maybe_secret.is_some());
        if !enabled {
            return Ok(None);
        }

        let secret = maybe_secret.context(
            "agent control is enabled but no secret key found (set PIKA_AGENT_CONTROL_NOSTR_SECRET or NOSTR_SECRET_KEY)",
        )?;
        let keys = Keys::parse(&secret).context("parse agent control nostr secret key")?;

        let relay_csv = std::env::var("PIKA_AGENT_CONTROL_RELAYS")
            .ok()
            .or_else(|| std::env::var("RELAYS").ok())
            .unwrap_or_default();
        let relay_urls: Vec<String> = relay_csv
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if relay_urls.is_empty() {
            anyhow::bail!(
                "agent control is enabled but no relays are configured (set PIKA_AGENT_CONTROL_RELAYS or RELAYS)"
            );
        }
        let relays = parse_relay_urls(&relay_urls)?;

        let client = Client::new(keys.clone());
        for relay in &relays {
            client
                .add_relay(relay.clone())
                .await
                .with_context(|| format!("add agent control relay {relay}"))?;
        }
        client.connect().await;

        info!(
            pubkey = %keys.public_key(),
            relay_count = relays.len(),
            "agent control plane enabled"
        );

        Ok(Some(Self {
            client,
            keys,
            relays,
            service: AgentControlService::new()?,
        }))
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let lookback_secs = control_cmd_lookback_secs();
        let since_unix = Timestamp::now().as_secs().saturating_sub(lookback_secs);
        let filter = Filter::new()
            .kind(Kind::Custom(CONTROL_CMD_KIND))
            .custom_tag(
                SingleLetterTag::lowercase(Alphabet::P),
                self.keys.public_key().to_hex(),
            )
            .since(Timestamp::from(since_unix));
        self.client.subscribe(filter, None).await?;

        let mut notifications = self.client.notifications();
        let mut seen: HashSet<EventId> = HashSet::new();
        let mut seen_order: VecDeque<EventId> = VecDeque::new();

        loop {
            let notification = match notifications.recv().await {
                Ok(notification) => notification,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "agent control listener lagged notifications");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    anyhow::bail!("agent control listener channel closed");
                }
            };

            let RelayPoolNotification::Event { event, .. } = notification else {
                continue;
            };
            let event = *event;
            if event.kind != Kind::Custom(CONTROL_CMD_KIND) {
                continue;
            }
            if !seen.insert(event.id) {
                continue;
            }
            seen_order.push_back(event.id);
            while seen_order.len() > EVENT_DEDUP_WINDOW {
                if let Some(oldest) = seen_order.pop_front() {
                    seen.remove(&oldest);
                }
            }

            let requester = event.pubkey;
            let decrypted = match nostr_sdk::nostr::nips::nip44::decrypt(
                self.keys.secret_key(),
                &requester,
                event.content.as_str(),
            ) {
                Ok(content) => content,
                Err(err) => {
                    warn!(
                        error = %err,
                        requester = %requester,
                        "failed to decrypt control command"
                    );
                    continue;
                }
            };
            let cmd = match serde_json::from_str::<AgentControlCmdEnvelope>(&decrypted) {
                Ok(cmd) => cmd,
                Err(err) => {
                    let request_id = extract_request_id(&decrypted)
                        .unwrap_or_else(|| "unknown-request".to_string());
                    let envelope = AgentControlErrorEnvelope::v1(
                        request_id,
                        "invalid_command_json",
                        Some("command payload must decode as agent.control.cmd.v1".to_string()),
                        Some(err.to_string()),
                    );
                    if let Err(publish_err) = publish_control_event(
                        &self.client,
                        &self.keys,
                        &self.relays,
                        requester,
                        CONTROL_ERROR_KIND,
                        &envelope,
                    )
                    .await
                    {
                        error!(error = %publish_err, "failed to publish command decode error");
                    }
                    continue;
                }
            };

            let outcome = self
                .service
                .handle_command(&requester.to_hex(), requester, cmd)
                .await;

            for status in &outcome.statuses {
                if let Err(err) = publish_control_event(
                    &self.client,
                    &self.keys,
                    &self.relays,
                    requester,
                    CONTROL_STATUS_KIND,
                    status,
                )
                .await
                {
                    error!(error = %err, "failed to publish control status");
                }
            }

            if let Some(result) = &outcome.result {
                if let Err(err) = publish_control_event(
                    &self.client,
                    &self.keys,
                    &self.relays,
                    requester,
                    CONTROL_RESULT_KIND,
                    result,
                )
                .await
                {
                    error!(error = %err, "failed to publish control result");
                }
            }

            if let Some(error_envelope) = &outcome.error {
                if let Err(err) = publish_control_event(
                    &self.client,
                    &self.keys,
                    &self.relays,
                    requester,
                    CONTROL_ERROR_KIND,
                    error_envelope,
                )
                .await
                {
                    error!(error = %err, "failed to publish control error");
                }
            }
        }
    }
}

pub fn control_schema_healthcheck() -> anyhow::Result<()> {
    anyhow::ensure!(CMD_SCHEMA_V1 == "agent.control.cmd.v1");
    anyhow::ensure!(STATUS_SCHEMA_V1 == "agent.control.status.v1");
    anyhow::ensure!(RESULT_SCHEMA_V1 == "agent.control.result.v1");
    anyhow::ensure!(ERROR_SCHEMA_V1 == "agent.control.error.v1");
    Ok(())
}

fn extract_request_id(content: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|v| {
            v.get("request_id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
}

fn env_bool(key: &str) -> Option<bool> {
    std::env::var(key).ok().and_then(|raw| match raw.trim() {
        "1" | "true" | "TRUE" | "yes" | "on" => Some(true),
        "0" | "false" | "FALSE" | "no" | "off" => Some(false),
        _ => None,
    })
}

fn env_usize(key: &str) -> Option<usize> {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
}

fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
}

fn control_cmd_lookback_secs() -> u64 {
    env_u64("PIKA_AGENT_CONTROL_CMD_LOOKBACK_SECS").unwrap_or(DEFAULT_CONTROL_LOOKBACK_SECS)
}

fn parse_relay_urls(relay_urls: &[String]) -> anyhow::Result<Vec<RelayUrl>> {
    relay_urls
        .iter()
        .map(|relay| RelayUrl::parse(relay).with_context(|| format!("parse relay url {relay}")))
        .collect()
}

async fn publish_control_event(
    client: &Client,
    keys: &Keys,
    relays: &[RelayUrl],
    recipient: PublicKey,
    kind: u16,
    payload: &impl Serialize,
) -> anyhow::Result<()> {
    let content = serde_json::to_string(payload).context("serialize control event payload")?;
    let encrypted = nostr_sdk::nostr::nips::nip44::encrypt(
        keys.secret_key(),
        &recipient,
        content,
        nostr_sdk::nostr::nips::nip44::Version::V2,
    )
    .context("encrypt control event payload")?;
    let event = EventBuilder::new(Kind::Custom(kind), encrypted)
        .tags([Tag::public_key(recipient)])
        .sign_with_keys(keys)
        .context("sign control event")?;
    let output = client
        .send_event_to(relays.to_vec(), &event)
        .await
        .context("publish control event")?;
    if output.success.is_empty() {
        let reasons: Vec<String> = output.failed.values().cloned().collect();
        anyhow::bail!("no relay accepted control event kind={kind}: {reasons:?}");
    }
    Ok(())
}

#[derive(Clone, Debug)]
enum ProvisionPolicy {
    AllowAll,
    Allowlist(HashSet<String>),
    DenyAll,
}

impl ProvisionPolicy {
    fn is_allowed(&self, requester_pubkey_hex: &str) -> bool {
        match self {
            Self::AllowAll => true,
            Self::Allowlist(allowed) => allowed.contains(requester_pubkey_hex),
            Self::DenyAll => false,
        }
    }
}

fn load_provision_policy() -> anyhow::Result<ProvisionPolicy> {
    if env_bool("PIKA_AGENT_CONTROL_ALLOW_OPEN_PROVISIONING") == Some(true) {
        warn!("PIKA_AGENT_CONTROL_ALLOW_OPEN_PROVISIONING=1 set; any requester may provision");
        return Ok(ProvisionPolicy::AllowAll);
    }

    let raw = std::env::var("PIKA_AGENT_CONTROL_PROVISION_ALLOWLIST")
        .ok()
        .unwrap_or_default();
    let mut allowed = HashSet::new();
    for value in raw.split(',') {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        let pubkey = PublicKey::parse(trimmed)
            .with_context(|| format!("parse provision allowlist pubkey: {trimmed}"))?;
        allowed.insert(pubkey.to_hex());
    }
    if allowed.is_empty() {
        warn!(
            "no provisioning allowlist configured; provisioning commands are disabled (set PIKA_AGENT_CONTROL_PROVISION_ALLOWLIST or PIKA_AGENT_CONTROL_ALLOW_OPEN_PROVISIONING=1)"
        );
        Ok(ProvisionPolicy::DenyAll)
    } else {
        info!(count = allowed.len(), "loaded provisioning allowlist");
        Ok(ProvisionPolicy::Allowlist(allowed))
    }
}

#[derive(Clone)]
struct AgentControlService {
    state: std::sync::Arc<RwLock<ControlState>>,
    persistence: Option<std::sync::Arc<ControlStatePersistence>>,
    provision_policy: ProvisionPolicy,
    idempotency_max_entries: usize,
    fly: std::sync::Arc<dyn ProviderAdapter>,
    microvm: std::sync::Arc<dyn ProviderAdapter>,
}

impl AgentControlService {
    fn new() -> anyhow::Result<Self> {
        let state_path = std::env::var("PIKA_AGENT_CONTROL_STATE_PATH")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONTROL_STATE_PATH));
        let idempotency_max_entries = env_usize("PIKA_AGENT_CONTROL_IDEMPOTENCY_MAX_ENTRIES")
            .unwrap_or(DEFAULT_IDEMPOTENCY_MAX_ENTRIES)
            .max(256);
        let persistence = std::sync::Arc::new(ControlStatePersistence::new(state_path));
        let mut loaded_state = persistence.load()?;
        loaded_state.truncate_idempotency(idempotency_max_entries);
        info!(
            runtimes = loaded_state.runtimes.len(),
            idempotency = loaded_state.idempotency.len(),
            path = %persistence.path.display(),
            "loaded agent control state"
        );
        let provision_policy = load_provision_policy()?;
        Ok(Self {
            state: std::sync::Arc::new(RwLock::new(loaded_state)),
            persistence: Some(persistence),
            provision_policy,
            idempotency_max_entries,
            fly: std::sync::Arc::new(FlyAdapter),
            microvm: std::sync::Arc::new(MicrovmAdapter),
        })
    }

    #[cfg(test)]
    fn with_adapters(
        fly: std::sync::Arc<dyn ProviderAdapter>,
        microvm: std::sync::Arc<dyn ProviderAdapter>,
    ) -> Self {
        Self {
            state: std::sync::Arc::new(RwLock::new(ControlState::default())),
            persistence: None,
            provision_policy: ProvisionPolicy::AllowAll,
            idempotency_max_entries: DEFAULT_IDEMPOTENCY_MAX_ENTRIES,
            fly,
            microvm,
        }
    }

    #[cfg(test)]
    fn with_adapters_and_policy(
        fly: std::sync::Arc<dyn ProviderAdapter>,
        microvm: std::sync::Arc<dyn ProviderAdapter>,
        provision_policy: ProvisionPolicy,
        idempotency_max_entries: usize,
    ) -> Self {
        Self {
            state: std::sync::Arc::new(RwLock::new(ControlState::default())),
            persistence: None,
            provision_policy,
            idempotency_max_entries,
            fly,
            microvm,
        }
    }

    #[cfg(test)]
    fn with_adapters_policy_and_persistence(
        fly: std::sync::Arc<dyn ProviderAdapter>,
        microvm: std::sync::Arc<dyn ProviderAdapter>,
        provision_policy: ProvisionPolicy,
        idempotency_max_entries: usize,
        persistence: std::sync::Arc<ControlStatePersistence>,
    ) -> Self {
        Self {
            state: std::sync::Arc::new(RwLock::new(ControlState::default())),
            persistence: Some(persistence),
            provision_policy,
            idempotency_max_entries,
            fly,
            microvm,
        }
    }

    fn persist_state_snapshot(&self, state: &ControlState) -> anyhow::Result<()> {
        let Some(persistence) = &self.persistence else {
            return Ok(());
        };
        persistence.save(state)
    }

    fn adapter_for(&self, provider: ProviderKind) -> std::sync::Arc<dyn ProviderAdapter> {
        match provider {
            ProviderKind::Fly => self.fly.clone(),
            ProviderKind::Microvm => self.microvm.clone(),
        }
    }

    fn can_access_runtime(&self, requester_pubkey_hex: &str, runtime: &RuntimeRecord) -> bool {
        if runtime.owner_pubkey_hex.is_empty() {
            // Legacy state files (pre-owner field) remain manageable by trusted operators.
            return self.provision_policy.is_allowed(requester_pubkey_hex);
        }
        runtime.owner_pubkey_hex == requester_pubkey_hex
    }

    async fn handle_command(
        &self,
        requester_pubkey_hex: &str,
        requester_pubkey: PublicKey,
        cmd: AgentControlCmdEnvelope,
    ) -> CommandOutcome {
        let mut statuses = vec![AgentControlStatusEnvelope::v1(
            cmd.request_id.clone(),
            RuntimeLifecyclePhase::Queued,
            None,
            None,
            Some("request queued".to_string()),
            Value::Null,
        )];

        if cmd.schema != CMD_SCHEMA_V1 {
            return CommandOutcome::error(
                statuses,
                AgentControlErrorEnvelope::v1(
                    cmd.request_id,
                    "invalid_schema",
                    Some(format!("expected {CMD_SCHEMA_V1}")),
                    Some(format!("got {}", cmd.schema)),
                ),
            );
        }

        let cache_key = (
            requester_pubkey_hex.to_string(),
            cmd.idempotency_key.clone(),
        );
        {
            let state = self.state.read().await;
            if let Some(cached) = state.idempotency.get(&cache_key) {
                info!(
                    request_id = %cmd.request_id,
                    idempotency_key = %cmd.idempotency_key,
                    "replaying idempotent command"
                );
                statuses.push(AgentControlStatusEnvelope::v1(
                    cmd.request_id.clone(),
                    RuntimeLifecyclePhase::Ready,
                    cached.runtime_id(),
                    cached.provider(),
                    Some("idempotent replay".to_string()),
                    Value::Null,
                ));
                return cached.to_outcome(statuses, cmd.request_id);
            }
        }

        let outcome = match cmd.command.clone() {
            AgentControlCommand::Provision(provision) => {
                self.handle_provision(
                    requester_pubkey_hex,
                    cmd.request_id.clone(),
                    requester_pubkey,
                    provision,
                    statuses,
                )
                .await
            }
            AgentControlCommand::ProcessWelcome(process_welcome) => {
                self.handle_process_welcome(
                    requester_pubkey_hex,
                    cmd.request_id.clone(),
                    process_welcome,
                    statuses,
                )
                .await
            }
            AgentControlCommand::Teardown(teardown) => {
                self.handle_teardown(
                    requester_pubkey_hex,
                    cmd.request_id.clone(),
                    teardown,
                    statuses,
                )
                .await
            }
            AgentControlCommand::GetRuntime(get_runtime) => {
                self.handle_get_runtime(
                    requester_pubkey_hex,
                    cmd.request_id.clone(),
                    get_runtime,
                    statuses,
                )
                .await
            }
            AgentControlCommand::ListRuntimes(list) => {
                self.handle_list_runtimes(
                    requester_pubkey_hex,
                    cmd.request_id.clone(),
                    list,
                    statuses,
                )
                .await
            }
        };

        if outcome.result.is_none() || !should_cache_success_result(&cmd.command) {
            return outcome;
        }

        let Some(result) = &outcome.result else {
            return outcome;
        };
        let terminal = CachedTerminal::Result {
            provider: result.runtime.provider,
            runtime_id: result.runtime.runtime_id.clone(),
            runtime: Box::new(result.runtime.clone()),
            payload: result.payload.clone(),
        };

        let mut state = self.state.write().await;
        state.insert_idempotency(cache_key, terminal, self.idempotency_max_entries);
        if let Err(err) = self.persist_state_snapshot(&state) {
            error!(
                error = %err,
                request_id = %cmd.request_id,
                "failed to persist idempotency cache; continuing without durable replay entry"
            );
        }
        outcome
    }

    async fn handle_provision(
        &self,
        requester_pubkey_hex: &str,
        request_id: String,
        requester_pubkey: PublicKey,
        provision: ProvisionCommand,
        mut statuses: Vec<AgentControlStatusEnvelope>,
    ) -> CommandOutcome {
        if !self.provision_policy.is_allowed(requester_pubkey_hex) {
            statuses.push(AgentControlStatusEnvelope::v1(
                request_id.clone(),
                RuntimeLifecyclePhase::Failed,
                None,
                Some(provision.provider),
                Some("requester is not allowed to provision runtimes".to_string()),
                Value::Null,
            ));
            return CommandOutcome::error(
                statuses,
                AgentControlErrorEnvelope::v1(
                    request_id,
                    "provision_unauthorized",
                    Some(
                        "set PIKA_AGENT_CONTROL_PROVISION_ALLOWLIST to include this requester pubkey"
                            .to_string(),
                    ),
                    Some(format!("requester_pubkey={requester_pubkey_hex}")),
                ),
            );
        }
        statuses.push(AgentControlStatusEnvelope::v1(
            request_id.clone(),
            RuntimeLifecyclePhase::Provisioning,
            None,
            Some(provision.provider),
            Some("provisioning runtime".to_string()),
            json!({
                "provider": provider_name(provision.provider),
                "protocol": protocol_name(provision.protocol),
            }),
        ));
        if let Some(requested_class) = provision.runtime_class.as_deref() {
            let advertised_class = runtime_profile(provision.provider)
                .runtime_class
                .unwrap_or_else(|| provider_name(provision.provider).to_string());
            if requested_class != advertised_class {
                statuses.push(AgentControlStatusEnvelope::v1(
                    request_id.clone(),
                    RuntimeLifecyclePhase::Failed,
                    None,
                    Some(provision.provider),
                    Some("requested runtime class is not available on this server".to_string()),
                    json!({
                        "requested_runtime_class": requested_class,
                        "available_runtime_class": advertised_class,
                    }),
                ));
                return CommandOutcome::error(
                    statuses,
                    AgentControlErrorEnvelope::v1(
                        request_id,
                        "runtime_class_unavailable",
                        Some(
                            "route this command to a server that advertises the requested class"
                                .to_string(),
                        ),
                        Some(format!(
                            "requested={}, available={}",
                            requested_class, advertised_class
                        )),
                    ),
                );
            }
        }

        let runtime_id = new_runtime_id(provision.provider);
        let adapter = self.adapter_for(provision.provider);
        let provisioned = match adapter
            .provision(&runtime_id, requester_pubkey, &provision)
            .await
        {
            Ok(outcome) => outcome,
            Err(err) => {
                statuses.push(AgentControlStatusEnvelope::v1(
                    request_id.clone(),
                    RuntimeLifecyclePhase::Failed,
                    Some(runtime_id),
                    Some(provision.provider),
                    Some("provisioning failed".to_string()),
                    Value::Null,
                ));
                return CommandOutcome::error(
                    statuses,
                    AgentControlErrorEnvelope::v1(
                        request_id,
                        "provision_failed",
                        Some("check provider credentials/config and retry".to_string()),
                        Some(format!("{err:#}")),
                    ),
                );
            }
        };
        let descriptor = RuntimeDescriptor {
            runtime_id: runtime_id.clone(),
            provider: provision.provider,
            lifecycle_phase: RuntimeLifecyclePhase::Ready,
            runtime_class: provisioned.runtime_class.clone(),
            region: provisioned.region.clone(),
            capacity: provisioned.capacity.clone(),
            policy_constraints: provisioned.policy_constraints.clone(),
            protocol_compatibility: provisioned.protocol_compatibility.clone(),
            bot_pubkey: provisioned.bot_pubkey.clone(),
            metadata: provisioned.metadata.clone(),
        };

        let runtime_record = RuntimeRecord {
            owner_pubkey_hex: requester_pubkey_hex.to_string(),
            descriptor: descriptor.clone(),
            provider_handle: provisioned.provider_handle,
        };

        if !descriptor
            .protocol_compatibility
            .contains(&provision.protocol)
        {
            statuses.push(AgentControlStatusEnvelope::v1(
                request_id.clone(),
                RuntimeLifecyclePhase::Failed,
                Some(runtime_id),
                Some(provision.provider),
                Some("requested protocol is not supported by runtime".to_string()),
                json!({
                    "requested_protocol": protocol_name(provision.protocol),
                }),
            ));
            let cleanup_outcome = match adapter.teardown(&runtime_record).await {
                Ok(payload) => format!("provider cleanup attempted: {payload}"),
                Err(cleanup_err) => {
                    format!("provider cleanup failed: {cleanup_err:#}")
                }
            };
            return CommandOutcome::error(
                statuses,
                AgentControlErrorEnvelope::v1(
                    request_id,
                    "unsupported_protocol",
                    Some("choose a compatible runtime protocol".to_string()),
                    Some(format!(
                        "requested={}, compatibility={:?}; {cleanup_outcome}",
                        protocol_name(provision.protocol),
                        descriptor.protocol_compatibility
                    )),
                ),
            );
        }
        if let Some(requested_class) = provision.runtime_class.as_deref() {
            if descriptor.runtime_class.as_deref() != Some(requested_class) {
                let available = descriptor
                    .runtime_class
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                statuses.push(AgentControlStatusEnvelope::v1(
                    request_id.clone(),
                    RuntimeLifecyclePhase::Failed,
                    Some(runtime_id),
                    Some(provision.provider),
                    Some("requested runtime class is not available on this server".to_string()),
                    json!({
                        "requested_runtime_class": requested_class,
                        "available_runtime_class": available,
                    }),
                ));
                let cleanup_outcome = match adapter.teardown(&runtime_record).await {
                    Ok(payload) => format!("provider cleanup attempted: {payload}"),
                    Err(cleanup_err) => {
                        format!("provider cleanup failed: {cleanup_err:#}")
                    }
                };
                return CommandOutcome::error(
                    statuses,
                    AgentControlErrorEnvelope::v1(
                        request_id,
                        "runtime_class_unavailable",
                        Some(
                            "route this command to a server that advertises the requested class"
                                .to_string(),
                        ),
                        Some(format!(
                            "requested={}, available={}; {cleanup_outcome}",
                            requested_class, available,
                        )),
                    ),
                );
            }
        }

        {
            let mut state = self.state.write().await;
            state
                .runtimes
                .insert(runtime_id.clone(), runtime_record.clone());
            if let Err(err) = self.persist_state_snapshot(&state) {
                state.runtimes.remove(&runtime_id);
                let rollback_err = self.persist_state_snapshot(&state).err();
                drop(state);

                let cleanup_outcome = match adapter.teardown(&runtime_record).await {
                    Ok(payload) => format!("provider rollback attempted: {payload}"),
                    Err(cleanup_err) => {
                        format!("provider rollback failed: {cleanup_err:#}")
                    }
                };
                let mut detail = format!("{err:#}; {cleanup_outcome}");
                if let Some(rollback_err) = rollback_err {
                    detail.push_str(&format!(
                        "; rollback state persist failed: {rollback_err:#}"
                    ));
                }
                return CommandOutcome::error(
                    statuses,
                    AgentControlErrorEnvelope::v1(
                        request_id,
                        "state_persist_failed",
                        Some(
                            "runtime provisioning was rolled back due to server state persistence failure"
                                .to_string(),
                        ),
                        Some(detail),
                    ),
                );
            }
        }

        statuses.push(AgentControlStatusEnvelope::v1(
            request_id.clone(),
            RuntimeLifecyclePhase::Ready,
            Some(runtime_id.clone()),
            Some(provision.provider),
            Some("runtime ready".to_string()),
            json!({
                "runtime_id": runtime_id,
                "provider": provider_name(provision.provider),
            }),
        ));

        CommandOutcome::result(
            statuses,
            AgentControlResultEnvelope::v1(
                request_id,
                descriptor,
                json!({
                    "operation": "provision",
                }),
            ),
        )
    }

    async fn handle_process_welcome(
        &self,
        requester_pubkey_hex: &str,
        request_id: String,
        process_welcome: ProcessWelcomeCommand,
        mut statuses: Vec<AgentControlStatusEnvelope>,
    ) -> CommandOutcome {
        let runtime = {
            let state = self.state.read().await;
            state.runtimes.get(&process_welcome.runtime_id).cloned()
        };
        let Some(runtime) = runtime else {
            return CommandOutcome::error(
                statuses,
                AgentControlErrorEnvelope::v1(
                    request_id,
                    "runtime_not_found",
                    Some("runtime id is unknown to this server".to_string()),
                    Some(process_welcome.runtime_id),
                ),
            );
        };
        if !self.can_access_runtime(requester_pubkey_hex, &runtime) {
            return CommandOutcome::error(
                statuses,
                AgentControlErrorEnvelope::v1(
                    request_id,
                    "runtime_not_found",
                    Some("runtime id is unknown to this server".to_string()),
                    Some(process_welcome.runtime_id),
                ),
            );
        }

        statuses.push(AgentControlStatusEnvelope::v1(
            request_id.clone(),
            RuntimeLifecyclePhase::Provisioning,
            Some(runtime.descriptor.runtime_id.clone()),
            Some(runtime.descriptor.provider),
            Some("processing welcome".to_string()),
            Value::Null,
        ));
        let adapter = self.adapter_for(runtime.descriptor.provider);
        let payload = match adapter
            .process_welcome(&runtime, &process_welcome)
            .await
            .with_context(|| "provider process_welcome call failed")
        {
            Ok(payload) => payload,
            Err(err) => {
                statuses.push(AgentControlStatusEnvelope::v1(
                    request_id.clone(),
                    RuntimeLifecyclePhase::Failed,
                    Some(runtime.descriptor.runtime_id.clone()),
                    Some(runtime.descriptor.provider),
                    Some("process_welcome failed".to_string()),
                    Value::Null,
                ));
                return CommandOutcome::error(
                    statuses,
                    AgentControlErrorEnvelope::v1(
                        request_id,
                        "process_welcome_failed",
                        Some("check provider runtime state and welcome payload".to_string()),
                        Some(format!("{err:#}")),
                    ),
                );
            }
        };
        statuses.push(AgentControlStatusEnvelope::v1(
            request_id.clone(),
            RuntimeLifecyclePhase::Ready,
            Some(runtime.descriptor.runtime_id.clone()),
            Some(runtime.descriptor.provider),
            Some("welcome processed".to_string()),
            Value::Null,
        ));
        CommandOutcome::result(
            statuses,
            AgentControlResultEnvelope::v1(request_id, runtime.descriptor, payload),
        )
    }

    async fn handle_teardown(
        &self,
        requester_pubkey_hex: &str,
        request_id: String,
        teardown: TeardownCommand,
        mut statuses: Vec<AgentControlStatusEnvelope>,
    ) -> CommandOutcome {
        let runtime = {
            let state = self.state.read().await;
            state.runtimes.get(&teardown.runtime_id).cloned()
        };
        let Some(mut runtime) = runtime else {
            return CommandOutcome::error(
                statuses,
                AgentControlErrorEnvelope::v1(
                    request_id,
                    "runtime_not_found",
                    Some("runtime id is unknown to this server".to_string()),
                    Some(teardown.runtime_id),
                ),
            );
        };
        if !self.can_access_runtime(requester_pubkey_hex, &runtime) {
            return CommandOutcome::error(
                statuses,
                AgentControlErrorEnvelope::v1(
                    request_id,
                    "runtime_not_found",
                    Some("runtime id is unknown to this server".to_string()),
                    Some(teardown.runtime_id),
                ),
            );
        }

        statuses.push(AgentControlStatusEnvelope::v1(
            request_id.clone(),
            RuntimeLifecyclePhase::Teardown,
            Some(runtime.descriptor.runtime_id.clone()),
            Some(runtime.descriptor.provider),
            Some("teardown in progress".to_string()),
            Value::Null,
        ));
        let adapter = self.adapter_for(runtime.descriptor.provider);
        let mut payload = match adapter.teardown(&runtime).await {
            Ok(payload) => payload,
            Err(err) => {
                return CommandOutcome::error(
                    statuses,
                    AgentControlErrorEnvelope::v1(
                        request_id,
                        "teardown_failed",
                        Some("manual cleanup may be required".to_string()),
                        Some(format!("{err:#}")),
                    ),
                );
            }
        };
        runtime.descriptor.lifecycle_phase = RuntimeLifecyclePhase::Teardown;
        {
            let mut state = self.state.write().await;
            state
                .runtimes
                .insert(runtime.descriptor.runtime_id.clone(), runtime.clone());
            if let Err(err) = self.persist_state_snapshot(&state) {
                warn!(
                    error = %err,
                    runtime_id = %runtime.descriptor.runtime_id,
                    "teardown completed but state persistence failed; continuing with in-memory teardown state"
                );
                statuses.push(AgentControlStatusEnvelope::v1(
                    request_id.clone(),
                    RuntimeLifecyclePhase::Teardown,
                    Some(runtime.descriptor.runtime_id.clone()),
                    Some(runtime.descriptor.provider),
                    Some("teardown completed but state persistence failed".to_string()),
                    Value::Null,
                ));
                if let Value::Object(ref mut map) = payload {
                    map.insert("state_persist".to_string(), json!("failed"));
                    map.insert("state_persist_error".to_string(), json!(format!("{err:#}")));
                } else {
                    payload = json!({
                        "provider_payload": payload,
                        "state_persist": "failed",
                        "state_persist_error": format!("{err:#}"),
                    });
                }
            }
        }
        CommandOutcome::result(
            statuses,
            AgentControlResultEnvelope::v1(request_id, runtime.descriptor, payload),
        )
    }

    async fn handle_get_runtime(
        &self,
        requester_pubkey_hex: &str,
        request_id: String,
        get_runtime: GetRuntimeCommand,
        statuses: Vec<AgentControlStatusEnvelope>,
    ) -> CommandOutcome {
        let runtime = {
            let state = self.state.read().await;
            state.runtimes.get(&get_runtime.runtime_id).cloned()
        };
        let Some(runtime) = runtime else {
            return CommandOutcome::error(
                statuses,
                AgentControlErrorEnvelope::v1(
                    request_id,
                    "runtime_not_found",
                    Some("runtime id is unknown to this server".to_string()),
                    Some(get_runtime.runtime_id),
                ),
            );
        };
        if !self.can_access_runtime(requester_pubkey_hex, &runtime) {
            return CommandOutcome::error(
                statuses,
                AgentControlErrorEnvelope::v1(
                    request_id,
                    "runtime_not_found",
                    Some("runtime id is unknown to this server".to_string()),
                    Some(get_runtime.runtime_id),
                ),
            );
        }
        CommandOutcome::result(
            statuses,
            AgentControlResultEnvelope::v1(
                request_id,
                runtime.descriptor,
                json!({"operation":"get_runtime"}),
            ),
        )
    }

    async fn handle_list_runtimes(
        &self,
        requester_pubkey_hex: &str,
        request_id: String,
        list: ListRuntimesCommand,
        statuses: Vec<AgentControlStatusEnvelope>,
    ) -> CommandOutcome {
        let runtimes: Vec<RuntimeDescriptor> = {
            let state = self.state.read().await;
            state
                .runtimes
                .values()
                .filter(|runtime| self.can_access_runtime(requester_pubkey_hex, runtime))
                .map(|r| r.descriptor.clone())
                .collect()
        };
        let mut filtered: Vec<RuntimeDescriptor> = runtimes
            .into_iter()
            .filter(|descriptor| {
                if let Some(provider) = list.provider {
                    if descriptor.provider != provider {
                        return false;
                    }
                }
                if let Some(protocol) = list.protocol {
                    if !descriptor.protocol_compatibility.contains(&protocol) {
                        return false;
                    }
                }
                if let Some(phase) = list.lifecycle_phase {
                    if descriptor.lifecycle_phase != phase {
                        return false;
                    }
                }
                if let Some(requested_class) = list.runtime_class.as_deref() {
                    if descriptor.runtime_class.as_deref() != Some(requested_class) {
                        return false;
                    }
                }
                true
            })
            .collect();
        filtered.sort_by(|a, b| a.runtime_id.cmp(&b.runtime_id));
        if let Some(limit) = list.limit {
            filtered.truncate(limit);
        }
        let summary = filtered
            .first()
            .cloned()
            .unwrap_or_else(default_list_summary_descriptor);
        CommandOutcome::result(
            statuses,
            AgentControlResultEnvelope::v1(
                request_id,
                summary,
                json!({
                    "operation":"list_runtimes",
                    "count": filtered.len(),
                    "runtimes": filtered,
                }),
            ),
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RuntimeRecord {
    #[serde(default)]
    owner_pubkey_hex: String,
    descriptor: RuntimeDescriptor,
    provider_handle: ProviderHandle,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum ProviderHandle {
    Fly {
        machine_id: String,
        volume_id: String,
        app_name: String,
    },
    Microvm {
        vm_id: String,
        spawner_url: String,
        keep: bool,
    },
}

#[derive(Default)]
struct ControlState {
    runtimes: HashMap<String, RuntimeRecord>,
    idempotency: HashMap<(String, String), CachedTerminal>,
    idempotency_order: VecDeque<(String, String)>,
}

impl ControlState {
    fn insert_idempotency(
        &mut self,
        key: (String, String),
        terminal: CachedTerminal,
        max_entries: usize,
    ) {
        if self.idempotency.contains_key(&key) {
            self.idempotency_order.retain(|existing| existing != &key);
        }
        self.idempotency.insert(key.clone(), terminal);
        self.idempotency_order.push_back(key);
        while self.idempotency_order.len() > max_entries {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }

    fn truncate_idempotency(&mut self, max_entries: usize) {
        while self.idempotency_order.len() > max_entries {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct PersistedControlState {
    runtimes: HashMap<String, RuntimeRecord>,
    idempotency: Vec<PersistedIdempotencyEntry>,
}

#[derive(Serialize, Deserialize)]
struct PersistedIdempotencyEntry {
    requester_pubkey_hex: String,
    idempotency_key: String,
    terminal: CachedTerminal,
}

impl From<PersistedControlState> for ControlState {
    fn from(value: PersistedControlState) -> Self {
        let mut state = Self {
            runtimes: value.runtimes,
            ..Self::default()
        };
        for entry in value.idempotency {
            state.insert_idempotency(
                (entry.requester_pubkey_hex, entry.idempotency_key),
                entry.terminal,
                usize::MAX,
            );
        }
        state
    }
}

impl From<&ControlState> for PersistedControlState {
    fn from(value: &ControlState) -> Self {
        let mut idempotency = Vec::new();
        for (requester_pubkey_hex, idempotency_key) in &value.idempotency_order {
            let Some(terminal) = value
                .idempotency
                .get(&(requester_pubkey_hex.clone(), idempotency_key.clone()))
            else {
                continue;
            };
            idempotency.push(PersistedIdempotencyEntry {
                requester_pubkey_hex: requester_pubkey_hex.clone(),
                idempotency_key: idempotency_key.clone(),
                terminal: terminal.clone(),
            });
        }
        Self {
            runtimes: value.runtimes.clone(),
            idempotency,
        }
    }
}

#[derive(Clone, Debug)]
struct ControlStatePersistence {
    path: PathBuf,
}

impl ControlStatePersistence {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn load(&self) -> anyhow::Result<ControlState> {
        if !self.path.exists() {
            return Ok(ControlState::default());
        }
        let data = std::fs::read_to_string(&self.path)
            .with_context(|| format!("read control state {}", self.path.display()))?;
        if data.trim().is_empty() {
            return Ok(ControlState::default());
        }
        let mut raw: Value = match serde_json::from_str(&data) {
            Ok(raw) => raw,
            Err(err) => {
                warn!(
                    error = %err,
                    path = %self.path.display(),
                    "failed to decode control state; starting with empty state"
                );
                return Ok(ControlState::default());
            }
        };
        let migrated_protocol_values = migrate_legacy_protocol_values(&mut raw);
        if migrated_protocol_values > 0 {
            warn!(
                count = migrated_protocol_values,
                path = %self.path.display(),
                "migrated legacy protocol values to acp while loading control state"
            );
        }
        let persisted: PersistedControlState = match serde_json::from_value(raw) {
            Ok(persisted) => persisted,
            Err(err) => {
                warn!(
                    error = %err,
                    path = %self.path.display(),
                    "failed to decode control state; starting with empty state"
                );
                return Ok(ControlState::default());
            }
        };
        let state: ControlState = persisted.into();
        let legacy_ownerless = state
            .runtimes
            .values()
            .filter(|runtime| runtime.owner_pubkey_hex.is_empty())
            .count();
        if legacy_ownerless > 0 {
            warn!(
                count = legacy_ownerless,
                path = %self.path.display(),
                "loaded legacy runtimes without owner pubkeys; access is limited to provisioning-allowed requesters"
            );
        }
        Ok(state)
    }

    fn save(&self, state: &ControlState) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("create control state directory {}", parent.display())
                })?;
            }
        }
        let persisted = PersistedControlState::from(state);
        let serialized =
            serde_json::to_string_pretty(&persisted).context("encode control state json")?;
        let tmp_path = self.path.with_extension("tmp");
        std::fs::write(&tmp_path, serialized)
            .with_context(|| format!("write control state {}", tmp_path.display()))?;
        std::fs::rename(&tmp_path, &self.path)
            .with_context(|| format!("persist control state {}", self.path.display()))?;
        Ok(())
    }
}

fn migrate_legacy_protocol_values(root: &mut Value) -> usize {
    let Some(runtimes) = root.get_mut("runtimes").and_then(Value::as_object_mut) else {
        return 0;
    };
    let mut migrated = 0usize;
    for runtime in runtimes.values_mut() {
        let Some(descriptor) = runtime.get_mut("descriptor").and_then(Value::as_object_mut) else {
            continue;
        };
        migrated += migrate_legacy_descriptor_protocols(descriptor);
    }
    migrated
}

fn migrate_legacy_descriptor_protocols(descriptor: &mut serde_json::Map<String, Value>) -> usize {
    let mut migrated = 0usize;
    if let Some(protocols) = descriptor
        .get_mut("protocol_compatibility")
        .and_then(Value::as_array_mut)
    {
        for protocol in protocols.iter_mut() {
            let Some(raw) = protocol.as_str() else {
                continue;
            };
            let Some(normalized) = normalize_legacy_protocol_name(raw) else {
                continue;
            };
            if raw != normalized {
                *protocol = Value::String(normalized.to_string());
                migrated += 1;
            }
        }
        return migrated;
    }

    let Some(raw) = descriptor
        .get("protocol")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return migrated;
    };
    let Some(normalized) = normalize_legacy_protocol_name(&raw) else {
        return migrated;
    };
    descriptor.insert(
        "protocol_compatibility".to_string(),
        Value::Array(vec![Value::String(normalized.to_string())]),
    );
    if raw != normalized {
        migrated += 1;
    }
    migrated
}

fn normalize_legacy_protocol_name(raw: &str) -> Option<&'static str> {
    if raw.eq_ignore_ascii_case("acp") || raw.eq_ignore_ascii_case("pi") {
        Some("acp")
    } else {
        None
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum CachedTerminal {
    Result {
        provider: ProviderKind,
        runtime_id: String,
        runtime: Box<RuntimeDescriptor>,
        payload: Value,
    },
    Error {
        provider: Option<ProviderKind>,
        code: String,
        hint: Option<String>,
        detail: Option<String>,
    },
}

impl CachedTerminal {
    fn provider(&self) -> Option<ProviderKind> {
        match self {
            Self::Result { provider, .. } => Some(*provider),
            Self::Error { provider, .. } => *provider,
        }
    }

    fn runtime_id(&self) -> Option<String> {
        match self {
            Self::Result { runtime_id, .. } => Some(runtime_id.clone()),
            Self::Error { .. } => None,
        }
    }

    fn to_outcome(
        &self,
        statuses: Vec<AgentControlStatusEnvelope>,
        request_id: String,
    ) -> CommandOutcome {
        match self {
            Self::Result {
                runtime, payload, ..
            } => CommandOutcome::result(
                statuses,
                AgentControlResultEnvelope::v1(
                    request_id,
                    runtime.as_ref().clone(),
                    payload.clone(),
                ),
            ),
            Self::Error {
                code, hint, detail, ..
            } => CommandOutcome::error(
                statuses,
                AgentControlErrorEnvelope::v1(
                    request_id,
                    code.clone(),
                    hint.clone(),
                    detail.clone(),
                ),
            ),
        }
    }
}

struct CommandOutcome {
    statuses: Vec<AgentControlStatusEnvelope>,
    result: Option<AgentControlResultEnvelope>,
    error: Option<AgentControlErrorEnvelope>,
}

impl CommandOutcome {
    fn result(
        statuses: Vec<AgentControlStatusEnvelope>,
        result: AgentControlResultEnvelope,
    ) -> Self {
        Self {
            statuses,
            result: Some(result),
            error: None,
        }
    }

    fn error(statuses: Vec<AgentControlStatusEnvelope>, error: AgentControlErrorEnvelope) -> Self {
        Self {
            statuses,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Clone, Debug)]
struct RuntimeProfile {
    runtime_class: Option<String>,
    region: Option<String>,
    capacity: Value,
    policy_constraints: Value,
}

fn runtime_profile(provider: ProviderKind) -> RuntimeProfile {
    RuntimeProfile {
        runtime_class: env_string_for_provider(provider, "PIKA_AGENT_RUNTIME_CLASS")
            .or_else(|| Some(provider_name(provider).to_string())),
        region: env_string_for_provider(provider, "PIKA_AGENT_RUNTIME_REGION"),
        capacity: env_json_for_provider(provider, "PIKA_AGENT_RUNTIME_CAPACITY_JSON"),
        policy_constraints: env_json_for_provider(provider, "PIKA_AGENT_RUNTIME_POLICY_JSON"),
    }
}

fn env_string_for_provider(provider: ProviderKind, key: &str) -> Option<String> {
    let provider_key = format!("{key}_{}", provider_name(provider).to_ascii_uppercase());
    std::env::var(provider_key)
        .ok()
        .or_else(|| std::env::var(key).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn env_json_for_provider(provider: ProviderKind, key: &str) -> Value {
    let provider_key = format!("{key}_{}", provider_name(provider).to_ascii_uppercase());
    let raw = std::env::var(provider_key)
        .ok()
        .or_else(|| std::env::var(key).ok());
    match raw {
        Some(value) if !value.trim().is_empty() => match serde_json::from_str::<Value>(&value) {
            Ok(parsed) => parsed,
            Err(_) => json!({"raw": value}),
        },
        _ => Value::Null,
    }
}

#[derive(Clone, Debug)]
struct ProvisionedRuntime {
    provider_handle: ProviderHandle,
    bot_pubkey: Option<String>,
    metadata: Value,
    runtime_class: Option<String>,
    region: Option<String>,
    capacity: Value,
    policy_constraints: Value,
    protocol_compatibility: Vec<ProtocolKind>,
}

#[async_trait]
trait ProviderAdapter: Send + Sync {
    async fn provision(
        &self,
        runtime_id: &str,
        owner_pubkey: PublicKey,
        provision: &ProvisionCommand,
    ) -> anyhow::Result<ProvisionedRuntime>;

    async fn process_welcome(
        &self,
        runtime: &RuntimeRecord,
        process_welcome: &ProcessWelcomeCommand,
    ) -> anyhow::Result<Value>;

    async fn teardown(&self, runtime: &RuntimeRecord) -> anyhow::Result<Value>;
}

#[derive(Clone)]
struct FlyAdapter;

#[async_trait]
impl ProviderAdapter for FlyAdapter {
    async fn provision(
        &self,
        _runtime_id: &str,
        _owner_pubkey: PublicKey,
        provision: &ProvisionCommand,
    ) -> anyhow::Result<ProvisionedRuntime> {
        let profile = runtime_profile(ProviderKind::Fly);
        let fly = FlyClient::from_env()?;
        let anthropic_key =
            std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY must be set")?;
        let openai_key = std::env::var("OPENAI_API_KEY").ok();
        let pi_model = std::env::var("PI_MODEL")
            .ok()
            .filter(|v| !v.trim().is_empty());

        let bot_keys = if let Some(secret) = provision.bot_secret_key_hex.as_deref() {
            Keys::parse(secret).context("parse bot_secret_key_hex")?
        } else {
            Keys::generate()
        };
        let bot_pubkey = bot_keys.public_key().to_hex();
        let bot_secret_hex = bot_keys.secret_key().to_secret_hex();

        let suffix = format!("{:08x}", rand::thread_rng().r#gen::<u32>());
        let volume_name = format!("agent_{suffix}");
        let machine_name = provision
            .name
            .clone()
            .unwrap_or_else(|| format!("agent-{suffix}"));

        let volume = fly.create_volume(&volume_name).await?;
        let mut env = HashMap::new();
        env.insert("STATE_DIR".to_string(), "/app/state".to_string());
        env.insert("NOSTR_SECRET_KEY".to_string(), bot_secret_hex);
        env.insert("ANTHROPIC_API_KEY".to_string(), anthropic_key);
        if let Some(openai) = openai_key {
            env.insert("OPENAI_API_KEY".to_string(), openai);
        }
        if let Some(model) = pi_model {
            env.insert("PI_MODEL".to_string(), model);
        }
        let machine = fly.create_machine(&machine_name, &volume.id, env).await?;

        Ok(ProvisionedRuntime {
            provider_handle: ProviderHandle::Fly {
                machine_id: machine.id.clone(),
                volume_id: volume.id.clone(),
                app_name: fly.app_name().to_string(),
            },
            bot_pubkey: Some(bot_pubkey),
            metadata: json!({
                "machine_id": machine.id,
                "volume_id": volume.id,
                "app_name": fly.app_name(),
                "runtime_class": profile.runtime_class.clone(),
                "region": profile.region.clone(),
            }),
            runtime_class: profile.runtime_class,
            region: profile.region,
            capacity: profile.capacity,
            policy_constraints: profile.policy_constraints,
            protocol_compatibility: vec![ProtocolKind::Acp],
        })
    }

    async fn process_welcome(
        &self,
        _runtime: &RuntimeRecord,
        _process_welcome: &ProcessWelcomeCommand,
    ) -> anyhow::Result<Value> {
        Ok(
            json!({"processed": false, "reason": "fly runtime does not require explicit welcome hook"}),
        )
    }

    async fn teardown(&self, runtime: &RuntimeRecord) -> anyhow::Result<Value> {
        let ProviderHandle::Fly {
            machine_id,
            volume_id,
            app_name,
        } = &runtime.provider_handle
        else {
            anyhow::bail!("fly adapter received non-fly runtime handle")
        };
        Ok(json!({
            "teardown": "manual",
            "machine_id": machine_id,
            "volume_id": volume_id,
            "app_name": app_name,
            "hint": format!("fly machine stop {} -a {}", machine_id, app_name),
        }))
    }
}

#[derive(Clone)]
struct MicrovmAdapter;

#[async_trait]
impl ProviderAdapter for MicrovmAdapter {
    async fn provision(
        &self,
        _runtime_id: &str,
        owner_pubkey: PublicKey,
        provision: &ProvisionCommand,
    ) -> anyhow::Result<ProvisionedRuntime> {
        let profile = runtime_profile(ProviderKind::Microvm);
        let params = provision.microvm.clone().unwrap_or_default();
        let resolved = resolve_params(&params, provision.keep);
        let relay_urls = if provision.relay_urls.is_empty() {
            default_relay_urls()
        } else {
            provision.relay_urls.clone()
        };

        let bot_keys = if let Some(secret) = provision.bot_secret_key_hex.as_deref() {
            Keys::parse(secret).context("parse bot_secret_key_hex")?
        } else {
            Keys::generate()
        };
        let bot_pubkey = bot_keys.public_key().to_hex();
        let bot_secret_hex = bot_keys.secret_key().to_secret_hex();

        let spawner = MicrovmSpawnerClient::new(resolved.spawner_url.clone());
        let create_vm = build_create_vm_request(
            &resolved,
            &owner_pubkey,
            &relay_urls,
            &bot_secret_hex,
            &bot_pubkey,
        );
        let vm = spawner
            .create_vm(&create_vm)
            .await
            .map_err(|err| spawner_create_error(&resolved.spawner_url, err))?;

        Ok(ProvisionedRuntime {
            provider_handle: ProviderHandle::Microvm {
                vm_id: vm.id.clone(),
                spawner_url: resolved.spawner_url.clone(),
                keep: resolved.keep,
            },
            bot_pubkey: Some(bot_pubkey),
            metadata: json!({
                "vm_id": vm.id,
                "vm_ip": vm.ip,
                "spawner_url": resolved.spawner_url,
                "keep": resolved.keep,
                "runtime_class": profile.runtime_class.clone(),
                "region": profile.region.clone(),
            }),
            runtime_class: profile.runtime_class,
            region: profile.region,
            capacity: profile.capacity,
            policy_constraints: profile.policy_constraints,
            protocol_compatibility: vec![ProtocolKind::Acp],
        })
    }

    async fn process_welcome(
        &self,
        _runtime: &RuntimeRecord,
        _process_welcome: &ProcessWelcomeCommand,
    ) -> anyhow::Result<Value> {
        Ok(
            json!({"processed": false, "reason": "microvm runtime receives welcome through relay flow"}),
        )
    }

    async fn teardown(&self, runtime: &RuntimeRecord) -> anyhow::Result<Value> {
        let ProviderHandle::Microvm {
            vm_id,
            spawner_url,
            keep,
        } = &runtime.provider_handle
        else {
            anyhow::bail!("microvm adapter received non-microvm runtime handle")
        };
        if *keep {
            return Ok(json!({
                "teardown": "skipped",
                "vm_id": vm_id,
                "spawner_url": spawner_url,
                "reason": "--keep policy",
            }));
        }
        let spawner = MicrovmSpawnerClient::new(spawner_url.clone());
        spawner.delete_vm(vm_id).await?;
        Ok(json!({
            "teardown": "deleted",
            "vm_id": vm_id,
            "spawner_url": spawner_url,
        }))
    }
}

fn default_relay_urls() -> Vec<String> {
    default_message_relays()
}

fn default_list_summary_descriptor() -> RuntimeDescriptor {
    RuntimeDescriptor {
        runtime_id: "runtime-list-summary".to_string(),
        provider: ProviderKind::Fly,
        lifecycle_phase: RuntimeLifecyclePhase::Ready,
        runtime_class: None,
        region: None,
        capacity: Value::Null,
        policy_constraints: Value::Null,
        protocol_compatibility: vec![],
        bot_pubkey: None,
        metadata: json!({"summary": true}),
    }
}

fn provider_name(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Fly => "fly",
        ProviderKind::Microvm => "microvm",
    }
}

fn protocol_name(_protocol: ProtocolKind) -> &'static str {
    "acp"
}

fn should_cache_success_result(command: &AgentControlCommand) -> bool {
    matches!(
        command,
        AgentControlCommand::Provision(_)
            | AgentControlCommand::ProcessWelcome(_)
            | AgentControlCommand::Teardown(_)
    )
}

fn new_runtime_id(provider: ProviderKind) -> String {
    let mut rng = rand::thread_rng();
    format!(
        "{}-{:08x}{:08x}",
        provider_name(provider),
        rng.r#gen::<u32>(),
        rng.r#gen::<u32>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pika_agent_control_plane::AuthContext;

    #[derive(Clone)]
    struct MockAdapter {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl ProviderAdapter for MockAdapter {
        async fn provision(
            &self,
            runtime_id: &str,
            _owner_pubkey: PublicKey,
            _provision: &ProvisionCommand,
        ) -> anyhow::Result<ProvisionedRuntime> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ProvisionedRuntime {
                provider_handle: ProviderHandle::Fly {
                    machine_id: "machine-1".to_string(),
                    volume_id: "volume-1".to_string(),
                    app_name: "app".to_string(),
                },
                bot_pubkey: Some("ab".repeat(32)),
                metadata: json!({"runtime_id": runtime_id, "mock": true}),
                runtime_class: Some("mock".to_string()),
                region: Some("local".to_string()),
                capacity: json!({"slots": 1}),
                policy_constraints: json!({"allow_keep": true}),
                protocol_compatibility: vec![ProtocolKind::Acp],
            })
        }

        async fn process_welcome(
            &self,
            _runtime: &RuntimeRecord,
            _process_welcome: &ProcessWelcomeCommand,
        ) -> anyhow::Result<Value> {
            Ok(json!({"ok": true}))
        }

        async fn teardown(&self, _runtime: &RuntimeRecord) -> anyhow::Result<Value> {
            Ok(json!({"ok": true}))
        }
    }

    #[derive(Clone)]
    struct CountingFailingAdapter {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl ProviderAdapter for CountingFailingAdapter {
        async fn provision(
            &self,
            _runtime_id: &str,
            _owner_pubkey: PublicKey,
            _provision: &ProvisionCommand,
        ) -> anyhow::Result<ProvisionedRuntime> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            anyhow::bail!("simulated provision failure")
        }

        async fn process_welcome(
            &self,
            _runtime: &RuntimeRecord,
            _process_welcome: &ProcessWelcomeCommand,
        ) -> anyhow::Result<Value> {
            Ok(json!({"ok": true}))
        }

        async fn teardown(&self, _runtime: &RuntimeRecord) -> anyhow::Result<Value> {
            Ok(json!({"ok": true}))
        }
    }

    #[derive(Clone)]
    struct ValidationMismatchAdapter {
        provision_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        teardown_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        runtime_class: Option<String>,
        protocol_compatibility: Vec<ProtocolKind>,
    }

    #[async_trait]
    impl ProviderAdapter for ValidationMismatchAdapter {
        async fn provision(
            &self,
            runtime_id: &str,
            _owner_pubkey: PublicKey,
            _provision: &ProvisionCommand,
        ) -> anyhow::Result<ProvisionedRuntime> {
            self.provision_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ProvisionedRuntime {
                provider_handle: ProviderHandle::Fly {
                    machine_id: "machine-1".to_string(),
                    volume_id: "volume-1".to_string(),
                    app_name: "app".to_string(),
                },
                bot_pubkey: Some("ab".repeat(32)),
                metadata: json!({"runtime_id": runtime_id, "mock": true}),
                runtime_class: self.runtime_class.clone(),
                region: Some("local".to_string()),
                capacity: json!({"slots": 1}),
                policy_constraints: json!({"allow_keep": true}),
                protocol_compatibility: self.protocol_compatibility.clone(),
            })
        }

        async fn process_welcome(
            &self,
            _runtime: &RuntimeRecord,
            _process_welcome: &ProcessWelcomeCommand,
        ) -> anyhow::Result<Value> {
            Ok(json!({"ok": true}))
        }

        async fn teardown(&self, _runtime: &RuntimeRecord) -> anyhow::Result<Value> {
            self.teardown_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(json!({"cleanup": true}))
        }
    }

    fn request_with(
        request_id: &str,
        idempotency_key: &str,
        command: AgentControlCommand,
    ) -> AgentControlCmdEnvelope {
        AgentControlCmdEnvelope::v1(
            request_id.to_string(),
            idempotency_key.to_string(),
            command,
            AuthContext::default(),
        )
    }

    fn request(command: AgentControlCommand) -> AgentControlCmdEnvelope {
        request_with("req-1", "idem-1", command)
    }

    fn unique_temp_path(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            rand::thread_rng().r#gen::<u64>()
        ))
    }

    #[tokio::test]
    async fn idempotency_replay_does_not_reprovision() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter);
        let requester = Keys::generate().public_key();

        let first = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request(AgentControlCommand::Provision(ProvisionCommand {
                    provider: ProviderKind::Fly,
                    protocol: ProtocolKind::Acp,
                    name: None,
                    runtime_class: None,
                    relay_urls: vec![],
                    keep: false,
                    bot_secret_key_hex: None,
                    microvm: None,
                })),
            )
            .await;
        assert!(first.result.is_some());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        let replay = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request(AgentControlCommand::Provision(ProvisionCommand {
                    provider: ProviderKind::Fly,
                    protocol: ProtocolKind::Acp,
                    name: None,
                    runtime_class: None,
                    relay_urls: vec![],
                    keep: false,
                    bot_secret_key_hex: None,
                    microvm: None,
                })),
            )
            .await;
        assert!(replay.result.is_some());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(replay
            .statuses
            .iter()
            .any(|status| status.message.as_deref() == Some("idempotent replay")));
    }

    #[tokio::test]
    async fn get_runtime_returns_not_found_before_provision() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter);
        let requester = Keys::generate().public_key();
        let out = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request(AgentControlCommand::GetRuntime(GetRuntimeCommand {
                    runtime_id: "does-not-exist".to_string(),
                })),
            )
            .await;
        assert!(out.result.is_none());
        let err = out.error.expect("expected not found error");
        assert_eq!(err.code, "runtime_not_found");
    }

    #[tokio::test]
    async fn list_runtimes_supports_filters() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter);
        let requester = Keys::generate().public_key();

        for (req_id, idem, provider) in [
            ("req-1", "idem-1", ProviderKind::Fly),
            ("req-2", "idem-2", ProviderKind::Microvm),
        ] {
            let out = service
                .handle_command(
                    &requester.to_hex(),
                    requester,
                    request_with(
                        req_id,
                        idem,
                        AgentControlCommand::Provision(ProvisionCommand {
                            provider,
                            protocol: ProtocolKind::Acp,
                            name: None,
                            runtime_class: None,
                            relay_urls: vec![],
                            keep: false,
                            bot_secret_key_hex: None,
                            microvm: None,
                        }),
                    ),
                )
                .await;
            assert!(out.result.is_some());
        }

        let out = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request_with(
                    "req-list",
                    "idem-list",
                    AgentControlCommand::ListRuntimes(ListRuntimesCommand {
                        provider: Some(ProviderKind::Microvm),
                        protocol: Some(ProtocolKind::Acp),
                        lifecycle_phase: Some(RuntimeLifecyclePhase::Ready),
                        runtime_class: Some("mock".to_string()),
                        limit: Some(10),
                    }),
                ),
            )
            .await;
        let result = out.result.expect("list result");
        let runtimes = result
            .payload
            .get("runtimes")
            .and_then(Value::as_array)
            .cloned()
            .expect("runtimes array");
        assert_eq!(runtimes.len(), 1);
        let provider = runtimes[0]
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or("");
        assert_eq!(provider, "microvm");
    }

    #[tokio::test]
    async fn runtime_class_mismatch_fails_before_provision() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter);
        let requester = Keys::generate().public_key();

        let out = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request(AgentControlCommand::Provision(ProvisionCommand {
                    provider: ProviderKind::Fly,
                    protocol: ProtocolKind::Acp,
                    name: None,
                    runtime_class: Some("not-fly".to_string()),
                    relay_urls: vec![],
                    keep: false,
                    bot_secret_key_hex: None,
                    microvm: None,
                })),
            )
            .await;
        assert!(out.result.is_none());
        let err = out.error.expect("runtime class mismatch");
        assert_eq!(err.code, "runtime_class_unavailable");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn post_provision_protocol_validation_triggers_cleanup() {
        let provision_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let teardown_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(ValidationMismatchAdapter {
            provision_calls: provision_calls.clone(),
            teardown_calls: teardown_calls.clone(),
            runtime_class: runtime_profile(ProviderKind::Fly).runtime_class,
            protocol_compatibility: vec![],
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter);
        let requester = Keys::generate().public_key();

        let out = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request(AgentControlCommand::Provision(ProvisionCommand {
                    provider: ProviderKind::Fly,
                    protocol: ProtocolKind::Acp,
                    name: None,
                    runtime_class: None,
                    relay_urls: vec![],
                    keep: false,
                    bot_secret_key_hex: None,
                    microvm: None,
                })),
            )
            .await;
        assert!(out.result.is_none());
        let err = out.error.expect("protocol mismatch error");
        assert_eq!(err.code, "unsupported_protocol");
        assert_eq!(provision_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(teardown_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let state = service.state.read().await;
        assert!(state.runtimes.is_empty());
    }

    #[tokio::test]
    async fn post_provision_runtime_class_validation_triggers_cleanup() {
        let provision_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let teardown_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requested_class = runtime_profile(ProviderKind::Fly)
            .runtime_class
            .unwrap_or_else(|| "fly".to_string());
        let adapter = std::sync::Arc::new(ValidationMismatchAdapter {
            provision_calls: provision_calls.clone(),
            teardown_calls: teardown_calls.clone(),
            runtime_class: Some(format!("{requested_class}-actual")),
            protocol_compatibility: vec![ProtocolKind::Acp],
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter);
        let requester = Keys::generate().public_key();

        let out = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request(AgentControlCommand::Provision(ProvisionCommand {
                    provider: ProviderKind::Fly,
                    protocol: ProtocolKind::Acp,
                    name: None,
                    runtime_class: Some(requested_class),
                    relay_urls: vec![],
                    keep: false,
                    bot_secret_key_hex: None,
                    microvm: None,
                })),
            )
            .await;
        assert!(out.result.is_none());
        let err = out.error.expect("runtime class mismatch error");
        assert_eq!(err.code, "runtime_class_unavailable");
        assert_eq!(provision_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(teardown_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let state = service.state.read().await;
        assert!(state.runtimes.is_empty());
    }

    #[tokio::test]
    async fn get_runtime_is_scoped_to_owner() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter);
        let owner = Keys::generate().public_key();
        let other = Keys::generate().public_key();

        let provision = service
            .handle_command(
                &owner.to_hex(),
                owner,
                request(AgentControlCommand::Provision(ProvisionCommand {
                    provider: ProviderKind::Fly,
                    protocol: ProtocolKind::Acp,
                    name: None,
                    runtime_class: None,
                    relay_urls: vec![],
                    keep: false,
                    bot_secret_key_hex: None,
                    microvm: None,
                })),
            )
            .await;
        let runtime_id = provision
            .result
            .expect("provision result")
            .runtime
            .runtime_id;

        let out = service
            .handle_command(
                &other.to_hex(),
                other,
                request_with(
                    "req-2",
                    "idem-2",
                    AgentControlCommand::GetRuntime(GetRuntimeCommand { runtime_id }),
                ),
            )
            .await;
        assert!(out.result.is_none());
        let err = out.error.expect("owner scoping error");
        assert_eq!(err.code, "runtime_not_found");
    }

    #[tokio::test]
    async fn list_runtimes_is_scoped_to_owner() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter);
        let owner = Keys::generate().public_key();
        let other = Keys::generate().public_key();

        let out = service
            .handle_command(
                &owner.to_hex(),
                owner,
                request_with(
                    "req-1",
                    "idem-1",
                    AgentControlCommand::Provision(ProvisionCommand {
                        provider: ProviderKind::Fly,
                        protocol: ProtocolKind::Acp,
                        name: None,
                        runtime_class: None,
                        relay_urls: vec![],
                        keep: false,
                        bot_secret_key_hex: None,
                        microvm: None,
                    }),
                ),
            )
            .await;
        assert!(out.result.is_some());

        let out = service
            .handle_command(
                &other.to_hex(),
                other,
                request_with(
                    "req-list",
                    "idem-list",
                    AgentControlCommand::ListRuntimes(ListRuntimesCommand::default()),
                ),
            )
            .await;
        let result = out.result.expect("list result");
        let count = result
            .payload
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn idempotent_error_outcomes_are_not_cached() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(CountingFailingAdapter {
            calls: calls.clone(),
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter);
        let requester = Keys::generate().public_key();
        let cmd = request(AgentControlCommand::Provision(ProvisionCommand {
            provider: ProviderKind::Fly,
            protocol: ProtocolKind::Acp,
            name: None,
            runtime_class: None,
            relay_urls: vec![],
            keep: false,
            bot_secret_key_hex: None,
            microvm: None,
        }));

        let first = service
            .handle_command(&requester.to_hex(), requester, cmd.clone())
            .await;
        assert!(first.error.is_some());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        let replay = service
            .handle_command(&requester.to_hex(), requester, cmd)
            .await;
        assert!(replay.error.is_some());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(!replay
            .statuses
            .iter()
            .any(|status| status.message.as_deref() == Some("idempotent replay")));
    }

    #[tokio::test]
    async fn provision_is_denied_when_policy_disallows_requester() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let allowed = Keys::generate().public_key();
        let denied = Keys::generate().public_key();
        let service = AgentControlService::with_adapters_and_policy(
            adapter.clone(),
            adapter,
            ProvisionPolicy::Allowlist(HashSet::from([allowed.to_hex()])),
            DEFAULT_IDEMPOTENCY_MAX_ENTRIES,
        );

        let out = service
            .handle_command(
                &denied.to_hex(),
                denied,
                request(AgentControlCommand::Provision(ProvisionCommand {
                    provider: ProviderKind::Fly,
                    protocol: ProtocolKind::Acp,
                    name: None,
                    runtime_class: None,
                    relay_urls: vec![],
                    keep: false,
                    bot_secret_key_hex: None,
                    microvm: None,
                })),
            )
            .await;
        assert!(out.result.is_none());
        let err = out.error.expect("provision should be unauthorized");
        assert_eq!(err.code, "provision_unauthorized");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        let state = service.state.read().await;
        assert!(state.idempotency.is_empty());
    }

    #[tokio::test]
    async fn idempotency_cache_is_bounded() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let service = AgentControlService::with_adapters_and_policy(
            adapter.clone(),
            adapter,
            ProvisionPolicy::AllowAll,
            2,
        );
        let requester = Keys::generate().public_key();

        for i in 0..3 {
            let req_id = format!("req-{i}");
            let idem = format!("idem-{i}");
            let out = service
                .handle_command(
                    &requester.to_hex(),
                    requester,
                    request_with(
                        &req_id,
                        &idem,
                        AgentControlCommand::Provision(ProvisionCommand {
                            provider: ProviderKind::Fly,
                            protocol: ProtocolKind::Acp,
                            name: None,
                            runtime_class: None,
                            relay_urls: vec![],
                            keep: false,
                            bot_secret_key_hex: None,
                            microvm: None,
                        }),
                    ),
                )
                .await;
            assert!(out.result.is_some());
        }

        let state = service.state.read().await;
        assert_eq!(state.idempotency.len(), 2);
        assert_eq!(state.idempotency_order.len(), 2);
        assert!(!state
            .idempotency
            .contains_key(&(requester.to_hex(), "idem-0".to_string())));
    }

    #[tokio::test]
    async fn teardown_persist_failure_keeps_runtime_in_teardown_phase() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let state_dir = unique_temp_path("pika-agent-control-state-dir");
        std::fs::create_dir_all(&state_dir).expect("create failing persistence directory");
        let persistence = std::sync::Arc::new(ControlStatePersistence::new(state_dir.clone()));
        let service = AgentControlService::with_adapters_policy_and_persistence(
            adapter.clone(),
            adapter,
            ProvisionPolicy::AllowAll,
            DEFAULT_IDEMPOTENCY_MAX_ENTRIES,
            persistence,
        );
        let owner = Keys::generate().public_key();
        let runtime_id = "runtime-teardown-persist".to_string();
        {
            let mut state = service.state.write().await;
            state.runtimes.insert(
                runtime_id.clone(),
                RuntimeRecord {
                    owner_pubkey_hex: owner.to_hex(),
                    descriptor: RuntimeDescriptor {
                        runtime_id: runtime_id.clone(),
                        provider: ProviderKind::Fly,
                        lifecycle_phase: RuntimeLifecyclePhase::Ready,
                        runtime_class: Some("fly".to_string()),
                        region: Some("local".to_string()),
                        capacity: json!({"slots": 1}),
                        policy_constraints: Value::Null,
                        protocol_compatibility: vec![ProtocolKind::Acp],
                        bot_pubkey: Some("ab".repeat(32)),
                        metadata: Value::Null,
                    },
                    provider_handle: ProviderHandle::Fly {
                        machine_id: "machine-1".to_string(),
                        volume_id: "volume-1".to_string(),
                        app_name: "app".to_string(),
                    },
                },
            );
        }

        let out = service
            .handle_command(
                &owner.to_hex(),
                owner,
                request(AgentControlCommand::Teardown(TeardownCommand {
                    runtime_id: runtime_id.clone(),
                })),
            )
            .await;
        let result = out.result.expect("teardown should still return result");
        assert_eq!(
            result.runtime.lifecycle_phase,
            RuntimeLifecyclePhase::Teardown
        );
        assert_eq!(
            result.payload.get("state_persist").and_then(Value::as_str),
            Some("failed")
        );
        let state = service.state.read().await;
        assert_eq!(
            state
                .runtimes
                .get(&runtime_id)
                .expect("runtime remains in state")
                .descriptor
                .lifecycle_phase,
            RuntimeLifecyclePhase::Teardown
        );

        let _ = std::fs::remove_dir_all(&state_dir);
    }

    #[test]
    fn loads_legacy_runtime_state_without_owner_field() {
        let path = unique_temp_path("pika-agent-control-state.json");
        let runtime_id = "runtime-legacy-ownerless".to_string();
        let mut state = ControlState::default();
        state.runtimes.insert(
            runtime_id.clone(),
            RuntimeRecord {
                owner_pubkey_hex: "owner".to_string(),
                descriptor: RuntimeDescriptor {
                    runtime_id: runtime_id.clone(),
                    provider: ProviderKind::Fly,
                    lifecycle_phase: RuntimeLifecyclePhase::Ready,
                    runtime_class: Some("fly".to_string()),
                    region: Some("local".to_string()),
                    capacity: json!({"slots": 1}),
                    policy_constraints: Value::Null,
                    protocol_compatibility: vec![ProtocolKind::Acp],
                    bot_pubkey: Some("ab".repeat(32)),
                    metadata: Value::Null,
                },
                provider_handle: ProviderHandle::Fly {
                    machine_id: "machine-1".to_string(),
                    volume_id: "volume-1".to_string(),
                    app_name: "app".to_string(),
                },
            },
        );
        let mut serialized =
            serde_json::to_value(PersistedControlState::from(&state)).expect("serialize state");
        let runtimes = serialized
            .get_mut("runtimes")
            .and_then(Value::as_object_mut)
            .expect("runtimes map");
        let runtime = runtimes
            .get_mut(&runtime_id)
            .and_then(Value::as_object_mut)
            .expect("runtime entry");
        runtime.remove("owner_pubkey_hex");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&serialized).expect("serialize legacy json"),
        )
        .expect("write legacy state");

        let persistence = ControlStatePersistence::new(path.clone());
        let loaded = persistence.load().expect("load legacy state");
        assert_eq!(loaded.runtimes.len(), 1);
        assert_eq!(
            loaded
                .runtimes
                .get(&runtime_id)
                .expect("legacy runtime")
                .owner_pubkey_hex,
            ""
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn loads_legacy_runtime_state_with_pi_protocol_value() {
        let path = unique_temp_path("pika-agent-control-state.json");
        let runtime_id = "runtime-legacy-pi-protocol".to_string();
        let mut state = ControlState::default();
        state.runtimes.insert(
            runtime_id.clone(),
            RuntimeRecord {
                owner_pubkey_hex: "owner".to_string(),
                descriptor: RuntimeDescriptor {
                    runtime_id: runtime_id.clone(),
                    provider: ProviderKind::Fly,
                    lifecycle_phase: RuntimeLifecyclePhase::Ready,
                    runtime_class: Some("fly".to_string()),
                    region: Some("local".to_string()),
                    capacity: json!({"slots": 1}),
                    policy_constraints: Value::Null,
                    protocol_compatibility: vec![ProtocolKind::Acp],
                    bot_pubkey: Some("ab".repeat(32)),
                    metadata: Value::Null,
                },
                provider_handle: ProviderHandle::Fly {
                    machine_id: "machine-1".to_string(),
                    volume_id: "volume-1".to_string(),
                    app_name: "app".to_string(),
                },
            },
        );
        let mut serialized =
            serde_json::to_value(PersistedControlState::from(&state)).expect("serialize state");
        let protocols = serialized["runtimes"][&runtime_id]["descriptor"]["protocol_compatibility"]
            .as_array_mut()
            .expect("protocol_compatibility array");
        protocols[0] = json!("pi");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&serialized).expect("serialize legacy json"),
        )
        .expect("write legacy state");

        let persistence = ControlStatePersistence::new(path.clone());
        let loaded = persistence.load().expect("load legacy state");
        let runtime = loaded.runtimes.get(&runtime_id).expect("legacy runtime");
        assert_eq!(
            runtime.descriptor.protocol_compatibility,
            vec![ProtocolKind::Acp]
        );

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn legacy_ownerless_runtime_is_accessible_to_allowlisted_requesters() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let allowed = Keys::generate().public_key();
        let denied = Keys::generate().public_key();
        let service = AgentControlService::with_adapters_and_policy(
            adapter.clone(),
            adapter,
            ProvisionPolicy::Allowlist(HashSet::from([allowed.to_hex()])),
            DEFAULT_IDEMPOTENCY_MAX_ENTRIES,
        );
        let runtime_id = "runtime-legacy-access".to_string();
        {
            let mut state = service.state.write().await;
            state.runtimes.insert(
                runtime_id.clone(),
                RuntimeRecord {
                    owner_pubkey_hex: String::new(),
                    descriptor: RuntimeDescriptor {
                        runtime_id: runtime_id.clone(),
                        provider: ProviderKind::Fly,
                        lifecycle_phase: RuntimeLifecyclePhase::Ready,
                        runtime_class: Some("fly".to_string()),
                        region: Some("local".to_string()),
                        capacity: json!({"slots": 1}),
                        policy_constraints: Value::Null,
                        protocol_compatibility: vec![ProtocolKind::Acp],
                        bot_pubkey: Some("ab".repeat(32)),
                        metadata: Value::Null,
                    },
                    provider_handle: ProviderHandle::Fly {
                        machine_id: "machine-1".to_string(),
                        volume_id: "volume-1".to_string(),
                        app_name: "app".to_string(),
                    },
                },
            );
        }

        let allowed_get = service
            .handle_command(
                &allowed.to_hex(),
                allowed,
                request_with(
                    "req-allowed",
                    "idem-allowed",
                    AgentControlCommand::GetRuntime(GetRuntimeCommand {
                        runtime_id: runtime_id.clone(),
                    }),
                ),
            )
            .await;
        assert!(allowed_get.result.is_some());

        let denied_get = service
            .handle_command(
                &denied.to_hex(),
                denied,
                request_with(
                    "req-denied",
                    "idem-denied",
                    AgentControlCommand::GetRuntime(GetRuntimeCommand { runtime_id }),
                ),
            )
            .await;
        assert!(denied_get.result.is_none());
        assert_eq!(
            denied_get.error.expect("denied should fail").code,
            "runtime_not_found"
        );
    }

    #[test]
    fn provider_name_returns_expected_strings() {
        assert_eq!(provider_name(ProviderKind::Fly), "fly");
        assert_eq!(provider_name(ProviderKind::Microvm), "microvm");
    }

    #[test]
    fn runtime_profile_defaults_to_provider_name_as_runtime_class() {
        // Ensure no env vars are set for this test
        std::env::remove_var("PIKA_AGENT_RUNTIME_CLASS");
        std::env::remove_var("PIKA_AGENT_RUNTIME_CLASS_FLY");
        std::env::remove_var("PIKA_AGENT_RUNTIME_CLASS_MICROVM");

        let fly_profile = runtime_profile(ProviderKind::Fly);
        assert_eq!(fly_profile.runtime_class, Some("fly".to_string()));
        assert_eq!(fly_profile.region, None);
        assert_eq!(fly_profile.capacity, Value::Null);
        assert_eq!(fly_profile.policy_constraints, Value::Null);

        let microvm_profile = runtime_profile(ProviderKind::Microvm);
        assert_eq!(microvm_profile.runtime_class, Some("microvm".to_string()));
    }

    #[test]
    fn env_string_for_provider_prefers_provider_specific_key() {
        let unique = format!("TEST_ENV_STR_{}", rand::thread_rng().r#gen::<u32>());
        let fly_key = format!("{unique}_FLY");

        std::env::set_var(&unique, "generic");
        std::env::set_var(&fly_key, "fly-specific");

        let result = env_string_for_provider(ProviderKind::Fly, &unique);
        assert_eq!(result, Some("fly-specific".to_string()));

        std::env::remove_var(&unique);
        std::env::remove_var(&fly_key);
    }

    #[test]
    fn env_string_for_provider_falls_back_to_generic_key() {
        let unique = format!("TEST_ENV_FALLBACK_{}", rand::thread_rng().r#gen::<u32>());
        let microvm_key = format!("{unique}_MICROVM");

        std::env::set_var(&unique, "generic-value");
        std::env::remove_var(&microvm_key);

        let result = env_string_for_provider(ProviderKind::Microvm, &unique);
        assert_eq!(result, Some("generic-value".to_string()));

        std::env::remove_var(&unique);
    }

    #[test]
    fn env_string_for_provider_returns_none_when_unset() {
        let unique = format!("TEST_ENV_NONE_{}", rand::thread_rng().r#gen::<u32>());
        let fly_key = format!("{unique}_FLY");
        std::env::remove_var(&unique);
        std::env::remove_var(&fly_key);

        let result = env_string_for_provider(ProviderKind::Fly, &unique);
        assert_eq!(result, None);
    }

    #[test]
    fn env_json_for_provider_parses_valid_json() {
        let unique = format!("TEST_ENV_JSON_{}", rand::thread_rng().r#gen::<u32>());
        std::env::set_var(&unique, r#"{"slots": 4}"#);

        let result = env_json_for_provider(ProviderKind::Fly, &unique);
        assert_eq!(result, json!({"slots": 4}));

        std::env::remove_var(&unique);
    }

    #[test]
    fn env_json_for_provider_wraps_invalid_json() {
        let unique = format!("TEST_ENV_BADJSON_{}", rand::thread_rng().r#gen::<u32>());
        std::env::set_var(&unique, "not-json");

        let result = env_json_for_provider(ProviderKind::Fly, &unique);
        assert_eq!(result, json!({"raw": "not-json"}));

        std::env::remove_var(&unique);
    }

    #[test]
    fn env_json_for_provider_returns_null_when_unset() {
        let unique = format!("TEST_ENV_NOJSON_{}", rand::thread_rng().r#gen::<u32>());
        std::env::remove_var(&unique);

        let result = env_json_for_provider(ProviderKind::Fly, &unique);
        assert_eq!(result, Value::Null);
    }

    #[tokio::test]
    async fn different_idempotency_key_triggers_new_provision() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter);
        let requester = Keys::generate().public_key();

        let first = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request_with(
                    "req-a",
                    "idem-a",
                    AgentControlCommand::Provision(ProvisionCommand {
                        provider: ProviderKind::Fly,
                        protocol: ProtocolKind::Acp,
                        name: None,
                        runtime_class: None,
                        relay_urls: vec![],
                        keep: false,
                        bot_secret_key_hex: None,
                        microvm: None,
                    }),
                ),
            )
            .await;
        assert!(first.result.is_some());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        let second = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request_with(
                    "req-b",
                    "idem-b",
                    AgentControlCommand::Provision(ProvisionCommand {
                        provider: ProviderKind::Fly,
                        protocol: ProtocolKind::Acp,
                        name: None,
                        runtime_class: None,
                        relay_urls: vec![],
                        keep: false,
                        bot_secret_key_hex: None,
                        microvm: None,
                    }),
                ),
            )
            .await;
        assert!(second.result.is_some());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn provision_failure_normalizes_error_code() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(CountingFailingAdapter {
            calls: calls.clone(),
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter);
        let requester = Keys::generate().public_key();

        let out = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request(AgentControlCommand::Provision(ProvisionCommand {
                    provider: ProviderKind::Fly,
                    protocol: ProtocolKind::Acp,
                    name: None,
                    runtime_class: None,
                    relay_urls: vec![],
                    keep: false,
                    bot_secret_key_hex: None,
                    microvm: None,
                })),
            )
            .await;
        assert!(out.result.is_none());
        let err = out.error.expect("expected provision_failed error");
        assert_eq!(err.code, "provision_failed");
        assert!(err.hint.is_some());
        assert!(
            err.detail
                .as_deref()
                .unwrap()
                .contains("simulated provision failure"),
            "raw error should be wrapped in detail field"
        );
        assert!(out
            .statuses
            .iter()
            .any(|s| s.phase == RuntimeLifecyclePhase::Failed));
    }

    #[tokio::test]
    async fn all_error_codes_are_consistent_strings() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter);
        let requester = Keys::generate().public_key();

        let not_found = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request_with(
                    "req-nf",
                    "idem-nf",
                    AgentControlCommand::GetRuntime(GetRuntimeCommand {
                        runtime_id: "nonexistent".to_string(),
                    }),
                ),
            )
            .await;
        let teardown_nf = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request_with(
                    "req-td",
                    "idem-td",
                    AgentControlCommand::Teardown(TeardownCommand {
                        runtime_id: "nonexistent".to_string(),
                    }),
                ),
            )
            .await;

        let nf_err = not_found.error.expect("get should fail");
        let td_err = teardown_nf.error.expect("teardown should fail");
        assert_eq!(nf_err.code, "runtime_not_found");
        assert_eq!(td_err.code, "runtime_not_found");
    }

    #[tokio::test]
    async fn descriptor_fields_flow_through_provision_result() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter);
        let requester = Keys::generate().public_key();

        let out = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request(AgentControlCommand::Provision(ProvisionCommand {
                    provider: ProviderKind::Fly,
                    protocol: ProtocolKind::Acp,
                    name: None,
                    runtime_class: None,
                    relay_urls: vec![],
                    keep: false,
                    bot_secret_key_hex: None,
                    microvm: None,
                })),
            )
            .await;

        let result = out.result.expect("provision should succeed");
        let rt = &result.runtime;
        assert_eq!(rt.provider, ProviderKind::Fly);
        assert_eq!(rt.lifecycle_phase, RuntimeLifecyclePhase::Ready);
        assert_eq!(rt.runtime_class, Some("mock".to_string()));
        assert_eq!(rt.region, Some("local".to_string()));
        assert_eq!(rt.capacity, json!({"slots": 1}));
        assert_eq!(rt.policy_constraints, json!({"allow_keep": true}));
        assert_eq!(rt.protocol_compatibility, vec![ProtocolKind::Acp]);
        assert_eq!(rt.bot_pubkey, Some("ab".repeat(32)));
        assert!(!rt.metadata.is_null());
    }

    #[tokio::test]
    async fn get_runtime_returns_full_descriptor_after_provision() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter);
        let requester = Keys::generate().public_key();

        let provision_out = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request(AgentControlCommand::Provision(ProvisionCommand {
                    provider: ProviderKind::Fly,
                    protocol: ProtocolKind::Acp,
                    name: None,
                    runtime_class: None,
                    relay_urls: vec![],
                    keep: false,
                    bot_secret_key_hex: None,
                    microvm: None,
                })),
            )
            .await;
        let provisioned_rt = provision_out
            .result
            .expect("provision should succeed")
            .runtime;
        let runtime_id = provisioned_rt.runtime_id.clone();

        let get_out = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request_with(
                    "req-get",
                    "idem-get",
                    AgentControlCommand::GetRuntime(GetRuntimeCommand {
                        runtime_id: runtime_id.clone(),
                    }),
                ),
            )
            .await;
        let get_rt = get_out.result.expect("get should succeed").runtime;
        assert_eq!(get_rt.runtime_id, runtime_id);
        assert_eq!(get_rt.provider, provisioned_rt.provider);
        assert_eq!(get_rt.runtime_class, provisioned_rt.runtime_class);
        assert_eq!(get_rt.region, provisioned_rt.region);
        assert_eq!(get_rt.bot_pubkey, provisioned_rt.bot_pubkey);
        assert_eq!(
            get_rt.protocol_compatibility,
            provisioned_rt.protocol_compatibility
        );
    }

    #[tokio::test]
    async fn teardown_transitions_lifecycle_phase() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter);
        let requester = Keys::generate().public_key();

        let provision_out = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request(AgentControlCommand::Provision(ProvisionCommand {
                    provider: ProviderKind::Fly,
                    protocol: ProtocolKind::Acp,
                    name: None,
                    runtime_class: None,
                    relay_urls: vec![],
                    keep: false,
                    bot_secret_key_hex: None,
                    microvm: None,
                })),
            )
            .await;
        let runtime_id = provision_out.result.expect("provision").runtime.runtime_id;

        let teardown_out = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request_with(
                    "req-td",
                    "idem-td",
                    AgentControlCommand::Teardown(TeardownCommand {
                        runtime_id: runtime_id.clone(),
                    }),
                ),
            )
            .await;
        let td_rt = teardown_out
            .result
            .expect("teardown should succeed")
            .runtime;
        assert_eq!(td_rt.runtime_id, runtime_id);
        assert_eq!(td_rt.lifecycle_phase, RuntimeLifecyclePhase::Teardown);

        let get_after = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request_with(
                    "req-get2",
                    "idem-get2",
                    AgentControlCommand::GetRuntime(GetRuntimeCommand {
                        runtime_id: runtime_id.clone(),
                    }),
                ),
            )
            .await;
        let get_rt = get_after.result.expect("get after teardown").runtime;
        assert_eq!(get_rt.lifecycle_phase, RuntimeLifecyclePhase::Teardown);
    }
}
