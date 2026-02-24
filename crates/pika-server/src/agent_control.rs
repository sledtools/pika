use std::collections::HashMap;
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use nostr_sdk::prelude::*;
use pikachat::fly_machines::FlyClient;
use pikachat::microvm_spawner::{CreateVmRequest, GuestAutostartRequest, MicrovmSpawnerClient};
use pikachat::provider_control_plane::{
    AgentControlCmdEnvelope, AgentControlCommand, AgentControlErrorEnvelope,
    AgentControlResultEnvelope, AgentControlStatusEnvelope, GetRuntimeCommand,
    MicrovmProvisionParams, ProcessWelcomeCommand, ProtocolKind, ProviderKind, ProvisionCommand,
    RuntimeDescriptor, RuntimeLifecyclePhase, TeardownCommand, CMD_SCHEMA_V1, CONTROL_CMD_KIND,
    CONTROL_ERROR_KIND, CONTROL_RESULT_KIND, CONTROL_STATUS_KIND, ERROR_SCHEMA_V1,
    RESULT_SCHEMA_V1, STATUS_SCHEMA_V1,
};
use pikachat::workers_agents::WorkersClient;
use rand::Rng;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

const DEFAULT_MICROVM_SPAWNER_URL: &str = "http://127.0.0.1:8080";
const DEFAULT_MICROVM_FLAKE_REF: &str = "github:sledtools/pika";
const DEFAULT_MICROVM_DEV_SHELL: &str = "default";
const DEFAULT_MICROVM_CPU: u32 = 1;
const DEFAULT_MICROVM_MEMORY_MB: u32 = 1024;
const DEFAULT_MICROVM_TTL_SECONDS: u64 = 7200;

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
            service: AgentControlService::new(),
        }))
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let server_pubkey_hex = self.keys.public_key().to_hex();
        let filter = Filter::new()
            .kind(Kind::Custom(CONTROL_CMD_KIND))
            .custom_tag(
                SingleLetterTag::lowercase(Alphabet::P),
                server_pubkey_hex.clone(),
            );
        self.client.subscribe(filter, None).await?;

        let mut notifications = self.client.notifications();
        let mut seen: std::collections::HashSet<EventId> = std::collections::HashSet::new();

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
            if seen.len() > 8192 {
                seen.clear();
            }

            let requester = event.pubkey;
            let cmd = match serde_json::from_str::<AgentControlCmdEnvelope>(&event.content) {
                Ok(cmd) => cmd,
                Err(err) => {
                    let request_id = extract_request_id(&event.content)
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
    let event = EventBuilder::new(Kind::Custom(kind), content)
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

#[derive(Clone)]
struct AgentControlService {
    state: std::sync::Arc<RwLock<ControlState>>,
    fly: std::sync::Arc<dyn ProviderAdapter>,
    workers: std::sync::Arc<dyn ProviderAdapter>,
    microvm: std::sync::Arc<dyn ProviderAdapter>,
}

impl AgentControlService {
    fn new() -> Self {
        Self {
            state: std::sync::Arc::new(RwLock::new(ControlState::default())),
            fly: std::sync::Arc::new(FlyAdapter),
            workers: std::sync::Arc::new(WorkersAdapter),
            microvm: std::sync::Arc::new(MicrovmAdapter),
        }
    }

    #[cfg(test)]
    fn with_adapters(
        fly: std::sync::Arc<dyn ProviderAdapter>,
        workers: std::sync::Arc<dyn ProviderAdapter>,
        microvm: std::sync::Arc<dyn ProviderAdapter>,
    ) -> Self {
        Self {
            state: std::sync::Arc::new(RwLock::new(ControlState::default())),
            fly,
            workers,
            microvm,
        }
    }

    fn adapter_for(&self, provider: ProviderKind) -> std::sync::Arc<dyn ProviderAdapter> {
        match provider {
            ProviderKind::Fly => self.fly.clone(),
            ProviderKind::Workers => self.workers.clone(),
            ProviderKind::Microvm => self.microvm.clone(),
        }
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
                    Some(cached.provider()),
                    Some("idempotent replay".to_string()),
                    Value::Null,
                ));
                return cached.to_outcome(statuses, cmd.request_id);
            }
        }

        let outcome = match cmd.command.clone() {
            AgentControlCommand::Provision(provision) => {
                self.handle_provision(
                    cmd.request_id.clone(),
                    requester_pubkey,
                    provision,
                    statuses,
                )
                .await
            }
            AgentControlCommand::ProcessWelcome(process_welcome) => {
                self.handle_process_welcome(cmd.request_id.clone(), process_welcome, statuses)
                    .await
            }
            AgentControlCommand::Teardown(teardown) => {
                self.handle_teardown(cmd.request_id.clone(), teardown, statuses)
                    .await
            }
            AgentControlCommand::GetRuntime(get_runtime) => {
                self.handle_get_runtime(cmd.request_id.clone(), get_runtime, statuses)
                    .await
            }
        };

        let terminal = match (&outcome.result, &outcome.error) {
            (Some(result), None) => CachedTerminal::Result {
                provider: result.runtime.provider,
                runtime_id: result.runtime.runtime_id.clone(),
                runtime: result.runtime.clone(),
                payload: result.payload.clone(),
            },
            (None, Some(err)) => CachedTerminal::Error {
                code: err.code.clone(),
                hint: err.hint.clone(),
                detail: err.detail.clone(),
            },
            _ => CachedTerminal::Error {
                code: "internal_state".to_string(),
                hint: Some("command produced invalid terminal state".to_string()),
                detail: None,
            },
        };

        let mut state = self.state.write().await;
        state.idempotency.insert(cache_key, terminal);
        outcome
    }

    async fn handle_provision(
        &self,
        request_id: String,
        requester_pubkey: PublicKey,
        provision: ProvisionCommand,
        mut statuses: Vec<AgentControlStatusEnvelope>,
    ) -> CommandOutcome {
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
            protocol_compatibility: provisioned.protocol_compatibility.clone(),
            bot_pubkey: provisioned.bot_pubkey.clone(),
            metadata: provisioned.metadata.clone(),
        };

        {
            let mut state = self.state.write().await;
            state.runtimes.insert(
                runtime_id.clone(),
                RuntimeRecord {
                    descriptor: descriptor.clone(),
                    provider_handle: provisioned.provider_handle,
                },
            );
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

        statuses.push(AgentControlStatusEnvelope::v1(
            request_id.clone(),
            RuntimeLifecyclePhase::Teardown,
            Some(runtime.descriptor.runtime_id.clone()),
            Some(runtime.descriptor.provider),
            Some("teardown in progress".to_string()),
            Value::Null,
        ));
        let adapter = self.adapter_for(runtime.descriptor.provider);
        let payload = match adapter.teardown(&runtime).await {
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
        }
        CommandOutcome::result(
            statuses,
            AgentControlResultEnvelope::v1(request_id, runtime.descriptor, payload),
        )
    }

    async fn handle_get_runtime(
        &self,
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
        CommandOutcome::result(
            statuses,
            AgentControlResultEnvelope::v1(
                request_id,
                runtime.descriptor,
                json!({"operation":"get_runtime"}),
            ),
        )
    }
}

#[derive(Clone, Debug)]
struct RuntimeRecord {
    descriptor: RuntimeDescriptor,
    provider_handle: ProviderHandle,
}

#[derive(Clone, Debug)]
enum ProviderHandle {
    Fly {
        machine_id: String,
        volume_id: String,
        app_name: String,
    },
    Workers {
        agent_id: String,
        base_url: String,
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
}

#[derive(Clone, Debug)]
enum CachedTerminal {
    Result {
        provider: ProviderKind,
        runtime_id: String,
        runtime: RuntimeDescriptor,
        payload: Value,
    },
    Error {
        code: String,
        hint: Option<String>,
        detail: Option<String>,
    },
}

impl CachedTerminal {
    fn provider(&self) -> ProviderKind {
        match self {
            Self::Result { provider, .. } => *provider,
            Self::Error { .. } => ProviderKind::Fly,
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
                AgentControlResultEnvelope::v1(request_id, runtime.clone(), payload.clone()),
            ),
            Self::Error { code, hint, detail } => CommandOutcome::error(
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
struct ProvisionedRuntime {
    provider_handle: ProviderHandle,
    bot_pubkey: Option<String>,
    metadata: Value,
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
            }),
            protocol_compatibility: vec![ProtocolKind::Pi],
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
struct WorkersAdapter;

#[async_trait]
impl ProviderAdapter for WorkersAdapter {
    async fn provision(
        &self,
        _runtime_id: &str,
        _owner_pubkey: PublicKey,
        provision: &ProvisionCommand,
    ) -> anyhow::Result<ProvisionedRuntime> {
        let workers = WorkersClient::from_env()?;
        let bot_keys = if let Some(secret) = provision.bot_secret_key_hex.as_deref() {
            Keys::parse(secret).context("parse bot_secret_key_hex")?
        } else {
            Keys::generate()
        };
        let bot_secret = bot_keys.secret_key().to_secret_hex();
        let bot_pubkey = bot_keys.public_key().to_hex();
        let agent_name = provision
            .name
            .clone()
            .unwrap_or_else(|| format!("agent-{:08x}", rand::thread_rng().r#gen::<u32>()));
        let relay_urls = if provision.relay_urls.is_empty() {
            default_relay_urls()
        } else {
            provision.relay_urls.clone()
        };

        let mut status = workers
            .create_agent(&pikachat::workers_agents::CreateAgentRequest {
                name: Some(agent_name),
                brain: "pi".to_string(),
                relay_urls,
                bot_secret_key_hex: Some(bot_secret),
            })
            .await?;
        if status.bot_pubkey.trim().to_lowercase() != bot_pubkey {
            anyhow::bail!(
                "workers bot pubkey mismatch: expected {}, got {}",
                bot_pubkey,
                status.bot_pubkey
            );
        }

        let start = tokio::time::Instant::now();
        let timeout = Duration::from_secs(120);
        while status.key_package_published_at_ms.is_none() {
            if start.elapsed() >= timeout {
                anyhow::bail!("timed out waiting for workers startup keypackage readiness");
            }
            tokio::time::sleep(Duration::from_millis(900)).await;
            status = workers.get_agent(&status.id).await?;
        }

        Ok(ProvisionedRuntime {
            provider_handle: ProviderHandle::Workers {
                agent_id: status.id.clone(),
                base_url: workers.base_url().to_string(),
            },
            bot_pubkey: Some(status.bot_pubkey.clone()),
            metadata: json!({
                "agent_id": status.id,
                "workers_base_url": workers.base_url(),
                "key_package_published_at_ms": status.key_package_published_at_ms,
            }),
            protocol_compatibility: vec![ProtocolKind::Pi, ProtocolKind::Acp],
        })
    }

    async fn process_welcome(
        &self,
        runtime: &RuntimeRecord,
        process_welcome: &ProcessWelcomeCommand,
    ) -> anyhow::Result<Value> {
        let ProviderHandle::Workers { agent_id, .. } = &runtime.provider_handle else {
            anyhow::bail!("workers adapter received non-workers runtime handle")
        };
        let workers = WorkersClient::from_env()?;
        let response = workers
            .runtime_process_welcome_event_json(
                agent_id,
                &process_welcome.group_id,
                process_welcome.wrapper_event_id_hex.as_deref(),
                process_welcome.welcome_event_json.as_deref(),
            )
            .await?;
        Ok(json!({
            "processed": true,
            "group_id": response.group_id,
            "created_group": response.created_group,
            "processed_welcomes": response.processed_welcomes,
            "mls_group_id_hex": response.mls_group_id_hex,
            "nostr_group_id_hex": response.nostr_group_id_hex,
        }))
    }

    async fn teardown(&self, runtime: &RuntimeRecord) -> anyhow::Result<Value> {
        let ProviderHandle::Workers { agent_id, base_url } = &runtime.provider_handle else {
            anyhow::bail!("workers adapter received non-workers runtime handle")
        };
        Ok(json!({
            "teardown": "manual",
            "agent_id": agent_id,
            "workers_base_url": base_url,
            "hint": format!("inspect with: {}/agents/{}", base_url.trim_end_matches('/'), agent_id),
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
        let params = provision.microvm.clone().unwrap_or_default();
        let resolved = resolve_microvm_params(&params, provision.keep);
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
        let create_vm = CreateVmRequest {
            flake_ref: Some(resolved.flake_ref.clone()),
            dev_shell: Some(resolved.dev_shell.clone()),
            cpu: Some(resolved.cpu),
            memory_mb: Some(resolved.memory_mb),
            ttl_seconds: Some(resolved.ttl_seconds),
            spawn_variant: Some(resolved.spawn_variant.clone()),
            guest_autostart: Some(GuestAutostartRequest {
                command: "bash /workspace/pika-agent/start-agent.sh".to_string(),
                env: std::collections::BTreeMap::from([
                    ("PIKA_OWNER_PUBKEY".to_string(), owner_pubkey.to_hex()),
                    ("PIKA_RELAY_URLS".to_string(), relay_urls.join(",")),
                    ("PIKA_BOT_PUBKEY".to_string(), bot_pubkey.clone()),
                ]),
                files: std::collections::BTreeMap::from([
                    (
                        "workspace/pika-agent/start-agent.sh".to_string(),
                        microvm_autostart_script().to_string(),
                    ),
                    (
                        "workspace/pika-agent/microvm-bridge.py".to_string(),
                        microvm_bridge_script().to_string(),
                    ),
                    (
                        "workspace/pika-agent/state/identity.json".to_string(),
                        bot_identity_file(&bot_secret_hex, &bot_pubkey),
                    ),
                ]),
            }),
        };
        let vm = spawner.create_vm(&create_vm).await.with_context(|| {
            format!(
                "failed to create microvm via vm-spawner at {} (health: {}/healthz)",
                resolved.spawner_url, resolved.spawner_url
            )
        })?;

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
            }),
            protocol_compatibility: vec![ProtocolKind::Pi, ProtocolKind::Acp],
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

#[derive(Clone, Debug)]
struct ResolvedMicrovmParams {
    spawner_url: String,
    spawn_variant: String,
    flake_ref: String,
    dev_shell: String,
    cpu: u32,
    memory_mb: u32,
    ttl_seconds: u64,
    keep: bool,
}

fn resolve_microvm_params(params: &MicrovmProvisionParams, keep: bool) -> ResolvedMicrovmParams {
    ResolvedMicrovmParams {
        spawner_url: params
            .spawner_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_MICROVM_SPAWNER_URL)
            .to_string(),
        spawn_variant: params
            .spawn_variant
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("prebuilt-cow")
            .to_string(),
        flake_ref: params
            .flake_ref
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_MICROVM_FLAKE_REF)
            .to_string(),
        dev_shell: params
            .dev_shell
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_MICROVM_DEV_SHELL)
            .to_string(),
        cpu: params.cpu.unwrap_or(DEFAULT_MICROVM_CPU),
        memory_mb: params.memory_mb.unwrap_or(DEFAULT_MICROVM_MEMORY_MB),
        ttl_seconds: params.ttl_seconds.unwrap_or(DEFAULT_MICROVM_TTL_SECONDS),
        keep,
    }
}

fn default_relay_urls() -> Vec<String> {
    vec![
        "wss://us-east.nostr.pikachat.org".to_string(),
        "wss://eu.nostr.pikachat.org".to_string(),
    ]
}

fn provider_name(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Fly => "fly",
        ProviderKind::Workers => "workers",
        ProviderKind::Microvm => "microvm",
    }
}

fn protocol_name(protocol: ProtocolKind) -> &'static str {
    match protocol {
        ProtocolKind::Pi => "pi",
        ProtocolKind::Acp => "acp",
    }
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

fn bot_identity_file(secret_hex: &str, pubkey_hex: &str) -> String {
    let body = serde_json::to_string_pretty(&json!({
        "secret_key_hex": secret_hex,
        "public_key_hex": pubkey_hex,
    }))
    .expect("identity json");
    format!("{body}\n")
}

fn microvm_autostart_script() -> &'static str {
    r#"#!/usr/bin/env bash
set -euo pipefail

STATE_DIR="/workspace/pika-agent/state"
mkdir -p "$STATE_DIR"

if [[ -z "${PIKA_OWNER_PUBKEY:-}" ]]; then
  echo "[microvm-agent] missing PIKA_OWNER_PUBKEY" >&2
  exit 1
fi
if [[ -z "${PIKA_RELAY_URLS:-}" ]]; then
  echo "[microvm-agent] missing PIKA_RELAY_URLS" >&2
  exit 1
fi

relay_args=()
IFS=',' read -r -a relay_values <<< "${PIKA_RELAY_URLS}"
for relay in "${relay_values[@]}"; do
  relay="${relay#"${relay%%[![:space:]]*}"}"
  relay="${relay%"${relay##*[![:space:]]}"}"
  if [[ -n "$relay" ]]; then
    relay_args+=(--relay "$relay")
  fi
done
if [[ ${#relay_args[@]} -eq 0 ]]; then
  echo "[microvm-agent] no valid relays in PIKA_RELAY_URLS" >&2
  exit 1
fi

if ! command -v pikachat >/dev/null 2>&1; then
  echo "[microvm-agent] could not find pikachat binary" >&2
  exit 1
fi

exec pikachat daemon \
  --state-dir "$STATE_DIR" \
  --auto-accept-welcomes \
  --allow-pubkey "${PIKA_OWNER_PUBKEY}" \
  "${relay_args[@]}" \
  --exec "python3 /workspace/pika-agent/microvm-bridge.py"
"#
}

fn microvm_bridge_script() -> &'static str {
    r#"#!/usr/bin/env python3
import json
import os
import re
import shlex
import subprocess
import sys
from collections import deque

owner = os.environ.get("PIKA_OWNER_PUBKEY", "").strip().lower()
pi_cmd = os.environ.get("PIKA_PI_CMD", "pi -p").strip()
pi_timeout_ms = int(os.environ.get("PIKA_PI_TIMEOUT_MS", "120000"))
if pi_timeout_ms < 1000:
    pi_timeout_ms = 1000

ANSI_RE = re.compile(r"\x1B\[[0-?]*[ -/]*[@-~]")
seen_message_ids = deque(maxlen=256)

def strip_ansi(text):
    return ANSI_RE.sub("", text)

def send(cmd):
    sys.stdout.write(json.dumps(cmd) + "\n")
    sys.stdout.flush()

def is_duplicate(message_id):
    if not message_id:
        return False
    if message_id in seen_message_ids:
        return True
    seen_message_ids.append(message_id)
    return False

def run_local_pi(prompt):
    if not pi_cmd:
        return None, "PIKA_PI_CMD is empty"
    try:
        proc = subprocess.run(
            shlex.split(pi_cmd),
            input=prompt + "\n",
            text=True,
            capture_output=True,
            timeout=pi_timeout_ms / 1000.0,
            check=False,
        )
    except FileNotFoundError:
        return None, f"pi command not found: {pi_cmd}"
    except subprocess.TimeoutExpired:
        return None, f"pi command timed out after {pi_timeout_ms}ms"
    except Exception as exc:
        return None, f"pi command failed: {exc}"

    stdout = strip_ansi(proc.stdout or "").strip()
    stderr = strip_ansi(proc.stderr or "").strip()
    if proc.returncode != 0:
        detail = stderr or stdout or f"exit code {proc.returncode}"
        return None, f"pi command failed ({detail})"

    lines = [line.strip() for line in stdout.splitlines() if line.strip()]
    if not lines:
        return None, "pi command returned empty output"
    return lines[-1], None

send({"type": "ready"})

for raw_line in sys.stdin:
    raw_line = raw_line.strip()
    if not raw_line:
        continue
    try:
        msg = json.loads(raw_line)
    except Exception:
        continue

    if msg.get("type") != "event":
        continue
    payload = msg.get("payload") or {}
    if payload.get("type") != "message":
        continue

    from_pub = str(payload.get("from_pubkey") or "").strip().lower()
    if owner and from_pub != owner:
        continue

    message_id = str(payload.get("message_id") or "").strip()
    if is_duplicate(message_id):
        continue

    prompt = str(payload.get("content") or "").strip()
    if not prompt:
        continue

    reply, err = run_local_pi(prompt)
    if err:
        reply = f"error: {err}"
    if not reply:
        continue

    send({
        "type": "send_message",
        "to_group_id": payload.get("nostr_group_id"),
        "content": reply,
        "client_request_id": message_id or None
    })
"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use pikachat::provider_control_plane::AuthContext;

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
                protocol_compatibility: vec![ProtocolKind::Pi],
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

    fn request(command: AgentControlCommand) -> AgentControlCmdEnvelope {
        AgentControlCmdEnvelope::v1(
            "req-1".to_string(),
            "idem-1".to_string(),
            command,
            AuthContext::default(),
        )
    }

    #[tokio::test]
    async fn idempotency_replay_does_not_reprovision() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let adapter = std::sync::Arc::new(MockAdapter {
            calls: calls.clone(),
        });
        let service = AgentControlService::with_adapters(adapter.clone(), adapter.clone(), adapter);
        let requester = Keys::generate().public_key();

        let first = service
            .handle_command(
                &requester.to_hex(),
                requester,
                request(AgentControlCommand::Provision(ProvisionCommand {
                    provider: ProviderKind::Fly,
                    protocol: ProtocolKind::Pi,
                    name: None,
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
                    protocol: ProtocolKind::Pi,
                    name: None,
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
        let service = AgentControlService::with_adapters(adapter.clone(), adapter.clone(), adapter);
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
}
