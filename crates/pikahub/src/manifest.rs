use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const MANIFEST_FILE: &str = "manifest.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_start_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_start_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_pubkey_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub postgres_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_npub: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_pubkey_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_start_time: Option<String>,
    pub state_dir: PathBuf,
    pub started_at: String,
}

impl Manifest {
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join(MANIFEST_FILE)
    }

    pub fn load(state_dir: &Path) -> Result<Option<Self>> {
        let p = Self::path(state_dir);
        if !p.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&p)
            .with_context(|| format!("read manifest: {}", p.display()))?;
        let m: Self = serde_json::from_str(&raw)
            .with_context(|| format!("parse manifest: {}", p.display()))?;
        Ok(Some(m))
    }

    pub fn save(&self, state_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(state_dir)?;
        let p = Self::path(state_dir);
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&p, format!("{json}\n"))
            .with_context(|| format!("write manifest: {}", p.display()))
    }

    pub fn remove(state_dir: &Path) -> Result<()> {
        let p = Self::path(state_dir);
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
        Ok(())
    }

    /// PIDs of non-postgres children in teardown order (bot, server, relay),
    /// paired with their recorded start times for safe kill verification.
    pub fn all_pids(&self) -> Vec<(u32, Option<&str>)> {
        let mut pids = Vec::new();
        if let Some(pid) = self.bot_pid {
            pids.push((pid, self.bot_start_time.as_deref()));
        }
        if let Some(pid) = self.server_pid {
            pids.push((pid, self.server_start_time.as_deref()));
        }
        if let Some(pid) = self.relay_pid {
            pids.push((pid, self.relay_start_time.as_deref()));
        }
        pids
    }

    pub fn env_exports(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();
        if let Some(ref url) = self.relay_url {
            env.push(("RELAY_EU".into(), url.clone()));
            env.push(("RELAY_US".into(), url.clone()));
        }
        if let Some(ref url) = self.server_url {
            env.push(("PIKA_SERVER_URL".into(), url.clone()));
        }
        if let Some(ref pk) = self.server_pubkey_hex {
            env.push(("PIKA_AGENT_CONTROL_SERVER_PUBKEY".into(), pk.clone()));
        }
        if let Some(ref url) = self.database_url {
            env.push(("DATABASE_URL".into(), url.clone()));
        }
        if let Some(ref npub) = self.bot_npub {
            env.push(("PIKA_E2E_BOT_NPUB".into(), npub.clone()));
        }
        if let Some(ref pk) = self.bot_pubkey_hex {
            env.push(("PIKA_E2E_BOT_PUBKEY".into(), pk.clone()));
        }
        env
    }
}

fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | ':' | '.' | '='))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

impl Manifest {
    pub fn shell_export_lines(&self) -> String {
        self.env_exports()
            .iter()
            .map(|(k, v)| format!("export {}={}", k, shell_quote(v)))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> Manifest {
        Manifest {
            profile: "backend".into(),
            relay_url: Some("ws://localhost:3334".into()),
            relay_pid: Some(1234),
            relay_start_time: Some("Mon Feb 26 00:00:00 2026".into()),
            server_url: Some("http://localhost:8080".into()),
            server_pid: Some(1235),
            server_start_time: Some("Mon Feb 26 00:00:01 2026".into()),
            server_pubkey_hex: Some("abc123".into()),
            database_url: Some("postgresql:///pika_server?host=/tmp/pgdata".into()),
            postgres_pid: Some(1236),
            bot_npub: None,
            bot_pubkey_hex: None,
            bot_pid: None,
            bot_start_time: None,
            state_dir: PathBuf::from("/tmp/test-state"),
            started_at: "2026-02-26T00:00:00Z".into(),
        }
    }

    #[test]
    fn manifest_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let m = sample_manifest();
        m.save(dir.path()).unwrap();
        let loaded = Manifest::load(dir.path()).unwrap().unwrap();
        assert_eq!(m, loaded);
    }

    #[test]
    fn manifest_load_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Manifest::load(dir.path()).unwrap().is_none());
    }

    #[test]
    fn manifest_remove_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        Manifest::remove(dir.path()).unwrap();
        let m = sample_manifest();
        m.save(dir.path()).unwrap();
        assert!(Manifest::path(dir.path()).exists());
        Manifest::remove(dir.path()).unwrap();
        assert!(!Manifest::path(dir.path()).exists());
        Manifest::remove(dir.path()).unwrap();
    }

    #[test]
    fn all_pids_teardown_order() {
        let mut m = sample_manifest();
        m.bot_pid = Some(100);
        m.bot_start_time = Some("Mon Feb 26 00:00:02 2026".into());
        let pids: Vec<u32> = m.all_pids().iter().map(|(p, _)| *p).collect();
        assert_eq!(pids, vec![100, 1235, 1234]);
    }

    #[test]
    fn env_exports_keys() {
        let m = sample_manifest();
        let exports = m.env_exports();
        let keys: Vec<&str> = exports.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"RELAY_EU"));
        assert!(keys.contains(&"RELAY_US"));
        assert!(keys.contains(&"PIKA_SERVER_URL"));
        assert!(keys.contains(&"PIKA_AGENT_CONTROL_SERVER_PUBKEY"));
        assert!(keys.contains(&"DATABASE_URL"));
    }

    #[test]
    fn shell_export_lines_quoted() {
        let mut m = sample_manifest();
        m.database_url = Some("postgresql:///db?host=/path with spaces/pgdata".into());
        let lines = m.shell_export_lines();
        assert!(lines.contains("'postgresql:///db?host=/path with spaces/pgdata'"));
    }

    #[test]
    fn shell_quote_safe_values_unquoted() {
        assert_eq!(shell_quote("ws://localhost:3334"), "ws://localhost:3334");
        assert_eq!(shell_quote("abc123"), "abc123");
    }

    #[test]
    fn shell_quote_unsafe_values_quoted() {
        assert_eq!(shell_quote("has space"), "'has space'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
