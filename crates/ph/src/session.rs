use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use nostr::{EventBuilder, Keys, Kind, Tag, TagKind};
use serde::{Deserialize, Serialize};
use url::Url;

const DEFAULT_BASE_URL: &str = "https://git.pikachat.org";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Session {
    pub(crate) base_url: String,
    pub(crate) token: String,
    pub(crate) npub: String,
    pub(crate) is_admin: bool,
    pub(crate) can_forge_write: bool,
}

pub(crate) fn build_nip98_verify_event_json(
    nsec: &str,
    base_url: &str,
    challenge: &str,
) -> anyhow::Result<String> {
    let keys = Keys::parse(nsec).context("parse Nostr signing key")?;
    let verify_url = format!("{}/news/auth/verify", base_url.trim_end_matches('/'));
    let event = EventBuilder::new(Kind::Custom(27235), challenge)
        .tags([
            Tag::custom(TagKind::custom("u"), [verify_url.as_str()]),
            Tag::custom(TagKind::custom("method"), ["POST"]),
        ])
        .sign_with_keys(&keys)
        .context("sign NIP-98 auth event")?;
    serde_json::to_string(&event).context("serialize signed NIP-98 auth event")
}

pub(crate) fn login_nsec(nsec: Option<&str>, nsec_file: Option<&Path>) -> anyhow::Result<String> {
    match (nsec, nsec_file) {
        (Some(value), None) => Ok(value.trim().to_string()),
        (None, Some(path)) => read_nsec_file(path),
        (None, None) => bail!("nsec is required; pass --nsec or --nsec-file"),
        (Some(_), Some(_)) => unreachable!("clap enforces conflicts"),
    }
}

fn read_nsec_file(path: &Path) -> anyhow::Result<String> {
    let raw = if path == Path::new("-") {
        let mut stdin = String::new();
        std::io::stdin()
            .read_to_string(&mut stdin)
            .context("read nsec from stdin")?;
        stdin
    } else {
        fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?
    };
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        bail!("nsec file is empty");
    }
    Ok(trimmed)
}

pub(crate) fn default_state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_STATE_HOME") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("ph");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed)
                .join(".local")
                .join("state")
                .join("ph");
        }
    }
    PathBuf::from(".ph")
}

fn session_path(state_dir: &Path) -> PathBuf {
    state_dir.join("session.json")
}

pub(crate) fn save_session(state_dir: &Path, session: &Session) -> anyhow::Result<()> {
    fs::create_dir_all(state_dir).with_context(|| format!("create {}", state_dir.display()))?;
    let path = session_path(state_dir);
    let bytes = serde_json::to_vec_pretty(session).context("serialize session")?;
    let mut file = fs::File::create(&path).with_context(|| format!("create {}", path.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod {}", path.display()))?;
    }
    Ok(())
}

pub(crate) fn load_session(state_dir: &Path) -> anyhow::Result<Session> {
    let path = session_path(state_dir);
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("read {} (run `ph login` first)", path.display()))?;
    serde_json::from_str(&raw).context("parse ph session")
}

pub(crate) fn remove_session(state_dir: &Path) -> anyhow::Result<bool> {
    let path = session_path(state_dir);
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
    Ok(true)
}

pub(crate) fn resolve_base_url(
    explicit: Option<&str>,
    session: Option<&Session>,
) -> anyhow::Result<String> {
    let base = explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| session.map(|session| session.base_url.clone()))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let url = Url::parse(&base).with_context(|| format!("parse base url {}", base))?;
    Ok(url.to_string().trim_end_matches('/').to_string())
}

pub(crate) fn resolve_authenticated_base_url(
    explicit: Option<&str>,
    session: &Session,
) -> anyhow::Result<String> {
    let session_base_url = resolve_base_url(None, Some(session))?;
    let explicit_base_url = resolve_base_url(explicit, None)?;
    if let Some(explicit) = explicit
        && !explicit.trim().is_empty()
        && explicit_base_url != session_base_url
    {
        bail!(
            "saved session belongs to {}; refusing to reuse its token for {}",
            session_base_url,
            explicit_base_url
        );
    }
    Ok(session_base_url)
}
