use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use nostr_sdk::ToBech32;
use nostr_sdk::prelude::{Keys, NostrSigner};
use pika_core::{
    AgentProvisioningPhase, AppAction, AuthMode, AuthState, BunkerConnectError,
    BunkerConnectErrorKind, BunkerConnectOutput, CallStatus, ExternalSignerErrorKind,
    ExternalSignerHandshakeResult, FfiApp, Screen,
};
use pikahut::config::ProfileName;
use pikahut::testing::{FixtureHandle, FixtureSpec, TestContext, start_fixture};

#[path = "../../../tests/support/nostr_connect.rs"]
mod nostr_connect_support;
use nostr_connect_support::{
    MockBunkerSignerConnector, MockExternalSignerBridge, SequenceBunkerSignerConnector,
    nostrconnect_metadata, query_param,
};

#[derive(Clone, Copy, Debug)]
struct CallStatsSnapshot {
    tx_frames: u64,
    rx_frames: u64,
    jitter_buffer_ms: u32,
}

struct ScopedEnvVar {
    key: String,
    previous: Option<String>,
}

impl ScopedEnvVar {
    fn set(key: &str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        // SAFETY: callers serialize env mutations before invoking helpers that
        // rely on temporary process-wide environment overrides.
        unsafe {
            std::env::set_var(key, value);
        }
        Self {
            key: key.to_string(),
            previous,
        }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        if let Some(previous) = &self.previous {
            // SAFETY: callers serialize env mutations before invoking helpers
            // that rely on temporary process-wide environment overrides.
            unsafe {
                std::env::set_var(&self.key, previous);
            }
        } else {
            // SAFETY: callers serialize env mutations before invoking helpers
            // that rely on temporary process-wide environment overrides.
            unsafe {
                std::env::remove_var(&self.key);
            }
        }
    }
}

pub fn run_call_over_local_moq_relay(context: &TestContext) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    with_relay_and_moq_fixture(context, |fixture| {
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
    })
}

// CI-facing readable DM contract: a new DM appears, the first message sends, and the peer sees
// that delivery through the same `FfiApp` state the apps render.
pub fn run_dm_creation_and_first_message_delivery(context: &TestContext) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    with_relay_fixture(context, |fixture| {
        let relay_url = fixture
            .relay_url()
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("fixture manifest missing relay_url"))?;

        let alice_dir = context.state_dir().join("alice");
        let bob_dir = context.state_dir().join("bob");
        write_config_with_relay(&alice_dir, &relay_url)?;
        write_config_with_relay(&bob_dir, &relay_url)?;

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

        let chat_id = create_or_open_dm_chat(&alice, &bob_npub, Duration::from_secs(60))?;
        wait_until("bob sees dm shell", Duration::from_secs(20), || {
            bob.state()
                .chat_list
                .iter()
                .any(|chat| chat.chat_id == chat_id)
        })?;

        let message = "hi-from-alice";
        alice.dispatch(AppAction::SendMessage {
            chat_id: chat_id.clone(),
            content: message.into(),
            kind: None,
            reply_to_message_id: None,
        });

        wait_until("alice first message sent", Duration::from_secs(10), || {
            alice
                .state()
                .current_chat
                .as_ref()
                .and_then(|chat| chat.messages.iter().find(|msg| msg.content == message))
                .map(|msg| matches!(msg.delivery, pika_core::MessageDeliveryState::Sent))
                .unwrap_or(false)
        })?;

        wait_until(
            "bob preview and unread updated",
            Duration::from_secs(20),
            || {
                bob.state()
                    .chat_list
                    .iter()
                    .find(|chat| chat.chat_id == chat_id)
                    .map(|chat| {
                        chat.unread_count > 0 && chat.last_message.as_deref() == Some(message)
                    })
                    .unwrap_or(false)
            },
        )?;

        bob.dispatch(AppAction::OpenChat {
            chat_id: chat_id.clone(),
        });
        wait_until(
            "bob opened chat has message",
            Duration::from_secs(20),
            || {
                bob.state()
                    .current_chat
                    .as_ref()
                    .and_then(|chat| chat.messages.iter().find(|msg| msg.content == message))
                    .is_some()
            },
        )?;

        let bob_state = bob.state();
        let received = bob_state
            .current_chat
            .as_ref()
            .and_then(|chat| chat.messages.iter().find(|msg| msg.content == message))
            .ok_or_else(|| anyhow!("bob chat missing first delivered message"))?;
        anyhow::ensure!(
            !received.is_mine,
            "peer-delivered message must not be marked as mine"
        );

        Ok(())
    })
}

// Checked-in deterministic group-profile contract: after a late joiner gets the group shell, an
// explicit member profile refresh makes those names visible in the group they open.
pub fn run_late_joiner_group_profile_visibility_after_explicit_refresh(
    context: &TestContext,
) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    with_relay_fixture(context, |fixture| {
        let relay_url = fixture
            .relay_url()
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("fixture manifest missing relay_url"))?;

        let alice_dir = context.state_dir().join("alice");
        let bob_dir = context.state_dir().join("bob");
        let charlie_dir = context.state_dir().join("charlie");
        write_config_with_relay(&alice_dir, &relay_url)?;
        write_config_with_relay(&bob_dir, &relay_url)?;
        write_config_with_relay(&charlie_dir, &relay_url)?;

        let alice = FfiApp::new(path_arg(&alice_dir), String::new(), String::new());
        let bob = FfiApp::new(path_arg(&bob_dir), String::new(), String::new());
        let charlie = FfiApp::new(path_arg(&charlie_dir), String::new(), String::new());

        alice.dispatch(AppAction::CreateAccount);
        bob.dispatch(AppAction::CreateAccount);
        charlie.dispatch(AppAction::CreateAccount);
        wait_until("alice logged in", Duration::from_secs(10), || {
            matches!(alice.state().auth, AuthState::LoggedIn { .. })
        })?;
        wait_until("bob logged in", Duration::from_secs(10), || {
            matches!(bob.state().auth, AuthState::LoggedIn { .. })
        })?;
        wait_until("charlie logged in", Duration::from_secs(10), || {
            matches!(charlie.state().auth, AuthState::LoggedIn { .. })
        })?;

        let bob_npub = match bob.state().auth {
            AuthState::LoggedIn { npub, .. } => npub,
            _ => bail!("bob failed to enter logged-in state"),
        };
        let charlie_npub = match charlie.state().auth {
            AuthState::LoggedIn { npub, .. } => npub,
            _ => bail!("charlie failed to enter logged-in state"),
        };

        let chat_id = create_group_chat(
            &alice,
            &bob_npub,
            "LateJoinerProfileBoundary",
            Duration::from_secs(60),
        )?;
        wait_until("bob sees group shell", Duration::from_secs(30), || {
            bob.state()
                .chat_list
                .iter()
                .any(|chat| chat.chat_id == chat_id)
        })?;

        alice.dispatch(AppAction::SaveGroupProfile {
            chat_id: chat_id.clone(),
            name: "Admin Alice".to_owned(),
            about: String::new(),
        });
        bob.dispatch(AppAction::SaveGroupProfile {
            chat_id: chat_id.clone(),
            name: "Builder Bob".to_owned(),
            about: String::new(),
        });

        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(60) {
            if charlie
                .state()
                .chat_list
                .iter()
                .any(|chat| chat.chat_id == chat_id)
            {
                break;
            }
            alice.dispatch(AppAction::AddGroupMembers {
                chat_id: chat_id.clone(),
                peer_npubs: vec![charlie_npub.clone()],
            });
            std::thread::sleep(Duration::from_secs(2));
        }

        wait_until("charlie sees group shell", Duration::from_secs(30), || {
            charlie
                .state()
                .chat_list
                .iter()
                .any(|chat| chat.chat_id == chat_id)
        })?;

        std::thread::sleep(Duration::from_secs(2));

        // This selector owns only the readable "late joiner sees member names after explicit
        // post-join refresh" contract. It is not a proof that already-established pre-join names
        // rebroadcast on their own in this deterministic harness.
        alice.dispatch(AppAction::SaveGroupProfile {
            chat_id: chat_id.clone(),
            name: "Admin Alice".to_owned(),
            about: String::new(),
        });
        bob.dispatch(AppAction::SaveGroupProfile {
            chat_id: chat_id.clone(),
            name: "Builder Bob".to_owned(),
            about: String::new(),
        });

        charlie.dispatch(AppAction::OpenChat {
            chat_id: chat_id.clone(),
        });
        wait_until(
            "charlie sees explicitly refreshed group member names",
            Duration::from_secs(30),
            || {
                charlie
                    .state()
                    .current_chat
                    .as_ref()
                    .map(|chat| {
                        chat.members
                            .iter()
                            .any(|member| member.name.as_deref() == Some("Admin Alice"))
                            && chat
                                .members
                                .iter()
                                .any(|member| member.name.as_deref() == Some("Builder Bob"))
                    })
                    .unwrap_or(false)
            },
        )?;

        Ok(())
    })
}

// CI-facing readable DM-profile contract: Alice sets a per-chat profile override, Bob sees that
// name inside the DM, and the override does not leak into a separate group chat with the same peer.
pub fn run_dm_local_profile_override_visibility(context: &TestContext) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    with_relay_fixture(context, |fixture| {
        let relay_url = fixture
            .relay_url()
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("fixture manifest missing relay_url"))?;

        let alice_dir = context.state_dir().join("alice");
        let bob_dir = context.state_dir().join("bob");
        write_config_with_relay(&alice_dir, &relay_url)?;
        write_config_with_relay(&bob_dir, &relay_url)?;

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

        let dm_chat_id = create_or_open_dm_chat(&alice, &bob_npub, Duration::from_secs(60))?;
        wait_until("bob sees dm shell", Duration::from_secs(30), || {
            bob.state()
                .chat_list
                .iter()
                .any(|chat| chat.chat_id == dm_chat_id)
        })?;

        let group_chat_id = create_group_chat(
            &alice,
            &bob_npub,
            "DmProfileScopeGroup",
            Duration::from_secs(60),
        )?;
        wait_until("bob sees group shell", Duration::from_secs(30), || {
            bob.state()
                .chat_list
                .iter()
                .any(|chat| chat.chat_id == group_chat_id)
        })?;

        alice.dispatch(AppAction::SaveGroupProfile {
            chat_id: dm_chat_id.clone(),
            name: "DM Alice".to_owned(),
            about: "dm only".to_owned(),
        });

        alice.dispatch(AppAction::OpenChat {
            chat_id: dm_chat_id.clone(),
        });
        wait_until(
            "alice sees own dm-local profile",
            Duration::from_secs(10),
            || {
                alice
                    .state()
                    .current_chat
                    .as_ref()
                    .and_then(|chat| chat.my_group_profile.as_ref())
                    .map(|profile| profile.name == "DM Alice" && profile.about == "dm only")
                    .unwrap_or(false)
            },
        )?;

        bob.dispatch(AppAction::OpenChat {
            chat_id: dm_chat_id.clone(),
        });
        wait_until(
            "bob sees alice dm-local name",
            Duration::from_secs(30),
            || {
                bob.state()
                    .current_chat
                    .as_ref()
                    .map(|chat| {
                        chat.members
                            .iter()
                            .any(|member| member.name.as_deref() == Some("DM Alice"))
                    })
                    .unwrap_or(false)
            },
        )?;

        bob.dispatch(AppAction::OpenChat {
            chat_id: group_chat_id.clone(),
        });
        wait_until("bob opened group chat", Duration::from_secs(20), || {
            bob.state()
                .current_chat
                .as_ref()
                .map(|chat| chat.chat_id == group_chat_id)
                .unwrap_or(false)
        })?;
        anyhow::ensure!(
            bob.state()
                .current_chat
                .as_ref()
                .map(|chat| {
                    !chat
                        .members
                        .iter()
                        .any(|member| member.name.as_deref() == Some("DM Alice"))
                })
                .unwrap_or(false),
            "dm-local profile override leaked into a separate group chat"
        );

        Ok(())
    })
}

// CI-facing readable agent-launch contract: the app sees the launch button, kicks off
// provisioning through the same FfiApp actions the native shells use, shows meaningful
// provisioning phases, and lands in the direct chat once the mocked backend reports ready.
pub fn run_agent_launch_provisioning_success(context: &TestContext) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    with_backend_fixture(context, |fixture| {
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
        let server_url = fixture
            .server_url()
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("fixture manifest missing server_url"))?;
        let database_url = fixture
            .manifest()
            .database_url
            .as_deref()
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("fixture manifest missing database_url"))?;

        let alice_dir = context.state_dir().join("alice");
        let bob_dir = context.state_dir().join("bob");
        write_config_with_agent_backend(&alice_dir, &relay_url, &moq_url, &server_url)?;
        write_config_with_relay(&bob_dir, &relay_url)?;

        let alice_keys = Keys::generate();
        let alice_npub = alice_keys
            .public_key()
            .to_bech32()
            .context("encode alice npub")?;
        let alice_nsec = alice_keys
            .secret_key()
            .to_bech32()
            .context("encode alice nsec")?;
        insert_agent_allowlist_row(&database_url, &alice_npub)?;

        let alice = FfiApp::new(path_arg(&alice_dir), String::new(), String::new());
        let bob = FfiApp::new(path_arg(&bob_dir), String::new(), String::new());

        alice.dispatch(AppAction::RestoreSession { nsec: alice_nsec });
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

        wait_until(
            "alice sees agent launch button",
            Duration::from_secs(10),
            || {
                alice
                    .state()
                    .agent_button
                    .as_ref()
                    .map(|button| button.title == "New Agent" && !button.is_busy)
                    .unwrap_or(false)
            },
        )?;

        alice.dispatch(AppAction::EnsureAgent);
        wait_until(
            "agent provisioning started",
            Duration::from_secs(10),
            || alice.state().agent_provisioning.is_some(),
        )?;
        wait_until("agent button marked busy", Duration::from_secs(10), || {
            alice
                .state()
                .agent_button
                .as_ref()
                .map(|button| button.is_busy)
                .unwrap_or(false)
        })?;

        rewrite_active_agent_identity(
            &database_url,
            &alice_npub,
            &bob_npub,
            Duration::from_secs(10),
        )?;

        let phase_history =
            wait_for_agent_chat_outcome(&alice, &bob_npub, Duration::from_secs(45))?;
        assert_phase_sequence(
            &phase_history,
            &[
                AgentProvisioningPhase::BootingGuest,
                AgentProvisioningPhase::WaitingForServiceReady,
            ],
        )?;

        let state = alice.state();
        let chat = state
            .current_chat
            .as_ref()
            .ok_or_else(|| anyhow!("agent launch did not open a chat"))?;
        anyhow::ensure!(
            chat.group_name.is_none(),
            "agent launch should land in a direct chat"
        );
        anyhow::ensure!(
            chat.members.iter().any(|member| member.npub == bob_npub),
            "agent chat should target the provisioned peer identity"
        );
        anyhow::ensure!(
            state
                .router
                .screen_stack
                .iter()
                .any(|screen| matches!(screen, Screen::Chat { .. })),
            "agent launch should leave the app on the chat screen"
        );
        anyhow::ensure!(
            state.agent_provisioning.is_none(),
            "provisioning state should clear after chat opens"
        );
        anyhow::ensure!(
            !state.busy.starting_agent,
            "launch busy state should clear after success"
        );

        Ok(())
    })
}

// CI-facing readable agent-launch failure contract: after the launch button is tapped, a backend
// provisioning failure stays visible in agent_provisioning state instead of collapsing into a
// toast or disappearing.
pub fn run_agent_launch_provisioning_failure(context: &TestContext) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    with_backend_fixture(context, |fixture| {
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
        let server_url = fixture
            .server_url()
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("fixture manifest missing server_url"))?;
        let database_url = fixture
            .manifest()
            .database_url
            .as_deref()
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("fixture manifest missing database_url"))?;

        let alice_dir = context.state_dir().join("alice");
        write_config_with_agent_backend(&alice_dir, &relay_url, &moq_url, &server_url)?;

        let alice_keys = Keys::generate();
        let alice_npub = alice_keys
            .public_key()
            .to_bech32()
            .context("encode alice npub")?;
        let alice_nsec = alice_keys
            .secret_key()
            .to_bech32()
            .context("encode alice nsec")?;
        insert_agent_allowlist_row(&database_url, &alice_npub)?;

        let alice = FfiApp::new(path_arg(&alice_dir), String::new(), String::new());
        alice.dispatch(AppAction::RestoreSession { nsec: alice_nsec });
        wait_until("alice logged in", Duration::from_secs(10), || {
            matches!(alice.state().auth, AuthState::LoggedIn { .. })
        })?;
        wait_until(
            "alice sees agent launch button",
            Duration::from_secs(10),
            || {
                alice
                    .state()
                    .agent_button
                    .as_ref()
                    .map(|button| button.title == "New Agent" && !button.is_busy)
                    .unwrap_or(false)
            },
        )?;

        alice.dispatch(AppAction::EnsureAgent);
        wait_until(
            "agent provisioning entered error state",
            Duration::from_secs(10),
            || {
                alice
                    .state()
                    .agent_provisioning
                    .as_ref()
                    .map(|prov| prov.phase == AgentProvisioningPhase::Error)
                    .unwrap_or(false)
            },
        )?;

        let state = alice.state();
        let provisioning = state
            .agent_provisioning
            .as_ref()
            .ok_or_else(|| anyhow!("agent provisioning should stay visible on failure"))?;
        anyhow::ensure!(
            provisioning.status_message.contains("Agent request failed"),
            "expected backend failure message to stay on provisioning screen, got: {}",
            provisioning.status_message
        );
        anyhow::ensure!(
            state
                .router
                .screen_stack
                .iter()
                .any(|screen| matches!(screen, Screen::AgentProvisioning)),
            "provisioning screen should remain visible on launch failure"
        );
        anyhow::ensure!(
            state.current_chat.is_none(),
            "failed agent launch must not open a chat"
        );
        anyhow::ensure!(
            state.toast.is_none(),
            "failed agent launch should use provisioning state, not a toast"
        );
        anyhow::ensure!(
            !state.busy.starting_agent,
            "launch busy state should clear after failure"
        );

        Ok(())
    })
}

// CI-facing readable logout contract: after a user logs out, Rust-owned app state clears, and a
// fresh process from the same data dir still starts clean until some outer layer explicitly
// restores a session.
pub fn run_logout_reset_across_restart(context: &TestContext) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let data_dir = context.state_dir().join("app");
    write_config_offline(&data_dir)?;

    let app = FfiApp::new(path_arg(&data_dir), String::new(), String::new());
    app.dispatch(AppAction::CreateAccount);
    wait_until("logged in", Duration::from_secs(10), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    })?;

    let my_npub = match app.state().auth {
        AuthState::LoggedIn { npub, .. } => npub,
        _ => bail!("account failed to enter logged-in state"),
    };

    app.dispatch(AppAction::CreateChat {
        peer_npub: my_npub.clone(),
    });
    wait_until("note-to-self chat created", Duration::from_secs(10), || {
        !app.state().chat_list.is_empty()
    })?;

    let chat_id = app.state().chat_list[0].chat_id.clone();
    app.dispatch(AppAction::OpenChat {
        chat_id: chat_id.clone(),
    });
    wait_until("chat opened", Duration::from_secs(10), || {
        app.state()
            .current_chat
            .as_ref()
            .map(|chat| chat.chat_id == chat_id)
            .unwrap_or(false)
    })?;

    let message = "reset-me";
    app.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: message.to_owned(),
        kind: None,
        reply_to_message_id: None,
    });
    wait_until(
        "message appears before logout",
        Duration::from_secs(10),
        || {
            app.state()
                .current_chat
                .as_ref()
                .map(|chat| chat.messages.iter().any(|msg| msg.content == message))
                .unwrap_or(false)
        },
    )?;
    wait_until(
        "chat preview updates before logout",
        Duration::from_secs(10),
        || {
            app.state()
                .chat_list
                .iter()
                .find(|chat| chat.chat_id == chat_id)
                .and_then(|chat| chat.last_message.as_deref())
                == Some(message)
        },
    )?;

    app.dispatch(AppAction::Logout);
    wait_until(
        "logout clears runtime state",
        Duration::from_secs(10),
        || {
            let state = app.state();
            matches!(state.auth, AuthState::LoggedOut)
                && state.router.default_screen == pika_core::Screen::Login
                && state.chat_list.is_empty()
                && state.current_chat.is_none()
        },
    )?;

    drop(app);

    let restarted = FfiApp::new(path_arg(&data_dir), String::new(), String::new());
    wait_until(
        "fresh process starts logged out",
        Duration::from_secs(5),
        || {
            let state = restarted.state();
            matches!(state.auth, AuthState::LoggedOut)
                && state.router.default_screen == pika_core::Screen::Login
                && state.chat_list.is_empty()
                && state.current_chat.is_none()
        },
    )?;

    Ok(())
}

const NOSTR_CONNECT_CANONICAL_BUNKER_URI: &str = "bunker://79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798?relay=wss://relay.example.com";

fn build_external_signer_app(
    data_dir: &Path,
    handshake_result: ExternalSignerHandshakeResult,
) -> Result<(Arc<FfiApp>, MockExternalSignerBridge)> {
    write_config_with_external_signer(data_dir)?;
    let app = FfiApp::new(path_arg(data_dir), String::new(), String::new());
    let bridge = MockExternalSignerBridge::new(handshake_result);
    app.set_external_signer_bridge(Box::new(bridge.clone()));
    Ok((app, bridge))
}

fn new_unavailable_external_signer_bridge() -> MockExternalSignerBridge {
    MockExternalSignerBridge::new(ExternalSignerHandshakeResult {
        ok: false,
        pubkey: None,
        signer_package: None,
        current_user: None,
        error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
        error_message: Some("unused".into()),
    })
}

fn build_pending_nostr_connect_app(
    data_dir: &Path,
) -> Result<(Arc<FfiApp>, MockExternalSignerBridge)> {
    build_external_signer_app(
        data_dir,
        ExternalSignerHandshakeResult {
            ok: false,
            pubkey: None,
            signer_package: None,
            current_user: None,
            error_kind: Some(ExternalSignerErrorKind::SignerUnavailable),
            error_message: Some("unused".into()),
        },
    )
}

fn dispatch_nostr_connect_callback_and_response(app: &FfiApp, remote_signer_pubkey: &str) {
    app.dispatch(AppAction::NostrConnectCallback {
        url: "pika://nostrconnect-return".into(),
    });
    app.inject_nostr_connect_connect_response_for_tests(remote_signer_pubkey.to_string());
}

// Checked-in deterministic Nostr Connect contract: Rust launches the raw signer handshake URL, stays
// pending until the callback arrives, then finishes bunker bootstrap and lands signed in. Native
// intent glue still owns adding the callback URL onto the launched nostrconnect URI.
pub fn run_nostr_connect_login_success(context: &TestContext) -> Result<()> {
    let data_dir = context.state_dir().join("nostr-connect");
    let (app, bridge) = build_pending_nostr_connect_app(&data_dir)?;

    let (connector, _user_pubkey) =
        MockBunkerSignerConnector::success(NOSTR_CONNECT_CANONICAL_BUNKER_URI);
    app.set_bunker_signer_connector_for_tests(Arc::new(connector.clone()));

    app.dispatch(AppAction::BeginNostrConnectLogin);
    wait_until("nostrconnect uri opened", Duration::from_secs(10), || {
        bridge.last_opened_url().is_some()
    })?;

    let opened_url = bridge
        .last_opened_url()
        .ok_or_else(|| anyhow!("expected opened nostrconnect URL"))?;
    anyhow::ensure!(
        opened_url.starts_with("nostrconnect://"),
        "expected nostrconnect URL, got {opened_url}"
    );
    anyhow::ensure!(
        opened_url.contains("secret=")
            && opened_url.contains("metadata=")
            && opened_url.contains("perms=")
            && opened_url.contains("relay="),
        "nostrconnect URL missing required handshake parameters: {opened_url}"
    );
    anyhow::ensure!(
        query_param(&opened_url, "name").as_deref() == Some("Pika"),
        "nostrconnect URL should advertise Pika name"
    );
    anyhow::ensure!(
        query_param(&opened_url, "url").as_deref() == Some("https://pikachat.org"),
        "nostrconnect URL should advertise Pika URL"
    );
    let metadata = nostrconnect_metadata(&opened_url).ok_or_else(|| anyhow!("metadata JSON"))?;
    anyhow::ensure!(
        metadata.get("name").and_then(|v| v.as_str()) == Some("Pika"),
        "nostrconnect metadata should preserve app name"
    );
    anyhow::ensure!(
        metadata.get("url").and_then(|v| v.as_str()) == Some("https://pikachat.org"),
        "nostrconnect metadata should preserve app URL"
    );

    anyhow::ensure!(
        matches!(app.state().auth, AuthState::LoggedOut),
        "app should remain logged out until callback"
    );
    anyhow::ensure!(
        app.state().busy.logging_in,
        "app should stay pending while waiting for callback"
    );
    anyhow::ensure!(
        connector.last_bunker_uri().is_none(),
        "bunker connect must not start before callback"
    );
    anyhow::ensure!(
        bridge.last_hint().is_none(),
        "nostr connect login should not route through external signer user hints"
    );

    dispatch_nostr_connect_callback_and_response(
        &app,
        "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
    );

    wait_until("nostrconnect logged in", Duration::from_secs(10), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
            && app.state().router.default_screen == pika_core::Screen::ChatList
            && !app.state().busy.logging_in
    })?;

    let state = app.state();
    match state.auth {
        AuthState::LoggedIn {
            mode: AuthMode::BunkerSigner { bunker_uri },
            ..
        } => anyhow::ensure!(
            bunker_uri == NOSTR_CONNECT_CANONICAL_BUNKER_URI,
            "expected canonical bunker signer URI, got {bunker_uri}"
        ),
        other => bail!("expected bunker signer auth mode, got {other:?}"),
    }

    let connect_uri = connector
        .last_bunker_uri()
        .ok_or_else(|| anyhow!("expected bunker connect URI for signer bootstrap"))?;
    anyhow::ensure!(
        connect_uri.starts_with(
            "bunker://79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        ),
        "unexpected bunker connect URI: {connect_uri}"
    );
    anyhow::ensure!(
        connect_uri.contains("relay=") && connect_uri.contains("secret="),
        "bunker connect URI should carry relay and secret: {connect_uri}"
    );

    Ok(())
}

// Checked-in deterministic external-signer contract: a direct signer login should land the app
// signed in without bouncing through bunker/Nostr Connect handshake state.
pub fn run_external_signer_login_success(context: &TestContext) -> Result<()> {
    let data_dir = context.state_dir().join("external-signer-login-success");
    let (app, _bridge) = build_external_signer_app(
        &data_dir,
        ExternalSignerHandshakeResult {
            ok: true,
            pubkey: Some("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798".into()),
            signer_package: Some("com.greenart7c3.nostrsigner".into()),
            current_user: Some("amber-user-1".into()),
            error_kind: None,
            error_message: None,
        },
    )?;

    app.dispatch(AppAction::BeginExternalSignerLogin {
        current_user_hint: Some("hint-user".into()),
    });

    wait_until("external signer logged in", Duration::from_secs(10), || {
        let state = app.state();
        matches!(state.auth, AuthState::LoggedIn { .. })
            && state.router.default_screen == pika_core::Screen::ChatList
            && !state.busy.logging_in
    })?;

    match app.state().auth {
        AuthState::LoggedIn {
            mode:
                AuthMode::ExternalSigner {
                    pubkey,
                    signer_package,
                    current_user,
                },
            ..
        } => {
            anyhow::ensure!(
                pubkey == "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
                "unexpected external signer pubkey: {pubkey}"
            );
            anyhow::ensure!(
                signer_package == "com.greenart7c3.nostrsigner",
                "unexpected external signer package: {signer_package}"
            );
            anyhow::ensure!(
                current_user == "amber-user-1",
                "unexpected external signer current_user: {current_user}"
            );
        }
        other => bail!("expected external signer auth mode, got {other:?}"),
    }

    Ok(())
}

// Checked-in deterministic external-signer failure contract: a direct signer timeout should
// leave the app logged out, clear busy state, and surface a user-visible timeout toast.
pub fn run_external_signer_login_timeout_failure(context: &TestContext) -> Result<()> {
    let data_dir = context.state_dir().join("external-signer-login-timeout");
    let (app, _bridge) = build_external_signer_app(
        &data_dir,
        ExternalSignerHandshakeResult {
            ok: false,
            pubkey: None,
            signer_package: None,
            current_user: None,
            error_kind: Some(ExternalSignerErrorKind::Timeout),
            error_message: Some("timeout".into()),
        },
    )?;

    app.dispatch(AppAction::BeginExternalSignerLogin {
        current_user_hint: None,
    });

    wait_until(
        "external signer timeout surfaced",
        Duration::from_secs(10),
        || {
            let state = app.state();
            matches!(state.auth, AuthState::LoggedOut)
                && !state.busy.logging_in
                && state.toast.is_some()
        },
    )?;

    anyhow::ensure!(
        app.state()
            .toast
            .clone()
            .unwrap_or_default()
            .to_lowercase()
            .contains("timed out"),
        "expected timeout toast, got {:?}",
        app.state().toast
    );
    Ok(())
}

// Checked-in deterministic bunker-login contract: entering a bunker URI should sign the app in
// and land it on the normal signed-in chat shell. Descriptor/client-key plumbing stays below this layer.
pub fn run_bunker_login_success(context: &TestContext) -> Result<()> {
    let data_dir = context.state_dir().join("bunker-login-success");
    write_config_with_external_signer(&data_dir)?;

    let app = FfiApp::new(path_arg(&data_dir), String::new(), String::new());
    let (connector, expected_user_pubkey) =
        MockBunkerSignerConnector::success(NOSTR_CONNECT_CANONICAL_BUNKER_URI);
    app.set_bunker_signer_connector_for_tests(Arc::new(connector));

    app.dispatch(AppAction::BeginBunkerLogin {
        bunker_uri:
            "bunker://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa?relay=wss://relay.input"
                .into(),
    });

    wait_until("bunker login signed in", Duration::from_secs(10), || {
        let state = app.state();
        matches!(state.auth, AuthState::LoggedIn { .. })
            && state.router.default_screen == pika_core::Screen::ChatList
            && !state.busy.logging_in
    })?;

    match app.state().auth {
        AuthState::LoggedIn {
            pubkey,
            mode: AuthMode::BunkerSigner { bunker_uri },
            ..
        } => {
            anyhow::ensure!(
                pubkey == expected_user_pubkey,
                "unexpected bunker signer pubkey: {pubkey}"
            );
            anyhow::ensure!(
                bunker_uri == NOSTR_CONNECT_CANONICAL_BUNKER_URI,
                "unexpected canonical bunker URI: {bunker_uri}"
            );
        }
        other => bail!("expected bunker signer auth mode, got {other:?}"),
    }

    Ok(())
}

// Checked-in deterministic bunker-login failure contract: an invalid bunker URI should surface a
// user-visible error and leave the app cleanly logged out.
pub fn run_bunker_login_invalid_uri_failure(context: &TestContext) -> Result<()> {
    let data_dir = context.state_dir().join("bunker-login-invalid-uri");
    write_config_with_external_signer(&data_dir)?;

    let app = FfiApp::new(path_arg(&data_dir), String::new(), String::new());
    let connector = MockBunkerSignerConnector::failure(
        BunkerConnectErrorKind::InvalidUri,
        "invalid bunker URI",
    );
    app.set_bunker_signer_connector_for_tests(Arc::new(connector));

    app.dispatch(AppAction::BeginBunkerLogin {
        bunker_uri: "not-a-uri".into(),
    });

    wait_until(
        "bunker invalid uri surfaced",
        Duration::from_secs(10),
        || {
            let state = app.state();
            matches!(state.auth, AuthState::LoggedOut)
                && !state.busy.logging_in
                && state.toast.is_some()
        },
    )?;

    anyhow::ensure!(
        app.state()
            .toast
            .clone()
            .unwrap_or_default()
            .to_lowercase()
            .contains("invalid bunker uri"),
        "expected invalid bunker URI toast, got {:?}",
        app.state().toast
    );
    Ok(())
}

// Checked-in deterministic Nostr Connect fallback contract: a signer that rejects "new secret"
// pairing must still let the login succeed without asking the user to reset pairing manually.
pub fn run_nostr_connect_new_secret_retry(context: &TestContext) -> Result<()> {
    let data_dir = context.state_dir().join("nostr-connect-new-secret-retry");
    let (app, _bridge) = build_pending_nostr_connect_app(&data_dir)?;

    let signer_keys = Keys::generate();
    let remote_signer_pubkey = signer_keys.public_key().to_hex();
    let output = BunkerConnectOutput {
        user_pubkey: signer_keys.public_key(),
        canonical_bunker_uri: format!(
            "bunker://{remote_signer_pubkey}?relay=wss://relay.example.com"
        ),
        signer: Arc::new(signer_keys) as Arc<dyn NostrSigner>,
    };
    let connector = SequenceBunkerSignerConnector::new(vec![
        Err(BunkerConnectError {
            kind: BunkerConnectErrorKind::Rejected,
            message: "We don't accept connect requests with new secret.".into(),
        }),
        Ok(output),
    ]);
    app.set_bunker_signer_connector_for_tests(Arc::new(connector.clone()));

    app.dispatch(AppAction::BeginNostrConnectLogin);
    wait_until("nostrconnect pending", Duration::from_secs(10), || {
        app.state().busy.logging_in
    })?;
    dispatch_nostr_connect_callback_and_response(&app, &remote_signer_pubkey);

    wait_until(
        "nostrconnect logged in after retry",
        Duration::from_secs(10),
        || matches!(app.state().auth, AuthState::LoggedIn { .. }) && !app.state().busy.logging_in,
    )?;
    Ok(())
}

// Checked-in deterministic Nostr Connect failure contract: a normal signer rejection should
// surface as a failed login once, without silently retrying bunker connect.
pub fn run_nostr_connect_non_secret_rejection_stops_without_retry(
    context: &TestContext,
) -> Result<()> {
    let data_dir = context.state_dir().join("nostr-connect-policy-rejection");
    let (app, _bridge) = build_pending_nostr_connect_app(&data_dir)?;

    let signer_keys = Keys::generate();
    let remote_signer_pubkey = signer_keys.public_key().to_hex();
    let output = BunkerConnectOutput {
        user_pubkey: signer_keys.public_key(),
        canonical_bunker_uri: format!(
            "bunker://{remote_signer_pubkey}?relay=wss://relay.example.com"
        ),
        signer: Arc::new(signer_keys) as Arc<dyn NostrSigner>,
    };
    let connector = SequenceBunkerSignerConnector::new(vec![
        Err(BunkerConnectError {
            kind: BunkerConnectErrorKind::Rejected,
            message: "Request rejected by signer policy".into(),
        }),
        Ok(output),
    ]);
    app.set_bunker_signer_connector_for_tests(Arc::new(connector.clone()));

    app.dispatch(AppAction::BeginNostrConnectLogin);
    wait_until("nostrconnect pending", Duration::from_secs(10), || {
        app.state().busy.logging_in
    })?;
    dispatch_nostr_connect_callback_and_response(&app, &remote_signer_pubkey);

    wait_until(
        "nostrconnect rejection surfaced",
        Duration::from_secs(10),
        || {
            matches!(app.state().auth, AuthState::LoggedOut)
                && !app.state().busy.logging_in
                && app.state().toast.is_some()
        },
    )?;

    anyhow::ensure!(
        connector.seen_uris().len() == 1,
        "unexpected bunker retry sequence: {:?}",
        connector.seen_uris()
    );
    anyhow::ensure!(
        app.state()
            .toast
            .clone()
            .unwrap_or_default()
            .to_lowercase()
            .contains("rejected"),
        "expected user-visible rejection toast, got {:?}",
        app.state().toast
    );
    Ok(())
}

// Checked-in deterministic persistence contract: a pending Nostr Connect login survives process
// restart and can still complete bunker bootstrap after the callback arrives later.
pub fn run_pending_nostr_connect_login_survives_restart(context: &TestContext) -> Result<()> {
    let data_dir = context.state_dir().join("nostr-connect-pending-restart");
    let (app, bridge) = build_pending_nostr_connect_app(&data_dir)?;

    app.dispatch(AppAction::BeginNostrConnectLogin);
    wait_until("nostrconnect uri opened", Duration::from_secs(10), || {
        bridge.last_opened_url().is_some()
    })?;
    anyhow::ensure!(
        app.state().busy.logging_in,
        "first process should persist a pending login"
    );

    drop(app);

    let restarted = FfiApp::new(path_arg(&data_dir), String::new(), String::new());
    let restarted_bridge = new_unavailable_external_signer_bridge();
    restarted.set_external_signer_bridge(Box::new(restarted_bridge.clone()));
    let (connector, _user_pubkey) =
        MockBunkerSignerConnector::success(NOSTR_CONNECT_CANONICAL_BUNKER_URI);
    restarted.set_bunker_signer_connector_for_tests(Arc::new(connector.clone()));

    wait_until(
        "restart restores pending state before callback",
        Duration::from_secs(10),
        || {
            let state = restarted.state();
            state.busy.logging_in
                && matches!(state.auth, AuthState::LoggedOut)
                && connector.last_bunker_uri().is_none()
                && restarted_bridge.last_opened_url().is_none()
        },
    )?;

    dispatch_nostr_connect_callback_and_response(
        &restarted,
        "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
    );

    wait_until(
        "nostrconnect logged in after restart",
        Duration::from_secs(10),
        || {
            matches!(restarted.state().auth, AuthState::LoggedIn { .. })
                && !restarted.state().busy.logging_in
        },
    )?;
    anyhow::ensure!(
        connector.last_bunker_uri().is_some(),
        "restart completion should eventually reach bunker connect"
    );
    Ok(())
}

// Checked-in deterministic restore contract: a stored bunker session descriptor signs the app
// back in after relaunch. The exact client-key plumbing stays as a narrower Rust semantic owner.
pub fn run_restore_session_bunker_signs_in(context: &TestContext) -> Result<()> {
    let data_dir = context.state_dir().join("restore-session-bunker");
    write_config_with_external_signer(&data_dir)?;

    let app = FfiApp::new(path_arg(&data_dir), String::new(), String::new());
    let canonical_bunker_uri = "bunker://79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798?relay=wss://relay.restore";
    let (connector, _user_pubkey) = MockBunkerSignerConnector::success(canonical_bunker_uri);
    app.set_bunker_signer_connector_for_tests(Arc::new(connector));

    let client_keys = Keys::generate();
    let client_nsec = client_keys
        .secret_key()
        .to_bech32()
        .context("encode bunker client key")?;

    app.dispatch(AppAction::RestoreSessionBunker {
        bunker_uri:
            "bunker://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb?relay=wss://relay.restore.input"
                .into(),
        client_nsec,
    });

    wait_until("bunker restore logged in", Duration::from_secs(10), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
            && app.state().router.default_screen == pika_core::Screen::ChatList
            && !app.state().busy.logging_in
    })?;

    match app.state().auth {
        AuthState::LoggedIn {
            mode: AuthMode::BunkerSigner { bunker_uri },
            ..
        } => anyhow::ensure!(
            bunker_uri == canonical_bunker_uri,
            "expected canonical restored bunker URI, got {bunker_uri}"
        ),
        other => bail!("expected restored bunker signer auth mode, got {other:?}"),
    }

    Ok(())
}

pub fn run_call_with_pikachat_daemon(context: &TestContext) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    with_relay_and_moq_fixture(context, |fixture| {
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

        let audio_fixture_path = context.state_dir().join("audio-fixtures/alternating.wav");
        write_alternating_audio_fixture(&audio_fixture_path)?;
        let audio_fixture_env = path_arg(&audio_fixture_path);
        let _audio_fixture_env = ScopedEnvVar::set("PIKA_AUDIO_FIXTURE", &audio_fixture_env);

        eprintln!("[test] audio fixture: {}", audio_fixture_path.display());
        eprintln!("[test] using relay: {relay_url}");

        let daemon_state_dir = context.state_dir().join("daemon");
        fs::create_dir_all(&daemon_state_dir)
            .with_context(|| format!("create daemon state dir {}", daemon_state_dir.display()))?;
        let mut daemon = DaemonHandle::spawn(context, &relay_url, &daemon_state_dir)?;

        daemon.wait_for_event("daemon ready", Duration::from_secs(15), |value| {
            value.get("type").and_then(|kind| kind.as_str()) == Some("ready")
        })?;
        let daemon_npub = daemon.npub()?;
        let daemon_pubkey = daemon.pubkey()?;
        eprintln!("[test] daemon pubkey={daemon_pubkey} npub={daemon_npub}");

        daemon.send_cmd(serde_json::json!({
            "cmd": "set_relays",
            "request_id": "sr1",
            "relays": [relay_url.clone()]
        }))?;
        daemon.wait_for_event("set_relays ok", Duration::from_secs(15), |value| {
            value.get("type").and_then(|kind| kind.as_str()) == Some("ok")
                && value.get("request_id").and_then(|id| id.as_str()) == Some("sr1")
        })?;

        daemon.send_cmd(serde_json::json!({
            "cmd": "publish_keypackage",
            "request_id": "kp1",
            "relays": [relay_url.clone()]
        }))?;
        daemon.wait_for_event("kp published", Duration::from_secs(15), |value| {
            value.get("type").and_then(|kind| kind.as_str()) == Some("ok")
                && value.get("request_id").and_then(|id| id.as_str()) == Some("kp1")
        })?;

        let caller_dir = context.state_dir().join("caller");
        write_config_with_moq(&caller_dir, &relay_url, Some(&relay_url), &moq_url)?;
        let caller = FfiApp::new(path_arg(&caller_dir), String::new(), String::new());

        caller.dispatch(AppAction::CreateAccount);
        wait_until("caller logged in", Duration::from_secs(10), || {
            matches!(caller.state().auth, AuthState::LoggedIn { .. })
        })?;

        let chat_id = create_or_open_dm_chat(&caller, &daemon_npub, Duration::from_secs(90))?;
        eprintln!("[test] chat created: {chat_id}");

        let welcome = daemon.wait_for_event(
            "daemon welcome_received",
            Duration::from_secs(30),
            |value| value.get("type").and_then(|kind| kind.as_str()) == Some("welcome_received"),
        )?;
        let wrapper_id = welcome
            .get("wrapper_event_id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("welcome_received missing wrapper_event_id"))?
            .to_string();

        daemon.send_cmd(serde_json::json!({
            "cmd": "accept_welcome",
            "request_id": "acc1",
            "wrapper_event_id": wrapper_id
        }))?;
        daemon.wait_for_event("daemon group_joined", Duration::from_secs(30), |value| {
            value.get("type").and_then(|kind| kind.as_str()) == Some("group_joined")
        })?;

        let nonce = format!("{:016x}", rand::random::<u64>());
        let ping_msg = format!("ping:{nonce}");
        let pong_msg = format!("pong:{nonce}");

        caller.dispatch(AppAction::SendMessage {
            chat_id: chat_id.clone(),
            content: ping_msg.clone(),
            kind: None,
            reply_to_message_id: None,
        });

        let message = daemon.wait_for_event(
            "daemon message_received (ping)",
            Duration::from_secs(30),
            |value| {
                value.get("type").and_then(|kind| kind.as_str()) == Some("message_received")
                    && value
                        .get("content")
                        .and_then(|content| content.as_str())
                        .map(|content| content == ping_msg)
                        .unwrap_or(false)
            },
        )?;

        let nostr_group_id = message
            .get("nostr_group_id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("message_received missing nostr_group_id"))?
            .to_string();

        daemon.send_cmd(serde_json::json!({
            "cmd": "send_message",
            "request_id": "pong1",
            "nostr_group_id": nostr_group_id,
            "content": pong_msg.clone()
        }))?;
        daemon.wait_for_event("pong send ok", Duration::from_secs(15), |value| {
            value.get("type").and_then(|kind| kind.as_str()) == Some("ok")
                && value.get("request_id").and_then(|id| id.as_str()) == Some("pong1")
        })?;

        wait_until("caller received pong", Duration::from_secs(30), || {
            caller
                .state()
                .current_chat
                .as_ref()
                .and_then(|chat| {
                    chat.messages
                        .iter()
                        .find(|message| message.content == pong_msg)
                })
                .is_some()
        })?;
        eprintln!("[test] PASS: ping/pong works");

        caller.dispatch(AppAction::StartCall {
            chat_id: chat_id.clone(),
        });
        wait_until("caller offering", Duration::from_secs(10), || {
            caller
                .state()
                .active_call
                .as_ref()
                .map(|call| matches!(call.status, CallStatus::Offering))
                .unwrap_or(false)
        })?;

        let invite = daemon.wait_for_event(
            "daemon call_invite_received",
            Duration::from_secs(30),
            |value| {
                value.get("type").and_then(|kind| kind.as_str()) == Some("call_invite_received")
            },
        )?;
        let call_id = invite
            .get("call_id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("call_invite_received missing call_id"))?
            .to_string();

        daemon.send_cmd(serde_json::json!({
            "cmd": "accept_call",
            "request_id": "accept1",
            "call_id": call_id.clone()
        }))?;
        daemon.wait_for_event(
            "daemon call_session_started",
            Duration::from_secs(30),
            |value| {
                value.get("type").and_then(|kind| kind.as_str()) == Some("call_session_started")
            },
        )?;

        wait_until(
            "caller active with tx frames",
            Duration::from_secs(30),
            || {
                caller
                    .state()
                    .active_call
                    .as_ref()
                    .map(|call| {
                        matches!(call.status, CallStatus::Active)
                            && call
                                .debug
                                .as_ref()
                                .map(|debug| debug.tx_frames > 5)
                                .unwrap_or(false)
                    })
                    .unwrap_or(false)
            },
        )?;

        let require_rx = std::env::var("PIKACHAT_ECHO_MODE")
            .map(|value| !value.trim().is_empty() && value.trim() != "0")
            .unwrap_or(false);
        let use_real_ai = std::env::var("OPENAI_API_KEY").is_ok();

        if require_rx {
            wait_until(
                "caller receiving echoed frames",
                Duration::from_secs(15),
                || {
                    caller
                        .state()
                        .active_call
                        .as_ref()
                        .and_then(|call| call.debug.as_ref().map(|debug| debug.rx_frames > 0))
                        .unwrap_or(false)
                },
            )?;
        } else if use_real_ai {
            daemon.wait_for_event(
                "daemon accumulating audio",
                Duration::from_secs(30),
                |value| {
                    value.get("type").and_then(|kind| kind.as_str()) == Some("call_debug")
                        && value
                            .get("call_id")
                            .and_then(|id| id.as_str())
                            .map(|id| id == call_id)
                            .unwrap_or(false)
                        && value
                            .get("rx_frames")
                            .and_then(|frames| frames.as_u64())
                            .map(|frames| frames >= 200)
                            .unwrap_or(false)
                },
            )?;
        } else {
            daemon.wait_for_event(
                "daemon stt receiving frames",
                Duration::from_secs(20),
                |value| {
                    value.get("type").and_then(|kind| kind.as_str()) == Some("call_debug")
                        && value
                            .get("call_id")
                            .and_then(|id| id.as_str())
                            .map(|id| id == call_id)
                            .unwrap_or(false)
                        && value
                            .get("rx_frames")
                            .and_then(|frames| frames.as_u64())
                            .map(|frames| frames > 0)
                            .unwrap_or(false)
                },
            )?;
        }

        if !require_rx {
            let audio_chunk = daemon.wait_for_event(
                "daemon call_audio_chunk",
                Duration::from_secs(30),
                |value| {
                    value.get("type").and_then(|kind| kind.as_str()) == Some("call_audio_chunk")
                        && value
                            .get("call_id")
                            .and_then(|id| id.as_str())
                            .map(|id| id == call_id)
                            .unwrap_or(false)
                },
            )?;
            let audio_path = audio_chunk
                .get("audio_path")
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("call_audio_chunk missing audio_path"))?
                .to_string();
            let wav_data = fs::read(&audio_path)
                .with_context(|| format!("read daemon audio chunk {}", audio_path))?;
            anyhow::ensure!(wav_data.len() > 44, "WAV file too short at {}", audio_path);
            anyhow::ensure!(&wav_data[0..4] == b"RIFF", "WAV missing RIFF header");
            anyhow::ensure!(&wav_data[8..12] == b"WAVE", "WAV missing WAVE header");

            let tts_text = "This is a test of the text to speech system.";
            daemon.send_cmd(serde_json::json!({
                "cmd": "send_audio_response",
                "request_id": "tts1",
                "call_id": call_id.clone(),
                "tts_text": tts_text,
            }))?;
            let tts_timeout = if use_real_ai {
                Duration::from_secs(45)
            } else {
                Duration::from_secs(30)
            };
            let tts_result =
                daemon.wait_for_event("send_audio_response result", tts_timeout, |value| {
                    value.get("request_id").and_then(|id| id.as_str()) == Some("tts1")
                })?;
            let tts_ok = tts_result
                .get("type")
                .and_then(|kind| kind.as_str())
                .map(|kind| kind == "ok")
                .unwrap_or(false);
            anyhow::ensure!(tts_ok, "TTS publish failed: {tts_result}");

            wait_until(
                "caller receiving TTS frames",
                Duration::from_secs(30),
                || {
                    caller
                        .state()
                        .active_call
                        .as_ref()
                        .and_then(|call| call.debug.as_ref().map(|debug| debug.rx_frames > 0))
                        .unwrap_or(false)
                },
            )?;
        }

        caller.dispatch(AppAction::EndCall);
        wait_until("caller call ended", Duration::from_secs(10), || {
            caller
                .state()
                .active_call
                .as_ref()
                .map(|call| matches!(call.status, CallStatus::Ended { .. }))
                .unwrap_or(true)
        })?;

        if let Some(debug) = caller
            .state()
            .active_call
            .as_ref()
            .and_then(|call| call.debug.as_ref())
        {
            eprintln!(
                "[test] caller final: tx={} rx={} dropped={}",
                debug.tx_frames, debug.rx_frames, debug.rx_dropped
            );
        }

        eprintln!("[test] PASS: pikachat call test on {relay_url}");
        Ok(())
    })
}

fn with_relay_and_moq_fixture(
    context: &TestContext,
    run: impl FnOnce(&FixtureHandle) -> Result<()>,
) -> Result<()> {
    let fixture_context = fixture_context(context)?;
    let fixture = match start_relay_and_moq(&fixture_context) {
        Ok(fixture) => fixture,
        Err(err) => {
            preserve_fixture_diagnostics(context, fixture_context.state_dir())
                .context("preserve fixture startup diagnostics")?;
            return Err(err);
        }
    };
    let result = run(&fixture);

    if let Err(err) = result {
        preserve_fixture_diagnostics(context, fixture.state_dir())
            .context("preserve fixture diagnostics after selector failure")?;
        return Err(err);
    }

    Ok(())
}

fn with_backend_fixture(
    context: &TestContext,
    run: impl FnOnce(&FixtureHandle) -> Result<()>,
) -> Result<()> {
    let fixture_context = fixture_context(context)?;
    let fixture = match start_backend(&fixture_context) {
        Ok(fixture) => fixture,
        Err(err) => {
            preserve_fixture_diagnostics(context, fixture_context.state_dir())
                .context("preserve fixture startup diagnostics")?;
            return Err(err);
        }
    };
    let result = run(&fixture);

    if let Err(err) = result {
        preserve_fixture_diagnostics(context, fixture.state_dir())
            .context("preserve fixture diagnostics after selector failure")?;
        return Err(err);
    }

    Ok(())
}

fn with_relay_fixture(
    context: &TestContext,
    run: impl FnOnce(&FixtureHandle) -> Result<()>,
) -> Result<()> {
    let fixture_context = fixture_context(context)?;
    let fixture = match start_relay(&fixture_context) {
        Ok(fixture) => fixture,
        Err(err) => {
            preserve_fixture_diagnostics(context, fixture_context.state_dir())
                .context("preserve fixture startup diagnostics")?;
            return Err(err);
        }
    };
    let result = run(&fixture);

    if let Err(err) = result {
        preserve_fixture_diagnostics(context, fixture.state_dir())
            .context("preserve fixture diagnostics after selector failure")?;
        return Err(err);
    }

    Ok(())
}

fn create_group_chat(
    creator: &FfiApp,
    peer_npub: &str,
    group_name: &str,
    timeout: Duration,
) -> Result<String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(chat) = creator
            .state()
            .chat_list
            .iter()
            .find(|chat| chat.group_name.as_deref() == Some(group_name))
        {
            return Ok(chat.chat_id.clone());
        }
        creator.dispatch(AppAction::CreateGroupChat {
            peer_npubs: vec![peer_npub.to_owned()],
            group_name: group_name.to_owned(),
        });
        std::thread::sleep(Duration::from_secs(2));
    }
    bail!("group '{group_name}' was not created within {timeout:?}");
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

fn start_relay(context: &TestContext) -> Result<FixtureHandle> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create tokio runtime for local relay selector")?;
    runtime
        .block_on(start_fixture(
            context,
            &FixtureSpec::builder(ProfileName::Relay).build(),
        ))
        .context("start relay fixture")
}

fn start_backend(context: &TestContext) -> Result<FixtureHandle> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create tokio runtime for local agent selector")?;
    runtime
        .block_on(start_fixture(
            context,
            &FixtureSpec::builder(ProfileName::Backend)
                .server_port(0)
                .moq_port(0)
                .build(),
        ))
        .context("start backend fixture")
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

struct DaemonHandle {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout_lines: Arc<Mutex<Vec<serde_json::Value>>>,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
    stdout_thread: Option<std::thread::JoinHandle<()>>,
}

impl DaemonHandle {
    fn spawn(context: &TestContext, relay_url: &str, state_dir: &Path) -> Result<Self> {
        let binary = pikachat_binary(context)?;
        let use_real_ai = std::env::var("OPENAI_API_KEY").is_ok();
        eprintln!(
            "[daemon] spawning {} daemon --relay {} --state-dir {} real_ai={use_real_ai}",
            binary.display(),
            relay_url,
            state_dir.display()
        );

        let mut command = Command::new(&binary);
        command
            .arg("daemon")
            .arg("--relay")
            .arg(relay_url)
            .arg("--state-dir")
            .arg(path_arg(state_dir));
        if use_real_ai {
            command.env("OPENAI_API_KEY", std::env::var("OPENAI_API_KEY").unwrap());
        } else {
            command.env("PIKACHAT_TTS_FIXTURE", "1");
        }
        let mut child = command
            .env(
                "PIKACHAT_ECHO_MODE",
                std::env::var("PIKACHAT_ECHO_MODE").unwrap_or_default(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn pikachat at {}", binary.display()))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("capture pikachat stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("capture pikachat stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("capture pikachat stderr"))?;

        let stderr_thread = std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                eprintln!("[pikachat stderr] {line}");
            }
        });

        let stdout_lines = Arc::new(Mutex::new(Vec::new()));
        let lines_for_thread = Arc::clone(&stdout_lines);
        let stdout_thread = std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                eprintln!("[pikachat stdout] {line}");
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                    lines_for_thread.lock().unwrap().push(value);
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            stdout_lines,
            stderr_thread: Some(stderr_thread),
            stdout_thread: Some(stdout_thread),
        })
    }

    fn send_cmd(&mut self, value: serde_json::Value) -> Result<()> {
        let encoded = serde_json::to_string(&value).context("encode daemon command")?;
        writeln!(self.stdin, "{encoded}").context("write daemon command")?;
        self.stdin.flush().context("flush daemon command")?;
        Ok(())
    }

    fn wait_for_event(
        &self,
        what: &str,
        timeout: Duration,
        pred: impl Fn(&serde_json::Value) -> bool,
    ) -> Result<serde_json::Value> {
        let start = Instant::now();
        let mut last_idx = 0;
        while start.elapsed() < timeout {
            let lines = self.stdout_lines.lock().unwrap();
            for index in last_idx..lines.len() {
                if pred(&lines[index]) {
                    return Ok(lines[index].clone());
                }
            }
            last_idx = lines.len();
            drop(lines);
            std::thread::sleep(Duration::from_millis(50));
        }

        let lines = self.stdout_lines.lock().unwrap();
        let dump = lines
            .iter()
            .enumerate()
            .map(|(index, line)| format!("  [{index}] {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "{what}: daemon event not received within {timeout:?}. stdout events:\n{}",
            if dump.is_empty() {
                "(none)".to_string()
            } else {
                dump
            }
        );
    }

    fn npub(&self) -> Result<String> {
        self.ready_field("npub")
    }

    fn pubkey(&self) -> Result<String> {
        self.ready_field("pubkey")
    }

    fn ready_field(&self, field: &str) -> Result<String> {
        let lines = self.stdout_lines.lock().unwrap();
        lines
            .iter()
            .find(|line| line.get("type").and_then(|kind| kind.as_str()) == Some("ready"))
            .and_then(|line| line.get(field).and_then(|value| value.as_str()))
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("daemon ready event missing {field}"))
    }
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stdout_thread.take() {
            let _ = thread.join();
        }
    }
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn env_path_var(key: &str) -> Option<PathBuf> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn pikachat_binary(context: &TestContext) -> Result<PathBuf> {
    let binary = env_path_var("PIKAHUT_TEST_PIKACHAT_BIN")
        .or_else(|| env_path_var("PIKACHAT_BIN"))
        .unwrap_or_else(|| context.workspace_root().join("target/debug/pikachat"));
    if !binary.exists() {
        bail!(
            "pikachat binary not found at {}. Set PIKAHUT_TEST_PIKACHAT_BIN/PIKACHAT_BIN or build it with `cargo build -p pikachat`",
            binary.display()
        );
    }
    Ok(binary)
}

fn write_alternating_audio_fixture(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create audio fixture dir {}", parent.display()))?;
    }

    let sample_rate = 48_000u32;
    let duration_secs = 10u32;
    let total_samples = sample_rate * duration_secs;
    let mut pcm = Vec::with_capacity(total_samples as usize);
    let freq = 440.0f32;
    let step = 2.0f32 * std::f32::consts::PI * freq / sample_rate as f32;
    let samples_per_sec = sample_rate as usize;
    for index in 0..total_samples as usize {
        let second = index / samples_per_sec;
        let sample = if second.is_multiple_of(2) {
            (((index as f32) * step).sin() * (i16::MAX as f32 * 0.3)) as i16
        } else {
            0i16
        };
        pcm.push(sample);
    }

    let data_len = (pcm.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in &pcm {
        wav.extend_from_slice(&sample.to_le_bytes());
    }

    fs::write(path, wav).with_context(|| format!("write audio fixture {}", path.display()))?;
    Ok(())
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

fn write_config_with_agent_backend(
    data_dir: &Path,
    relay_url: &str,
    moq_url: &str,
    agent_api_url: &str,
) -> Result<()> {
    fs::create_dir_all(data_dir)
        .with_context(|| format!("create config dir {}", data_dir.display()))?;
    let path = data_dir.join("pika_config.json");
    let value = serde_json::json!({
        "disable_network": false,
        "relay_urls": [relay_url],
        "key_package_relay_urls": [relay_url],
        "agent_api_url": agent_api_url,
        "call_moq_url": moq_url,
        "call_broadcast_prefix": "pika/calls",
        "call_audio_backend": "synthetic",
    });
    fs::write(
        &path,
        serde_json::to_vec(&value).context("serialize config")?,
    )
    .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn write_config_with_relay(data_dir: &Path, relay_url: &str) -> Result<()> {
    fs::create_dir_all(data_dir)
        .with_context(|| format!("create config dir {}", data_dir.display()))?;
    let path = data_dir.join("pika_config.json");
    let value = serde_json::json!({
        "disable_network": false,
        "disable_agent_allowlist_probe": true,
        "relay_urls": [relay_url],
        "key_package_relay_urls": [relay_url],
        "call_moq_url": "ws://moq.local/anon",
        "call_broadcast_prefix": "pika/calls",
        "call_audio_backend": "synthetic",
    });
    fs::write(
        &path,
        serde_json::to_vec(&value).context("serialize config")?,
    )
    .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn write_config_offline(data_dir: &Path) -> Result<()> {
    fs::create_dir_all(data_dir)
        .with_context(|| format!("create config dir {}", data_dir.display()))?;
    let path = data_dir.join("pika_config.json");
    let value = serde_json::json!({
        "disable_network": true,
        "disable_agent_allowlist_probe": true,
        "call_moq_url": "https://moq.local/anon",
        "call_broadcast_prefix": "pika/calls",
        "call_audio_backend": "synthetic",
    });
    fs::write(
        &path,
        serde_json::to_vec(&value).context("serialize config")?,
    )
    .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn write_config_with_external_signer(data_dir: &Path) -> Result<()> {
    fs::create_dir_all(data_dir)
        .with_context(|| format!("create config dir {}", data_dir.display()))?;
    let path = data_dir.join("pika_config.json");
    let value = serde_json::json!({
        "disable_network": true,
        "disable_agent_allowlist_probe": true,
        "enable_external_signer": true,
        "call_moq_url": "https://moq.local/anon",
        "call_broadcast_prefix": "pika/calls",
        "call_audio_backend": "synthetic",
    });
    fs::write(
        &path,
        serde_json::to_vec(&value).context("serialize external signer config")?,
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

fn insert_agent_allowlist_row(database_url: &str, npub: &str) -> Result<()> {
    let escaped_npub = npub.replace('\'', "''");
    let sql = format!(
        "INSERT INTO agent_allowlist (npub, active, note, updated_by, updated_at) \
         VALUES ('{escaped_npub}', TRUE, 'deterministic', '{escaped_npub}', now()) \
         ON CONFLICT (npub) DO UPDATE \
         SET active = EXCLUDED.active, note = EXCLUDED.note, updated_by = EXCLUDED.updated_by, updated_at = now();"
    );
    let output = Command::new("psql")
        .args(["-v", "ON_ERROR_STOP=1", "-d", database_url, "-c", &sql])
        .output()
        .context("run psql to upsert agent allowlist")?;
    if !output.status.success() {
        bail!(
            "psql failed upserting agent allowlist row: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn rewrite_active_agent_identity(
    database_url: &str,
    owner_npub: &str,
    agent_npub: &str,
    timeout: Duration,
) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if update_active_agent_identity(database_url, owner_npub, agent_npub)? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!("timed out rewriting active agent identity for owner {owner_npub} to {agent_npub}");
}

fn update_active_agent_identity(
    database_url: &str,
    owner_npub: &str,
    agent_npub: &str,
) -> Result<bool> {
    let escaped_owner = owner_npub.replace('\'', "''");
    let escaped_agent = agent_npub.replace('\'', "''");
    let sql = format!(
        "WITH updated AS ( \
            UPDATE agent_instances \
            SET agent_id = '{escaped_agent}' \
            WHERE owner_npub = '{escaped_owner}' AND phase IN ('creating', 'ready') \
            RETURNING agent_id \
         ) \
         SELECT COUNT(*) FROM updated;"
    );
    let output = Command::new("psql")
        .args([
            "-v",
            "ON_ERROR_STOP=1",
            "-d",
            database_url,
            "-At",
            "-c",
            &sql,
        ])
        .output()
        .context("run psql to rewrite active agent identity")?;
    if !output.status.success() {
        bail!(
            "psql failed rewriting active agent identity: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim() == "1")
}

fn wait_for_agent_chat_outcome(
    app: &FfiApp,
    peer_npub: &str,
    timeout: Duration,
) -> Result<Vec<(AgentProvisioningPhase, String)>> {
    let start = Instant::now();
    let mut phase_history = Vec::new();
    let mut last_phase = None;
    while start.elapsed() < timeout {
        let state = app.state();
        if let Some(provisioning) = state.agent_provisioning.as_ref()
            && last_phase.as_ref() != Some(&provisioning.phase)
        {
            phase_history.push((
                provisioning.phase.clone(),
                provisioning.status_message.clone(),
            ));
            last_phase = Some(provisioning.phase.clone());
        }
        let chat_open = state
            .current_chat
            .as_ref()
            .map(|chat| {
                chat.group_name.is_none()
                    && chat.members.iter().any(|member| member.npub == peer_npub)
            })
            .unwrap_or(false);
        if chat_open && state.agent_provisioning.is_none() {
            return Ok(phase_history);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!(
        "timed out waiting for agent chat outcome; observed provisioning phases: {:?}",
        phase_history
    );
}

fn assert_phase_sequence(
    history: &[(AgentProvisioningPhase, String)],
    expected: &[AgentProvisioningPhase],
) -> Result<()> {
    let phases = history
        .iter()
        .map(|(phase, _)| phase.clone())
        .collect::<Vec<_>>();
    let mut cursor = 0usize;
    for expected_phase in expected {
        let Some(offset) = phases[cursor..]
            .iter()
            .position(|phase| phase == expected_phase)
        else {
            bail!("expected provisioning phase {expected_phase:?} in history {history:?}");
        };
        cursor += offset + 1;
    }
    Ok(())
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

// Keep selector-side DM bootstrap local here even though `rust/tests/support` has a similar
// helper: `pikahut` owns fixture/orchestration boundaries and cannot depend on the private
// `rust/tests` support layer that the narrower FFI semantic tests use.
fn dm_chat_id_for_peer(app: &FfiApp, peer_npub: &str) -> Option<String> {
    let state = app.state();
    if let Some(chat) = state.current_chat.as_ref().filter(|chat| {
        chat.group_name.is_none() && chat.members.iter().any(|member| member.npub == peer_npub)
    }) {
        return Some(chat.chat_id.clone());
    }
    state
        .chat_list
        .iter()
        .find(|chat| {
            chat.group_name.is_none() && chat.members.iter().any(|member| member.npub == peer_npub)
        })
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
