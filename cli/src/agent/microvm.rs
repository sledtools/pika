use std::collections::BTreeMap;

use anyhow::anyhow;
use nostr_sdk::prelude::PublicKey;
use serde_json::json;

use crate::microvm_spawner::{CreateVmRequest, GuestAutostartRequest};
use crate::{AgentNewMicrovmArgs, MicrovmSpawnVariant};

pub const DEFAULT_SPAWNER_URL: &str = "http://127.0.0.1:8080";
const DEFAULT_FLAKE_REF: &str = "github:sledtools/pika";
const DEFAULT_DEV_SHELL: &str = "default";
const DEFAULT_CPU: u32 = 1;
const DEFAULT_MEMORY_MB: u32 = 1024;
const DEFAULT_TTL_SECONDS: u64 = 7200;
const AUTOSTART_COMMAND: &str = "bash /workspace/pika-agent/start-agent.sh";
const AUTOSTART_SCRIPT_PATH: &str = "workspace/pika-agent/start-agent.sh";
const AUTOSTART_BRIDGE_PATH: &str = "workspace/pika-agent/microvm-bridge.py";
const AUTOSTART_IDENTITY_PATH: &str = "workspace/pika-agent/state/identity.json";

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MicrovmResolvedArgs {
    pub spawner_url: String,
    pub spawn_variant: String,
    pub flake_ref: String,
    pub dev_shell: String,
    pub cpu: u32,
    pub memory_mb: u32,
    pub ttl_seconds: u64,
    pub keep: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MicrovmTeardownPolicy {
    DeleteOnExit,
    KeepVm,
}

pub fn resolve_args(args: &AgentNewMicrovmArgs) -> MicrovmResolvedArgs {
    MicrovmResolvedArgs {
        spawner_url: args
            .spawner_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_SPAWNER_URL)
            .to_string(),
        spawn_variant: spawn_variant_value(args.spawn_variant).to_string(),
        flake_ref: args
            .flake_ref
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_FLAKE_REF)
            .to_string(),
        dev_shell: args
            .dev_shell
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_DEV_SHELL)
            .to_string(),
        cpu: args.cpu.unwrap_or(DEFAULT_CPU),
        memory_mb: args.memory_mb.unwrap_or(DEFAULT_MEMORY_MB),
        ttl_seconds: args.ttl_seconds.unwrap_or(DEFAULT_TTL_SECONDS),
        keep: args.keep,
    }
}

pub fn teardown_policy(keep: bool) -> MicrovmTeardownPolicy {
    if keep {
        MicrovmTeardownPolicy::KeepVm
    } else {
        MicrovmTeardownPolicy::DeleteOnExit
    }
}

pub fn build_create_vm_request(
    resolved: &MicrovmResolvedArgs,
    owner_pubkey: PublicKey,
    relay_urls: &[String],
    bot_secret_hex: &str,
    bot_pubkey_hex: &str,
) -> CreateVmRequest {
    let mut env = BTreeMap::new();
    env.insert("PIKA_OWNER_PUBKEY".to_string(), owner_pubkey.to_hex());
    env.insert("PIKA_RELAY_URLS".to_string(), relay_urls.join(","));
    env.insert("PIKA_BOT_PUBKEY".to_string(), bot_pubkey_hex.to_string());
    for key in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "PI_MODEL"] {
        if let Ok(value) = std::env::var(key)
            && !value.trim().is_empty()
        {
            env.insert(key.to_string(), value);
        }
    }

    let mut files = BTreeMap::new();
    files.insert(
        AUTOSTART_SCRIPT_PATH.to_string(),
        microvm_autostart_script().to_string(),
    );
    files.insert(
        AUTOSTART_BRIDGE_PATH.to_string(),
        microvm_bridge_script().to_string(),
    );
    files.insert(
        AUTOSTART_IDENTITY_PATH.to_string(),
        bot_identity_file(bot_secret_hex, bot_pubkey_hex),
    );

    CreateVmRequest {
        flake_ref: Some(resolved.flake_ref.clone()),
        dev_shell: Some(resolved.dev_shell.clone()),
        cpu: Some(resolved.cpu),
        memory_mb: Some(resolved.memory_mb),
        ttl_seconds: Some(resolved.ttl_seconds),
        spawn_variant: Some(resolved.spawn_variant.clone()),
        guest_autostart: Some(GuestAutostartRequest {
            command: AUTOSTART_COMMAND.to_string(),
            env,
            files,
        }),
    }
}

pub fn spawner_create_error(spawner_url: &str, err: anyhow::Error) -> anyhow::Error {
    anyhow!(
        "failed to create microvm via vm-spawner at {}: {:#}\nhint: ensure vm-spawner is reachable (curl {}/healthz)\nif this is a remote host, open a tunnel:\n  nix develop .#infra -c just -f infra/justfile build-vmspawner-tunnel",
        spawner_url,
        err,
        spawner_url.trim_end_matches('/')
    )
}

fn spawn_variant_value(value: Option<MicrovmSpawnVariant>) -> &'static str {
    match value.unwrap_or(MicrovmSpawnVariant::PrebuiltCow) {
        MicrovmSpawnVariant::Prebuilt => "prebuilt",
        MicrovmSpawnVariant::PrebuiltCow => "prebuilt-cow",
    }
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

bin=""
if command -v pikachat >/dev/null 2>&1; then
  bin="pikachat"
elif [[ -n "${PIKA_MARMOTD_BIN:-}" ]]; then
  bin="${PIKA_MARMOTD_BIN}"
elif command -v marmotd >/dev/null 2>&1; then
  bin="marmotd"
fi
if [[ -z "$bin" ]]; then
  echo "[microvm-agent] could not find pikachat or marmotd binary" >&2
  exit 1
fi

echo "[microvm-agent] starting daemon via $bin" >&2
exec "$bin" daemon \
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
import sys

owner = os.environ.get("PIKA_OWNER_PUBKEY", "").strip().lower()
relay_env = os.environ.get("PIKA_RELAY_URLS", "")
relays = [relay.strip() for relay in relay_env.split(",") if relay.strip()]


def send(cmd):
    sys.stdout.write(json.dumps(cmd) + "\n")
    sys.stdout.flush()


for raw_line in sys.stdin:
    line = raw_line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except json.JSONDecodeError:
        continue

    kind = str(msg.get("type", ""))
    if kind == "ready":
        cmd = {"cmd": "publish_keypackage", "request_id": "microvm_boot_kp"}
        if relays:
            cmd["relays"] = relays
        send(cmd)
        continue

    if kind != "message_received":
        continue

    sender = str(msg.get("from_pubkey", "")).strip().lower()
    if owner and sender != owner:
        continue

    group_id = str(msg.get("nostr_group_id", "")).strip()
    content = str(msg.get("content", "")).strip()
    if not group_id or not content:
        continue

    send({
        "cmd": "send_message",
        "nostr_group_id": group_id,
        "content": f"microvm> {content}",
    })
"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::prelude::Keys;

    fn args_with_defaults() -> AgentNewMicrovmArgs {
        AgentNewMicrovmArgs {
            spawner_url: None,
            spawn_variant: None,
            flake_ref: None,
            dev_shell: None,
            cpu: None,
            memory_mb: None,
            ttl_seconds: None,
            keep: false,
        }
    }

    #[test]
    fn resolve_args_applies_defaults_and_overrides() {
        let defaults = resolve_args(&args_with_defaults());
        assert_eq!(defaults.spawner_url, DEFAULT_SPAWNER_URL);
        assert_eq!(defaults.spawn_variant, "prebuilt-cow");
        assert_eq!(defaults.flake_ref, DEFAULT_FLAKE_REF);
        assert_eq!(defaults.dev_shell, DEFAULT_DEV_SHELL);
        assert_eq!(defaults.cpu, DEFAULT_CPU);
        assert_eq!(defaults.memory_mb, DEFAULT_MEMORY_MB);
        assert_eq!(defaults.ttl_seconds, DEFAULT_TTL_SECONDS);
        assert!(!defaults.keep);

        let overridden = resolve_args(&AgentNewMicrovmArgs {
            spawner_url: Some("http://10.0.0.5:8080".to_string()),
            spawn_variant: Some(MicrovmSpawnVariant::Prebuilt),
            flake_ref: Some(".#nixpi".to_string()),
            dev_shell: Some("dev".to_string()),
            cpu: Some(2),
            memory_mb: Some(2048),
            ttl_seconds: Some(600),
            keep: true,
        });
        assert_eq!(overridden.spawner_url, "http://10.0.0.5:8080");
        assert_eq!(overridden.spawn_variant, "prebuilt");
        assert_eq!(overridden.flake_ref, ".#nixpi");
        assert_eq!(overridden.dev_shell, "dev");
        assert_eq!(overridden.cpu, 2);
        assert_eq!(overridden.memory_mb, 2048);
        assert_eq!(overridden.ttl_seconds, 600);
        assert!(overridden.keep);
    }

    #[test]
    fn build_create_vm_request_serializes_guest_autostart() {
        let resolved = resolve_args(&args_with_defaults());
        let keys = Keys::generate();
        let bot_keys = Keys::generate();
        let req = build_create_vm_request(
            &resolved,
            keys.public_key(),
            &[
                "wss://us-east.nostr.pikachat.org".to_string(),
                "wss://eu.nostr.pikachat.org".to_string(),
            ],
            &bot_keys.secret_key().to_secret_hex(),
            &bot_keys.public_key().to_hex(),
        );
        let value = serde_json::to_value(req).expect("serialize create vm request");

        assert_eq!(value["spawn_variant"], "prebuilt-cow");
        assert_eq!(value["guest_autostart"]["command"], AUTOSTART_COMMAND);
        assert_eq!(
            value["guest_autostart"]["env"]["PIKA_OWNER_PUBKEY"],
            keys.public_key().to_hex()
        );
        assert_eq!(
            value["guest_autostart"]["env"]["PIKA_RELAY_URLS"],
            "wss://us-east.nostr.pikachat.org,wss://eu.nostr.pikachat.org"
        );
        assert!(
            value["guest_autostart"]["files"][AUTOSTART_SCRIPT_PATH]
                .as_str()
                .expect("autostart script")
                .contains("starting daemon")
        );
        assert!(
            value["guest_autostart"]["files"][AUTOSTART_BRIDGE_PATH]
                .as_str()
                .expect("bridge script")
                .contains("publish_keypackage")
        );
        let identity_text = value["guest_autostart"]["files"][AUTOSTART_IDENTITY_PATH]
            .as_str()
            .expect("identity file");
        let identity_json: serde_json::Value =
            serde_json::from_str(identity_text).expect("parse identity file");
        assert_eq!(
            identity_json["public_key_hex"],
            serde_json::Value::String(bot_keys.public_key().to_hex())
        );
    }

    #[test]
    fn keep_flag_controls_teardown_policy() {
        assert_eq!(teardown_policy(false), MicrovmTeardownPolicy::DeleteOnExit);
        assert_eq!(teardown_policy(true), MicrovmTeardownPolicy::KeepVm);
    }
}
