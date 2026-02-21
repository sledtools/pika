use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use pika_core::{AppAction, AppReconciler, AppUpdate, AuthState, FfiApp};

fn write_config(data_dir: &str, relay_url: &str) -> Result<()> {
    let path = std::path::Path::new(data_dir).join("pika_config.json");
    let v = serde_json::json!({
        "disable_network": false,
        "relay_urls": [relay_url],
    });
    std::fs::write(path, serde_json::to_vec(&v).unwrap()).context("write pika_config.json")?;
    Ok(())
}

fn make_tmp_data_dir() -> Result<String> {
    let base = std::env::temp_dir().join(format!(
        "pika_interop_rustbot_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    std::fs::create_dir_all(&base).context("create temp data dir")?;
    Ok(base.to_string_lossy().to_string())
}

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(anyhow!("{what}: condition not met within {timeout:?}"))
}

#[derive(Clone, Default)]
struct Collector(std::sync::Arc<std::sync::Mutex<Vec<AppUpdate>>>);

impl Collector {
    fn toasts(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|u| match u {
                AppUpdate::FullState(s) => s.toast.clone(),
                _ => None,
            })
            .collect()
    }
}

impl AppReconciler for Collector {
    fn reconcile(&self, update: AppUpdate) {
        self.0.lock().unwrap().push(update);
    }
}

fn main() -> Result<()> {
    let relay_url = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow!("usage: interop_rustbot_baseline <relay_url> <peer_npub>"))?;
    let peer_npub = std::env::args()
        .nth(2)
        .ok_or_else(|| anyhow!("usage: interop_rustbot_baseline <relay_url> <peer_npub>"))?;

    let data_dir = make_tmp_data_dir()?;
    write_config(&data_dir, &relay_url)?;

    let app = FfiApp::new(data_dir, String::new());
    let collector = Collector::default();
    app.listen_for_updates(Box::new(collector.clone()));

    app.dispatch(AppAction::CreateAccount);
    wait_until("logged in", Duration::from_secs(20), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    })
    .with_context(|| format!("toasts={:?}", collector.toasts()))?;

    app.dispatch(AppAction::CreateChat { peer_npub });
    wait_until("chat opened", Duration::from_secs(90), || {
        app.state().current_chat.is_some()
    })
    .with_context(|| format!("toasts={:?}", collector.toasts()))?;

    let chat_id = app
        .state()
        .current_chat
        .as_ref()
        .ok_or_else(|| anyhow!("expected current_chat"))?
        .chat_id
        .clone();

    app.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: "ping".to_string(),
        kind: None,
        reply_to_message_id: None,
    });

    // Expect the peer to reply "pong".
    wait_until("received pong", Duration::from_secs(90), || {
        app.state()
            .current_chat
            .as_ref()
            .filter(|c| c.chat_id == chat_id)
            .map(|c| c.messages.iter().any(|m| m.content.trim() == "pong"))
            .unwrap_or(false)
    })
    .with_context(|| format!("toasts={:?}", collector.toasts()))?;

    println!("ok: interop baseline (rust bot) PASS");
    Ok(())
}
