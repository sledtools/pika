use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, anyhow, bail};

struct CommandOutput {
    output: Output,
    command: String,
}

fn run_output(cmd: &mut Command, context: &str) -> Result<Output> {
    let command_output = run_output_raw(cmd, context)?;
    if !command_output.output.status.success() {
        let desc = command_output.command;
        bail!(
            "{context}: `{desc}` failed with status {}\nstdout:\n{}\nstderr:\n{}",
            command_output.output.status,
            String::from_utf8_lossy(&command_output.output.stdout),
            String::from_utf8_lossy(&command_output.output.stderr)
        );
    }
    Ok(command_output.output)
}

fn run_output_raw(cmd: &mut Command, context: &str) -> Result<CommandOutput> {
    let desc = command_description(cmd);
    let output = cmd
        .output()
        .with_context(|| format!("{context}: spawn failed for `{desc}`"))?;
    Ok(CommandOutput {
        output,
        command: desc,
    })
}

fn command_description(cmd: &Command) -> String {
    let mut parts = Vec::new();
    parts.push(cmd.get_program().to_string_lossy().to_string());
    parts.extend(cmd.get_args().map(|arg| arg.to_string_lossy().to_string()));
    parts.join(" ")
}

pub(crate) fn command_exists(binary: &str) -> bool {
    let candidate = Path::new(binary);
    if candidate.is_absolute() || binary.contains('/') {
        return candidate.is_file();
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths)
        .map(|dir| dir.join(binary))
        .any(|path| path.is_file())
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
    if let Ok(from_env) = std::env::var("OPENCLAW_DIR")
        && !from_env.trim().is_empty()
    {
        return Ok(PathBuf::from(from_env));
    }

    let direct = root.join("openclaw");
    if direct.join("package.json").is_file() {
        return Ok(direct);
    }

    if let Some(parent) = root.parent() {
        let sibling = parent.join("openclaw");
        if sibling.join("package.json").is_file() {
            return Ok(sibling);
        }
    }

    Ok(direct)
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

    if !command_exists("python3") {
        bail!("python3 is required to generate an ephemeral nsec for local UI E2E");
    }

    let script = r#"
import secrets
CHARSET='qpzry9x8gf2tvdw0s3jn54khce6mua7l'
def bech32_polymod(values):
  GEN=[0x3b6a57b2,0x26508e6d,0x1ea119fa,0x3d4233dd,0x2a1462b3]
  chk=1
  for v in values:
    b=chk>>25;chk=((chk&0x1ffffff)<<5)^v
    for i in range(5): chk^=GEN[i] if((b>>i)&1)else 0
  return chk
def bech32_hrp_expand(hrp):
  return [ord(x)>>5 for x in hrp]+[0]+[ord(x)&31 for x in hrp]
def bech32_create_checksum(hrp,data):
  values=bech32_hrp_expand(hrp)+data
  polymod=bech32_polymod(values+[0,0,0,0,0,0])^1
  return [(polymod>>5*(5-i))&31 for i in range(6)]
def convertbits(data,frombits,tobits,pad=True):
  acc=0;bits=0;ret=[];maxv=(1<<tobits)-1
  for b in data:
    acc=(acc<<frombits)|b;bits+=frombits
    while bits>=tobits: bits-=tobits;ret.append((acc>>bits)&maxv)
  if pad and bits: ret.append((acc<<(tobits-bits))&maxv)
  return ret
sk=secrets.token_bytes(32)
data5=convertbits(list(sk),8,5,True)
combined=data5+bech32_create_checksum('nsec',data5)
print('nsec'+'1'+''.join([CHARSET[d] for d in combined]))
"#;

    let output = run_output(
        Command::new("python3").arg("-c").arg(script),
        "generate ephemeral nsec",
    )?;
    let generated = String::from_utf8(output.stdout)?.trim().to_string();
    if generated.is_empty() {
        bail!("python nsec generator returned empty output");
    }
    if !generated.starts_with("nsec1") {
        bail!("python nsec generator returned invalid bech32 nsec: {generated}");
    }
    eprintln!(
        "note: generated ephemeral local e2e nsec (set PIKA_UI_E2E_NSEC or .pikachat-test-nsec to override)"
    );
    Ok(generated)
}

pub(crate) fn in_ci() -> bool {
    env_truthy("CI") || env_truthy("GITHUB_ACTIONS")
}

pub(crate) fn env_truthy(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| {
            let s = v.trim().to_ascii_lowercase();
            s == "1" || s == "true" || s == "yes" || s == "on"
        })
        .unwrap_or(false)
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

pub(crate) fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub(crate) fn check_mdk_skew(rust_interop_dir: &Path) -> Result<()> {
    let mdk_dir = std::env::var("MDK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_home().unwrap_or_default().join("code/mdk"));

    if !mdk_dir.join(".git").is_dir() {
        return Ok(());
    }

    let mdk_head = String::from_utf8(
        run_output(
            Command::new("git")
                .current_dir(&mdk_dir)
                .args(["rev-parse", "HEAD"]),
            "read mdk git HEAD",
        )?
        .stdout,
    )?
    .trim()
    .to_string();

    let harness_cargo = rust_interop_dir.join("rust_harness/Cargo.toml");
    let harness_text = fs::read_to_string(&harness_cargo)
        .with_context(|| format!("read {}", harness_cargo.display()))?;
    let harness_toml: toml::Value = toml::from_str(&harness_text)
        .with_context(|| format!("parse {}", harness_cargo.display()))?;

    let harness_rev = harness_toml
        .get("dependencies")
        .and_then(|deps| deps.get("mdk-core"))
        .and_then(|dep| dep.get("rev"))
        .and_then(toml::Value::as_str)
        .filter(|rev| rev.len() == 40 && rev.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_string);

    let Some(harness_rev) = harness_rev else {
        return Ok(());
    };

    if mdk_head != harness_rev {
        bail!(
            "MDK version skew detected\n  pika uses local MDK at: {} (HEAD={})\n  rust harness pins MDK rev: {}\nfix: align one side before interop conclusions",
            mdk_dir.display(),
            mdk_head,
            harness_rev,
        );
    }

    println!("ok: MDK rev aligned: {mdk_head}");
    Ok(())
}

pub(crate) fn extract_field(line: &str, key: &str) -> Option<String> {
    let value = line.split(key).nth(1)?;
    Some(value.split_whitespace().next()?.to_string())
}
