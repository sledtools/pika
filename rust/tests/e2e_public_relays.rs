use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nostr_sdk::prelude::*;
use pika_core::{AppAction, AppReconciler, AppUpdate, AuthState, FfiApp};
use tempfile::tempdir;

fn relay_urls() -> Vec<String> {
    if let Ok(s) = std::env::var("PIKA_E2E_RELAYS") {
        let v: Vec<String> = s
            .split(',')
            .map(|x| x.trim())
            .filter(|x| !x.is_empty())
            .map(|x| x.to_string())
            .collect();
        if !v.is_empty() {
            return v;
        }
    }
    vec![
        "wss://relay.damus.io".into(),
        "wss://relay.primal.net".into(),
    ]
}

fn write_config(data_dir: &str, relays: &[String]) {
    let path = std::path::Path::new(data_dir).join("pika_config.json");
    let v = serde_json::json!({
        "disable_network": false,
        "relay_urls": relays,
    });
    std::fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
}

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("{what}: condition not met within {timeout:?}");
}

#[derive(Clone)]
struct Collector {
    updates: Arc<Mutex<Vec<AppUpdate>>>,
}

impl Collector {
    fn new() -> Self {
        Collector {
            updates: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn toasts(&self) -> Vec<String> {
        self.updates
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
        self.updates.lock().unwrap().push(update);
    }
}

async fn wait_for_kind443_on_any_relay(
    relays: &[String],
    pubkey: PublicKey,
    timeout: Duration,
) -> bool {
    let keys = Keys::generate();
    let client = Client::new(keys);
    for r in relays {
        let _ = client.add_relay(r.as_str()).await;
    }
    client.connect().await;
    client.wait_for_connection(Duration::from_secs(5)).await;

    let filter = Filter::new()
        .author(pubkey)
        .kind(Kind::MlsKeyPackage)
        .limit(1);
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(events) = client
            .fetch_events(filter.clone(), Duration::from_secs(5))
            .await
        {
            if events.into_iter().any(|_| true) {
                client.shutdown().await;
                return true;
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    client.shutdown().await;
    false
}

// This is intentionally NOT part of CI: public relays are nondeterministic (rate limits, downtime,
// kind restrictions, eventual consistency).
//
// Run manually:
//   PIKA_E2E_PUBLIC=1 cargo test -p pika_core --test e2e_public_relays -- --ignored --nocapture
// Optional:
//   PIKA_E2E_RELAYS="wss://relay.damus.io,wss://relay.primal.net" ...
#[test]
#[ignore]
fn alice_sends_bob_over_public_relays() {
    if std::env::var("PIKA_E2E_PUBLIC").ok().as_deref() != Some("1") {
        eprintln!("skipping: set PIKA_E2E_PUBLIC=1 to run this test (it uses public relays)");
        return;
    }

    let relays = relay_urls();
    eprintln!("public relays: {relays:?}");

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config(&dir_a.path().to_string_lossy(), &relays);
    write_config(&dir_b.path().to_string_lossy(), &relays);

    let alice = FfiApp::new(dir_a.path().to_string_lossy().to_string(), String::new());
    let bob = FfiApp::new(dir_b.path().to_string_lossy().to_string(), String::new());

    let bob_collector = Collector::new();
    bob.listen_for_updates(Box::new(bob_collector.clone()));

    alice.dispatch(AppAction::CreateAccount);
    bob.dispatch(AppAction::CreateAccount);

    wait_until("alice logged in", Duration::from_secs(10), || {
        matches!(alice.state().auth, AuthState::LoggedIn { .. })
    });
    wait_until("bob logged in", Duration::from_secs(10), || {
        matches!(bob.state().auth, AuthState::LoggedIn { .. })
    });

    let (bob_npub, bob_pubkey_hex) = match bob.state().auth {
        AuthState::LoggedIn { npub, pubkey } => (npub, pubkey),
        _ => unreachable!(),
    };
    let bob_pubkey = PublicKey::parse(&bob_pubkey_hex).expect("pubkey parse");

    // Wait for Bob's key package to be visible on at least one configured relay.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let kp_ok = rt.block_on(wait_for_kind443_on_any_relay(
        &relays,
        bob_pubkey,
        Duration::from_secs(60),
    ));
    if !kp_ok {
        panic!(
            "bob key package (kind 443) not observed on relays within timeout; relays={relays:?}; bob_pubkey={bob_pubkey_hex}; toasts={:?}",
            bob_collector.toasts()
        );
    }

    alice.dispatch(AppAction::CreateChat {
        peer_npub: bob_npub,
    });

    wait_until("alice chat opened", Duration::from_secs(90), || {
        alice.state().current_chat.is_some()
    });
    wait_until("bob has chat", Duration::from_secs(90), || {
        !bob.state().chat_list.is_empty()
    });

    let chat_id = alice.state().current_chat.as_ref().unwrap().chat_id.clone();
    wait_until("bob chat id matches", Duration::from_secs(90), || {
        bob.state().chat_list.iter().any(|c| c.chat_id == chat_id)
    });

    alice.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: "hi-from-alice-public".into(),
        kind: None,
        reply_to_message_id: None,
    });

    wait_until(
        "bob preview/unread updated",
        Duration::from_secs(90),
        || {
            bob.state()
                .chat_list
                .iter()
                .find(|c| c.chat_id == chat_id)
                .map(|c| {
                    c.unread_count > 0 || c.last_message.as_deref() == Some("hi-from-alice-public")
                })
                .unwrap_or(false)
        },
    );

    // Verify Alice's message transitions from Pending to Sent (publish confirmed by relays).
    wait_until(
        "alice message delivery state is Sent",
        Duration::from_secs(30),
        || {
            alice
                .state()
                .current_chat
                .as_ref()
                .and_then(|c| {
                    c.messages
                        .iter()
                        .find(|m| m.content == "hi-from-alice-public")
                })
                .map(|m| matches!(m.delivery, pika_core::MessageDeliveryState::Sent))
                .unwrap_or(false)
        },
    );

    bob.dispatch(AppAction::OpenChat { chat_id });
    wait_until(
        "bob opened chat has message",
        Duration::from_secs(90),
        || {
            bob.state()
                .current_chat
                .as_ref()
                .and_then(|c| {
                    c.messages
                        .iter()
                        .find(|m| m.content == "hi-from-alice-public")
                })
                .is_some()
        },
    );
}
