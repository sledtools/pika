use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use bech32::{Bech32, Hrp};
use rand::RngCore;

use crate::testing::util::{non_empty_env_path, resolve_openclaw_dir_default};

pub(crate) fn command_exists(binary: &str) -> bool {
    crate::testing::util::command_exists(binary)
}

pub(crate) fn pick_free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

pub(crate) fn parse_url_port(url: &str) -> Result<u16> {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host_port = after_scheme.split('/').next().unwrap_or(after_scheme);
    let port_str = host_port
        .rsplit_once(':')
        .map(|(_, port)| port)
        .ok_or_else(|| anyhow!("URL has no port: {url}"))?;
    port_str
        .parse::<u16>()
        .with_context(|| format!("invalid port in URL: {url}"))
}

pub(crate) fn tail_lines(path: &Path, count: usize) -> String {
    let Ok(content) = fs::read_to_string(path) else {
        return String::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

pub(crate) fn resolve_openclaw_dir(root: &Path, cli_value: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(dir) = cli_value {
        return Ok(dir);
    }
    if let Some(path) = non_empty_env_path("OPENCLAW_DIR") {
        return Ok(path);
    }
    Ok(resolve_openclaw_dir_default(root))
}

pub(crate) fn resolve_ui_client_nsec(root: &Path) -> Result<String> {
    if let Ok(nsec) = std::env::var("PIKA_UI_E2E_NSEC")
        && !nsec.trim().is_empty()
    {
        return Ok(nsec);
    }

    let nsec_file = root.join(".pikachat-test-nsec");
    if nsec_file.is_file() {
        let s = fs::read_to_string(&nsec_file)?;
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    let generated = generate_ephemeral_nsec()?;
    eprintln!(
        "note: generated ephemeral local e2e nsec (set PIKA_UI_E2E_NSEC or .pikachat-test-nsec to override)"
    );
    Ok(generated)
}

fn generate_ephemeral_nsec() -> Result<String> {
    let mut secret = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut secret);
    let hrp = Hrp::parse("nsec").context("parse nsec bech32 prefix")?;
    let generated = bech32::encode::<Bech32>(hrp, &secret).context("encode bech32 nsec")?;
    if !generated.starts_with("nsec1") {
        bail!("generated invalid bech32 nsec: {generated}");
    }
    Ok(generated)
}

pub(crate) fn in_ci() -> bool {
    env_truthy("CI") || env_truthy("GITHUB_ACTIONS")
}

pub(crate) fn env_truthy(key: &str) -> bool {
    crate::testing::util::env_truthy(key)
}

pub(crate) fn extract_udid(output: &str) -> Option<String> {
    for line in output.lines() {
        let prefix = "ok: ios simulator ready (udid=";
        if let Some(rest) = line.strip_prefix(prefix)
            && let Some(udid) = rest.strip_suffix(')')
        {
            return Some(udid.to_string());
        }
    }
    None
}

pub(crate) fn extract_field(line: &str, key: &str) -> Option<String> {
    let value = line.split(key).nth(1)?;
    Some(value.split_whitespace().next()?.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::{
        extract_udid, generate_ephemeral_nsec, parse_url_port, pick_free_port, tail_lines,
    };

    #[test]
    fn parse_url_port_extracts_port() {
        assert_eq!(
            parse_url_port("ws://127.0.0.1:7777").expect("parse port"),
            7777
        );
    }

    #[test]
    fn parse_url_port_rejects_missing_port() {
        let err = parse_url_port("ws://127.0.0.1").expect_err("missing port should fail");
        assert!(err.to_string().contains("URL has no port"));
    }

    #[test]
    fn pick_free_port_returns_bindable_port() {
        let port = pick_free_port().expect("pick port");
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", port)).expect("port should be bindable");
        drop(listener);
    }

    #[test]
    fn extract_udid_parses_ios_sim_ensure_output() {
        let output = "ok: ios simulator ready (udid=C128E86D-D60E-44B6-B8C4-EC3480D6BC9F)\n";
        assert_eq!(
            extract_udid(output),
            Some("C128E86D-D60E-44B6-B8C4-EC3480D6BC9F".to_string())
        );
    }

    #[test]
    fn tail_lines_reads_requested_suffix() {
        let temp = tempfile::NamedTempFile::new().expect("temp file");
        let mut file = std::fs::File::create(temp.path()).expect("open temp");
        writeln!(file, "a\nb\nc").expect("write temp");
        assert_eq!(tail_lines(temp.path(), 2), "b\nc");
    }

    #[test]
    fn generate_ephemeral_nsec_returns_valid_bech32() {
        let generated = generate_ephemeral_nsec().expect("generate nsec");
        assert!(generated.starts_with("nsec1"));
        assert!(generated.len() > "nsec1".len());
    }
}
