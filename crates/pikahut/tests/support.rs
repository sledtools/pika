use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use pika_core::{AppAction, AuthState, CallStatus, FfiApp};
use pikahut::config::ProfileName;
use pikahut::testing::{FixtureHandle, FixtureSpec, TestContext, start_fixture};

#[derive(Clone, Copy, Debug)]
struct CallStatsSnapshot {
    tx_frames: u64,
    rx_frames: u64,
    jitter_buffer_ms: u32,
}

pub fn run_call_over_local_moq_relay(context: &TestContext) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let fixture_context = fixture_context(context)?;
    let fixture = match start_relay_and_moq(&fixture_context) {
        Ok(fixture) => fixture,
        Err(err) => {
            preserve_fixture_diagnostics(context, fixture_context.state_dir())
                .context("preserve fixture startup diagnostics")?;
            return Err(err);
        }
    };
    let result = (|| -> Result<()> {
        let relay_url = fixture
            .relay_url()
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("fixture manifest missing relay_url"))?;
        let moq_url = fixture
            .manifest()
            .moq_url
            .as_deref()
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("fixture manifest missing moq_url"))?;

        let alice_dir = context.state_dir().join("alice");
        let bob_dir = context.state_dir().join("bob");
        write_config_with_moq(&alice_dir, &relay_url, Some(&relay_url), &moq_url)?;
        write_config_with_moq(&bob_dir, &relay_url, Some(&relay_url), &moq_url)?;

        let alice = FfiApp::new(path_arg(&alice_dir), String::new(), String::new());
        let bob = FfiApp::new(path_arg(&bob_dir), String::new(), String::new());

        alice.dispatch(AppAction::CreateAccount);
        bob.dispatch(AppAction::CreateAccount);
        wait_until("alice logged in", Duration::from_secs(10), || {
            matches!(alice.state().auth, AuthState::LoggedIn { .. })
        })?;
        wait_until("bob logged in", Duration::from_secs(10), || {
            matches!(bob.state().auth, AuthState::LoggedIn { .. })
        })?;

        let bob_npub = match bob.state().auth {
            AuthState::LoggedIn { npub, .. } => npub,
            _ => bail!("bob failed to enter logged-in state"),
        };

        let chat_id = create_or_open_dm_chat(&alice, &bob_npub, Duration::from_secs(90))?;
        wait_until("bob chat id matches", Duration::from_secs(45), || {
            bob.state().chat_list.iter().any(|c| c.chat_id == chat_id)
        })?;

        alice.dispatch(AppAction::StartCall {
            chat_id: chat_id.clone(),
        });
        wait_until("alice offering", Duration::from_secs(10), || {
            alice
                .state()
                .active_call
                .as_ref()
                .map(|c| matches!(c.status, CallStatus::Offering))
                .unwrap_or(false)
        })?;
        wait_until("bob ringing", Duration::from_secs(10), || {
            bob.state()
                .active_call
                .as_ref()
                .map(|c| matches!(c.status, CallStatus::Ringing))
                .unwrap_or(false)
        })?;

        bob.dispatch(AppAction::AcceptCall {
            chat_id: chat_id.clone(),
        });
        wait_until("bob connecting or active", Duration::from_secs(30), || {
            bob.state()
                .active_call
                .as_ref()
                .map(|c| matches!(c.status, CallStatus::Connecting | CallStatus::Active))
                .unwrap_or(false)
        })?;
        wait_until(
            "alice connecting or active",
            Duration::from_secs(30),
            || {
                alice
                    .state()
                    .active_call
                    .as_ref()
                    .map(|c| matches!(c.status, CallStatus::Connecting | CallStatus::Active))
                    .unwrap_or(false)
            },
        )?;

        wait_until(
            "alice active with tx+rx frames",
            Duration::from_secs(30),
            || {
                alice
                    .state()
                    .active_call
                    .as_ref()
                    .map(|c| {
                        matches!(c.status, CallStatus::Active)
                            && c.debug
                                .as_ref()
                                .map(|d| d.tx_frames > 5 && d.rx_frames > 5)
                                .unwrap_or(false)
                    })
                    .unwrap_or(false)
            },
        )?;
        wait_until(
            "bob active with tx+rx frames",
            Duration::from_secs(30),
            || {
                bob.state()
                    .active_call
                    .as_ref()
                    .map(|c| {
                        matches!(c.status, CallStatus::Active)
                            && c.debug
                                .as_ref()
                                .map(|d| d.tx_frames > 5 && d.rx_frames > 5)
                                .unwrap_or(false)
                    })
                    .unwrap_or(false)
            },
        )?;

        let alice_snap = call_stats_snapshot(&alice)?;
        let bob_snap = call_stats_snapshot(&bob)?;
        std::thread::sleep(Duration::from_secs(2));
        let alice_after = call_stats_snapshot(&alice)?;
        let bob_after = call_stats_snapshot(&bob)?;
        anyhow::ensure!(
            alice_after.tx_frames > alice_snap.tx_frames,
            "alice should keep transmitting"
        );
        anyhow::ensure!(
            bob_after.rx_frames > bob_snap.rx_frames,
            "bob should keep receiving"
        );
        anyhow::ensure!(
            alice_after.jitter_buffer_ms <= 240,
            "alice jitter buffer should stay bounded, got {}ms",
            alice_after.jitter_buffer_ms
        );
        anyhow::ensure!(
            bob_after.jitter_buffer_ms <= 240,
            "bob jitter buffer should stay bounded, got {}ms",
            bob_after.jitter_buffer_ms
        );

        alice.dispatch(AppAction::EndCall);
        wait_until("alice call ended", Duration::from_secs(10), || {
            alice
                .state()
                .active_call
                .as_ref()
                .map(|c| matches!(c.status, CallStatus::Ended { .. }))
                .unwrap_or(true)
        })?;
        wait_until("bob call ended", Duration::from_secs(10), || {
            bob.state()
                .active_call
                .as_ref()
                .map(|c| matches!(c.status, CallStatus::Ended { .. }))
                .unwrap_or(true)
        })?;

        if let Some(debug) = alice
            .state()
            .active_call
            .as_ref()
            .and_then(|c| c.debug.as_ref())
        {
            eprintln!(
                "alice final: tx={} rx={} dropped={}",
                debug.tx_frames, debug.rx_frames, debug.rx_dropped
            );
        }
        if let Some(debug) = bob
            .state()
            .active_call
            .as_ref()
            .and_then(|c| c.debug.as_ref())
        {
            eprintln!(
                "bob final: tx={} rx={} dropped={}",
                debug.tx_frames, debug.rx_frames, debug.rx_dropped
            );
        }

        Ok(())
    })();

    if let Err(err) = result {
        preserve_fixture_diagnostics(context, fixture.state_dir())
            .context("preserve fixture diagnostics after selector failure")?;
        return Err(err);
    }

    Ok(())
}

fn fixture_context(context: &TestContext) -> Result<TestContext> {
    // Keep the fixture under a child path so fixture teardown never removes the
    // selector root that PreserveOnFailure is responsible for retaining.
    TestContext::builder(format!("{}-fixture", context.run_name()))
        .state_dir(context.state_dir().join("fixture"))
        .build()
}

fn start_relay_and_moq(context: &TestContext) -> Result<FixtureHandle> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create tokio runtime for local moq call boundary")?;
    runtime
        .block_on(start_fixture(
            context,
            &FixtureSpec::builder(ProfileName::RelayMoq).build(),
        ))
        .context("start relay+moq fixture")
}

fn preserve_fixture_diagnostics(context: &TestContext, fixture_state_dir: &Path) -> Result<()> {
    if !fixture_state_dir.exists() {
        return Ok(());
    }

    let snapshot_dir = context.ensure_artifact_subdir("fixture-state")?;
    copy_tree(fixture_state_dir, &snapshot_dir)?;
    context.write_artifact(
        "fixture-state/source-path.txt",
        format!("{}\n", fixture_state_dir.display()),
    )?;
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("create snapshot dir {}", destination.display()))?;

    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());

        if file_type.is_dir() {
            copy_tree(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "copy fixture artifact {} -> {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn write_config_with_moq(
    data_dir: &Path,
    relay_url: &str,
    kp_relay_url: Option<&str>,
    moq_url: &str,
) -> Result<()> {
    fs::create_dir_all(data_dir)
        .with_context(|| format!("create config dir {}", data_dir.display()))?;
    let path = data_dir.join("pika_config.json");
    let mut value = serde_json::json!({
        "disable_network": false,
        "disable_agent_allowlist_probe": true,
        "relay_urls": [relay_url],
        "call_moq_url": moq_url,
        "call_broadcast_prefix": "pika/calls",
        "call_audio_backend": "synthetic",
    });
    if let Some(kp) = kp_relay_url {
        value.as_object_mut().expect("config object").insert(
            "key_package_relay_urls".to_string(),
            serde_json::json!([kp]),
        );
    }
    fs::write(
        &path,
        serde_json::to_vec(&value).context("serialize config")?,
    )
    .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!("{what}: condition not met within {timeout:?}");
}

fn call_stats_snapshot(app: &FfiApp) -> Result<CallStatsSnapshot> {
    let call = app
        .state()
        .active_call
        .ok_or_else(|| anyhow!("missing active call state"))?;
    let debug = call
        .debug
        .ok_or_else(|| anyhow!("missing call debug stats"))?;
    Ok(CallStatsSnapshot {
        tx_frames: debug.tx_frames,
        rx_frames: debug.rx_frames,
        jitter_buffer_ms: debug.jitter_buffer_ms,
    })
}

fn dm_chat_id_for_peer(app: &FfiApp, peer_npub: &str) -> Option<String> {
    let state = app.state();
    if let Some(chat) = state
        .current_chat
        .as_ref()
        .filter(|chat| chat.members.iter().any(|member| member.npub == peer_npub))
    {
        return Some(chat.chat_id.clone());
    }
    state
        .chat_list
        .iter()
        .find(|chat| chat.members.iter().any(|member| member.npub == peer_npub))
        .map(|chat| chat.chat_id.clone())
}

fn create_or_open_dm_chat(app: &FfiApp, peer_npub: &str, timeout: Duration) -> Result<String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(chat_id) = dm_chat_id_for_peer(app, peer_npub) {
            app.dispatch(AppAction::OpenChat {
                chat_id: chat_id.clone(),
            });
            wait_until("chat opened", Duration::from_secs(30), || {
                app.state()
                    .current_chat
                    .as_ref()
                    .map(|chat| chat.chat_id == chat_id)
                    .unwrap_or(false)
            })?;
            return Ok(chat_id);
        }
        app.dispatch(AppAction::CreateChat {
            peer_npub: peer_npub.to_owned(),
        });
        std::thread::sleep(Duration::from_millis(500));
    }
    bail!("chat for peer {peer_npub} was not ready within {timeout:?}");
}
