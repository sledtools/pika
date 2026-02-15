use std::time::{Duration, Instant};

use nostr_sdk::prelude::{Client, Filter, Keys, Kind, PublicKey, RelayPoolNotification};
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

fn key_package_relay_urls() -> Vec<String> {
    if let Ok(s) = std::env::var("PIKA_E2E_KP_RELAYS") {
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
        "wss://nostr-pub.wellorder.net".into(),
        "wss://nostr-01.yakihonne.com".into(),
        "wss://nostr-02.yakihonne.com".into(),
        "wss://relay.satlantis.io".into(),
    ]
}

fn write_config(data_dir: &str, relays: &[String], kp_relays: &[String]) {
    let path = std::path::Path::new(data_dir).join("pika_config.json");
    let v = serde_json::json!({
        "disable_network": false,
        "relay_urls": relays,
        "key_package_relay_urls": kp_relays,
    });
    std::fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
}

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("{what}: condition not met within {timeout:?}");
}

#[derive(Clone)]
struct Collector(std::sync::Arc<std::sync::Mutex<Vec<AppUpdate>>>);
impl Collector {
    fn new() -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(Vec::new())))
    }
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

async fn wait_for_kind443_on_any_relay(
    relays: &[String],
    pubkey: PublicKey,
    timeout: Duration,
) -> bool {
    let keys = Keys::generate();
    let client = Client::new(keys);
    for r in relays {
        let _ = client.add_relay(r).await;
    }
    client.connect().await;

    // Subscribe + watch notifications; more reliable than a single fetch across multiple relays.
    let filter = Filter::new()
        .author(pubkey)
        .kind(Kind::MlsKeyPackage)
        .limit(1);
    let _ = client.subscribe(filter, None).await;

    let mut rx = client.notifications();
    let start = Instant::now();
    while start.elapsed() < timeout {
        // `recv().await` can block indefinitely if no notifications arrive, which would
        // defeat our timeout. Poll with a short timeout so the loop can observe elapsed time.
        match tokio::time::timeout(Duration::from_millis(250), rx.recv()).await {
            Ok(Ok(RelayPoolNotification::Event { event, .. })) => {
                if event.kind == Kind::MlsKeyPackage && event.pubkey == pubkey {
                    return true;
                }
            }
            Ok(Ok(_other)) => {}
            Ok(Err(_closed)) => {
                // Notification channel closed; treat as failure.
                return false;
            }
            Err(_elapsed) => {
                // No notification in this polling interval.
            }
        }
    }
    false
}

// This is intentionally NOT part of CI: public relays are nondeterministic (rate limits, downtime,
// kind restrictions, eventual consistency).
//
// Run manually:
//   PIKA_E2E_PUBLIC=1 cargo test -p pika_core --test e2e_public_relays -- --ignored --nocapture
// Optional:
//   PIKA_E2E_RELAYS="wss://relay.damus.io,wss://relay.primal.net" ...
//   PIKA_E2E_KP_RELAYS="wss://nostr-pub.wellorder.net,wss://nostr-01.yakihonne.com,..." ...
#[test]
#[ignore]
fn alice_sends_bob_over_public_relays() {
    if std::env::var("PIKA_E2E_PUBLIC").ok().as_deref() != Some("1") {
        eprintln!("skipping: set PIKA_E2E_PUBLIC=1 to run this test (it uses public relays)");
        return;
    }

    let relays = relay_urls();
    let kp_relays = key_package_relay_urls();
    eprintln!("public relays (general): {relays:?}");
    eprintln!("public relays (key packages): {kp_relays:?}");

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config(&dir_a.path().to_string_lossy(), &relays, &kp_relays);
    write_config(&dir_b.path().to_string_lossy(), &relays, &kp_relays);

    let alice = FfiApp::new(dir_a.path().to_string_lossy().to_string());
    let bob = FfiApp::new(dir_b.path().to_string_lossy().to_string());

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
        &kp_relays,
        bob_pubkey,
        Duration::from_secs(60),
    ));
    if !kp_ok {
        panic!(
            "bob key package (kind 443) not observed on relays within timeout; kp_relays={kp_relays:?}; bob_pubkey={bob_pubkey_hex}; toasts={:?}",
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
