//! E2E call tests: signaling + media transport via pikahub.
//!
//! All tests are #[ignore] -- they require moq-relay and/or pikachat binaries
//! on PATH (use `nix develop`). They run in nightly, not pre-merge.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pika_core::{AppAction, AuthState, CallStatus, FfiApp};
use tempfile::tempdir;

mod support;
use support::{wait_until, write_config_multi, write_config_with_moq};

#[derive(Clone, Copy, Debug)]
struct CallStatsSnapshot {
    tx_frames: u64,
    rx_frames: u64,
    jitter_buffer_ms: u32,
}

fn call_stats_snapshot(app: &FfiApp) -> Option<CallStatsSnapshot> {
    let call = app.state().active_call?;
    let debug = call.debug?;
    Some(CallStatsSnapshot {
        tx_frames: debug.tx_frames,
        rx_frames: debug.rx_frames,
        jitter_buffer_ms: debug.jitter_buffer_ms,
    })
}

fn dm_chat_id_for_peer(app: &FfiApp, peer_npub: &str) -> Option<String> {
    let st = app.state();
    if let Some(chat) = st
        .current_chat
        .as_ref()
        .filter(|c| c.members.iter().any(|m| m.npub == peer_npub))
    {
        return Some(chat.chat_id.clone());
    }
    st.chat_list
        .iter()
        .find(|c| c.members.iter().any(|m| m.npub == peer_npub))
        .map(|c| c.chat_id.clone())
}

fn create_or_open_dm_chat(app: &FfiApp, peer_npub: &str, timeout: Duration) -> String {
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
                    .map(|c| c.chat_id == chat_id)
                    .unwrap_or(false)
            });
            return chat_id;
        }
        app.dispatch(AppAction::CreateChat {
            peer_npub: peer_npub.to_owned(),
        });
        std::thread::sleep(Duration::from_millis(500));
    }
    panic!("chat for peer {peer_npub} was not ready within {timeout:?}");
}

// ---------------------------------------------------------------------------
// FfiApp ↔ FfiApp call over local moq-relay
// ---------------------------------------------------------------------------

#[test]
#[ignore] // requires moq-relay on PATH (use `nix develop`)
fn call_over_local_moq_relay() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let infra = support::TestInfra::start_relay_and_moq();
    let moq_url = infra.moq_url.as_ref().expect("moq_url required");

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config_with_moq(
        &dir_a.path().to_string_lossy(),
        &infra.relay_url,
        Some(&infra.relay_url),
        moq_url,
    );
    write_config_with_moq(
        &dir_b.path().to_string_lossy(),
        &infra.relay_url,
        Some(&infra.relay_url),
        moq_url,
    );

    let alice = FfiApp::new(dir_a.path().to_string_lossy().to_string(), String::new());
    let bob = FfiApp::new(dir_b.path().to_string_lossy().to_string(), String::new());

    alice.dispatch(AppAction::CreateAccount);
    bob.dispatch(AppAction::CreateAccount);
    wait_until("alice logged in", Duration::from_secs(10), || {
        matches!(alice.state().auth, AuthState::LoggedIn { .. })
    });
    wait_until("bob logged in", Duration::from_secs(10), || {
        matches!(bob.state().auth, AuthState::LoggedIn { .. })
    });

    let bob_npub = match bob.state().auth {
        AuthState::LoggedIn { npub, .. } => npub,
        _ => unreachable!(),
    };

    let chat_id = create_or_open_dm_chat(&alice, &bob_npub, Duration::from_secs(90));
    wait_until("bob chat id matches", Duration::from_secs(45), || {
        bob.state().chat_list.iter().any(|c| c.chat_id == chat_id)
    });

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
    });
    wait_until("bob ringing", Duration::from_secs(10), || {
        bob.state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Ringing))
            .unwrap_or(false)
    });

    bob.dispatch(AppAction::AcceptCall {
        chat_id: chat_id.clone(),
    });
    wait_until("bob connecting or active", Duration::from_secs(30), || {
        bob.state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Connecting | CallStatus::Active))
            .unwrap_or(false)
    });
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
    );

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
    );
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
    );

    // Verify frames continue flowing.
    let alice_snap = call_stats_snapshot(&alice).expect("alice stats");
    let bob_snap = call_stats_snapshot(&bob).expect("bob stats");
    std::thread::sleep(Duration::from_secs(2));
    let alice_after = call_stats_snapshot(&alice).expect("alice stats after");
    let bob_after = call_stats_snapshot(&bob).expect("bob stats after");
    assert!(
        alice_after.tx_frames > alice_snap.tx_frames,
        "alice should keep transmitting"
    );
    assert!(
        bob_after.rx_frames > bob_snap.rx_frames,
        "bob should keep receiving"
    );
    assert!(
        alice_after.jitter_buffer_ms <= 240,
        "alice jitter buffer should stay bounded, got {}ms",
        alice_after.jitter_buffer_ms
    );
    assert!(
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
    });
    wait_until("bob call ended", Duration::from_secs(10), || {
        bob.state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Ended { .. }))
            .unwrap_or(true)
    });

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
}

// ---------------------------------------------------------------------------
// FfiApp ↔ pikachat daemon call over local infra
// ---------------------------------------------------------------------------

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

struct DaemonHandle {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout_lines: Arc<Mutex<Vec<serde_json::Value>>>,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
    stdout_thread: Option<std::thread::JoinHandle<()>>,
}

impl DaemonHandle {
    fn spawn(relay_url: &str, state_dir: &str) -> Self {
        let bin = pikachat_binary();
        assert!(
            std::path::Path::new(&bin).exists(),
            "pikachat binary not found at {bin}. Build it: cargo build -p pikachat"
        );
        let use_real_ai = std::env::var("OPENAI_API_KEY").is_ok();
        eprintln!(
            "[daemon] spawning {bin} daemon --relay {relay_url} --state-dir {state_dir} real_ai={use_real_ai}"
        );
        let mut cmd = Command::new(&bin);
        cmd.arg("daemon")
            .arg("--relay")
            .arg(relay_url)
            .arg("--state-dir")
            .arg(state_dir);
        if use_real_ai {
            cmd.env("OPENAI_API_KEY", std::env::var("OPENAI_API_KEY").unwrap());
        } else {
            cmd.env("PIKACHAT_TTS_FIXTURE", "1");
        }
        let mut child = cmd
            .env(
                "PIKACHAT_ECHO_MODE",
                std::env::var("PIKACHAT_ECHO_MODE").unwrap_or_default(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn pikachat failed: {e}"));

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let stderr_thread = std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                eprintln!("[pikachat stderr] {line}");
            }
        });

        let stdout_lines: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_for_thread = stdout_lines.clone();
        let stdout_thread = std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                eprintln!("[pikachat stdout] {line}");
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                    lines_for_thread.lock().unwrap().push(v);
                }
            }
        });

        Self {
            child,
            stdin,
            stdout_lines,
            stderr_thread: Some(stderr_thread),
            stdout_thread: Some(stdout_thread),
        }
    }

    fn send_cmd(&mut self, v: serde_json::Value) {
        let s = serde_json::to_string(&v).unwrap();
        writeln!(self.stdin, "{s}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn wait_for_event(
        &self,
        what: &str,
        timeout: Duration,
        pred: impl Fn(&serde_json::Value) -> bool,
    ) -> serde_json::Value {
        let start = Instant::now();
        let mut last_idx = 0;
        while start.elapsed() < timeout {
            let lines = self.stdout_lines.lock().unwrap();
            for i in last_idx..lines.len() {
                if pred(&lines[i]) {
                    return lines[i].clone();
                }
            }
            last_idx = lines.len();
            drop(lines);
            std::thread::sleep(Duration::from_millis(50));
        }
        let lines = self.stdout_lines.lock().unwrap();
        eprintln!("[daemon] all {what} stdout events ({} total):", lines.len());
        for (i, l) in lines.iter().enumerate() {
            eprintln!("  [{i}] {l}");
        }
        panic!("{what}: daemon event not received within {timeout:?}");
    }

    fn npub(&self) -> String {
        let lines = self.stdout_lines.lock().unwrap();
        for l in lines.iter() {
            if l.get("type").and_then(|t| t.as_str()) == Some("ready") {
                return l.get("npub").and_then(|p| p.as_str()).unwrap().to_string();
            }
        }
        panic!("daemon ready event not found");
    }

    fn pubkey(&self) -> String {
        let lines = self.stdout_lines.lock().unwrap();
        for l in lines.iter() {
            if l.get("type").and_then(|t| t.as_str()) == Some("ready") {
                return l
                    .get("pubkey")
                    .and_then(|p| p.as_str())
                    .unwrap()
                    .to_string();
            }
        }
        panic!("daemon ready event not found");
    }
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(t) = self.stderr_thread.take() {
            let _ = t.join();
        }
        if let Some(t) = self.stdout_thread.take() {
            let _ = t.join();
        }
    }
}

fn pikachat_binary() -> String {
    if let Ok(bin) = std::env::var("PIKACHAT_BIN") {
        if !bin.trim().is_empty() {
            return bin;
        }
    }
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    repo_root
        .join("target/debug/pikachat")
        .to_string_lossy()
        .to_string()
}

fn run_pikachat_call_test(relay_url: &str, moq_url: &str) {
    let bin = pikachat_binary();
    if !std::path::Path::new(&bin).exists() {
        panic!("pikachat binary not found at {bin}. Build it: cargo build -p pikachat");
    }

    // Generate an audio fixture that alternates 1s tone / 1s silence.
    let fixture_dir = tempdir().unwrap();
    let fixture_path = fixture_dir.path().join("alternating.wav");
    {
        let sample_rate = 48_000u32;
        let duration_secs = 10u32;
        let total_samples = sample_rate * duration_secs;
        let mut pcm = Vec::with_capacity(total_samples as usize);
        let freq = 440.0f32;
        let step = 2.0f32 * std::f32::consts::PI * freq / sample_rate as f32;
        let samples_per_sec = sample_rate as usize;
        for i in 0..total_samples as usize {
            let second = i / samples_per_sec;
            let sample = if second.is_multiple_of(2) {
                (((i as f32) * step).sin() * (i16::MAX as f32 * 0.3)) as i16
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
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for s in &pcm {
            wav.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(&fixture_path, &wav).unwrap();
    }
    std::env::set_var("PIKA_AUDIO_FIXTURE", fixture_path.to_str().unwrap());
    eprintln!("[test] audio fixture: {}", fixture_path.display());
    eprintln!("[test] using relay: {relay_url}");

    let daemon_state = tempdir().unwrap();
    let mut daemon = DaemonHandle::spawn(relay_url, &daemon_state.path().to_string_lossy());

    daemon.wait_for_event("daemon ready", Duration::from_secs(15), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("ready")
    });
    let daemon_npub = daemon.npub();
    let daemon_pubkey = daemon.pubkey();
    eprintln!("[test] daemon pubkey={daemon_pubkey} npub={daemon_npub}");

    daemon.send_cmd(serde_json::json!({
        "cmd": "set_relays",
        "request_id": "sr1",
        "relays": [relay_url]
    }));
    daemon.wait_for_event("set_relays ok", Duration::from_secs(15), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("ok")
            && v.get("request_id").and_then(|id| id.as_str()) == Some("sr1")
    });

    daemon.send_cmd(serde_json::json!({
        "cmd": "publish_keypackage",
        "request_id": "kp1",
        "relays": [relay_url]
    }));
    daemon.wait_for_event("kp published", Duration::from_secs(15), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("ok")
            && v.get("request_id").and_then(|id| id.as_str()) == Some("kp1")
    });

    let caller_dir = tempdir().unwrap();
    write_config_with_moq(
        &caller_dir.path().to_string_lossy(),
        relay_url,
        Some(relay_url),
        moq_url,
    );
    let caller = FfiApp::new(
        caller_dir.path().to_string_lossy().to_string(),
        String::new(),
    );

    caller.dispatch(AppAction::CreateAccount);
    wait_until("caller logged in", Duration::from_secs(10), || {
        matches!(caller.state().auth, AuthState::LoggedIn { .. })
    });

    let chat_id = create_or_open_dm_chat(&caller, &daemon_npub, Duration::from_secs(90));
    eprintln!("[test] chat created: {chat_id}");

    let welcome = daemon.wait_for_event("daemon welcome_received", Duration::from_secs(30), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("welcome_received")
    });
    let wrapper_id = welcome
        .get("wrapper_event_id")
        .and_then(|x| x.as_str())
        .unwrap()
        .to_string();

    daemon.send_cmd(serde_json::json!({
        "cmd": "accept_welcome",
        "request_id": "acc1",
        "wrapper_event_id": wrapper_id
    }));
    daemon.wait_for_event("daemon group_joined", Duration::from_secs(30), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("group_joined")
    });

    // Ping/pong
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let ping_msg = format!("ping:{nonce}");
    let pong_msg = format!("pong:{nonce}");

    caller.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: ping_msg.clone(),
        kind: None,
        reply_to_message_id: None,
    });

    let msg = daemon.wait_for_event(
        "daemon message_received (ping)",
        Duration::from_secs(30),
        |v| {
            v.get("type").and_then(|t| t.as_str()) == Some("message_received")
                && v.get("content")
                    .and_then(|c| c.as_str())
                    .map(|c| c == ping_msg)
                    .unwrap_or(false)
        },
    );

    let nostr_group_id = msg
        .get("nostr_group_id")
        .and_then(|x| x.as_str())
        .unwrap()
        .to_string();

    daemon.send_cmd(serde_json::json!({
        "cmd": "send_message",
        "request_id": "pong1",
        "nostr_group_id": nostr_group_id,
        "content": pong_msg
    }));
    daemon.wait_for_event("pong send ok", Duration::from_secs(15), |v| {
        v.get("type").and_then(|t| t.as_str()) == Some("ok")
            && v.get("request_id").and_then(|id| id.as_str()) == Some("pong1")
    });

    wait_until("caller received pong", Duration::from_secs(30), || {
        caller
            .state()
            .current_chat
            .as_ref()
            .and_then(|c| c.messages.iter().find(|m| m.content == pong_msg))
            .is_some()
    });
    eprintln!("[test] PASS: ping/pong works");

    // Call signaling
    caller.dispatch(AppAction::StartCall {
        chat_id: chat_id.clone(),
    });
    wait_until("caller offering", Duration::from_secs(10), || {
        caller
            .state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Offering))
            .unwrap_or(false)
    });

    let invite = daemon.wait_for_event(
        "daemon call_invite_received",
        Duration::from_secs(30),
        |v| v.get("type").and_then(|t| t.as_str()) == Some("call_invite_received"),
    );
    let call_id = invite
        .get("call_id")
        .and_then(|x| x.as_str())
        .unwrap()
        .to_string();

    daemon.send_cmd(serde_json::json!({
        "cmd": "accept_call",
        "request_id": "accept1",
        "call_id": call_id
    }));

    daemon.wait_for_event(
        "daemon call_session_started",
        Duration::from_secs(30),
        |v| v.get("type").and_then(|t| t.as_str()) == Some("call_session_started"),
    );

    wait_until(
        "caller active with tx frames",
        Duration::from_secs(30),
        || {
            caller
                .state()
                .active_call
                .as_ref()
                .map(|c| {
                    matches!(c.status, CallStatus::Active)
                        && c.debug.as_ref().map(|d| d.tx_frames > 5).unwrap_or(false)
                })
                .unwrap_or(false)
        },
    );

    let require_rx = std::env::var("PIKACHAT_ECHO_MODE")
        .map(|v| !v.trim().is_empty() && v.trim() != "0")
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
                    .and_then(|c| c.debug.as_ref().map(|d| d.rx_frames > 0))
                    .unwrap_or(false)
            },
        );
    } else if use_real_ai {
        daemon.wait_for_event("daemon accumulating audio", Duration::from_secs(30), |v| {
            v.get("type").and_then(|t| t.as_str()) == Some("call_debug")
                && v.get("call_id")
                    .and_then(|c| c.as_str())
                    .map(|c| c == call_id)
                    .unwrap_or(false)
                && v.get("rx_frames")
                    .and_then(|n| n.as_u64())
                    .map(|n| n >= 200)
                    .unwrap_or(false)
        });
    } else {
        daemon.wait_for_event(
            "daemon stt receiving frames",
            Duration::from_secs(20),
            |v| {
                v.get("type").and_then(|t| t.as_str()) == Some("call_debug")
                    && v.get("call_id")
                        .and_then(|c| c.as_str())
                        .map(|c| c == call_id)
                        .unwrap_or(false)
                    && v.get("rx_frames")
                        .and_then(|n| n.as_u64())
                        .map(|n| n > 0)
                        .unwrap_or(false)
            },
        );
    }

    if !require_rx {
        let audio_chunk =
            daemon.wait_for_event("daemon call_audio_chunk", Duration::from_secs(30), |v| {
                v.get("type").and_then(|t| t.as_str()) == Some("call_audio_chunk")
                    && v.get("call_id")
                        .and_then(|c| c.as_str())
                        .map(|c| c == call_id)
                        .unwrap_or(false)
            });
        let audio_path = audio_chunk
            .get("audio_path")
            .and_then(|p| p.as_str())
            .unwrap_or("")
            .to_string();
        assert!(!audio_path.is_empty(), "expected non-empty audio_path");
        let wav_data = std::fs::read(&audio_path)
            .unwrap_or_else(|e| panic!("failed to read WAV at {audio_path}: {e}"));
        assert!(wav_data.len() > 44, "WAV file too short");
        assert_eq!(&wav_data[0..4], b"RIFF");
        assert_eq!(&wav_data[8..12], b"WAVE");

        // Test TTS
        let tts_text = "This is a test of the text to speech system.";
        daemon.send_cmd(serde_json::json!({
            "cmd": "send_audio_response",
            "request_id": "tts1",
            "call_id": call_id,
            "tts_text": tts_text,
        }));
        let tts_timeout = if use_real_ai {
            Duration::from_secs(45)
        } else {
            Duration::from_secs(30)
        };
        let tts_result = daemon.wait_for_event("send_audio_response result", tts_timeout, |v| {
            v.get("request_id").and_then(|id| id.as_str()) == Some("tts1")
        });
        let tts_ok = tts_result
            .get("type")
            .and_then(|t| t.as_str())
            .map(|t| t == "ok")
            .unwrap_or(false);
        assert!(tts_ok, "TTS publish failed: {tts_result}");

        wait_until(
            "caller receiving TTS frames",
            Duration::from_secs(30),
            || {
                caller
                    .state()
                    .active_call
                    .as_ref()
                    .and_then(|c| c.debug.as_ref().map(|d| d.rx_frames > 0))
                    .unwrap_or(false)
            },
        );
    }

    caller.dispatch(AppAction::EndCall);
    wait_until("caller call ended", Duration::from_secs(10), || {
        caller
            .state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Ended { .. }))
            .unwrap_or(true)
    });

    if let Some(debug) = caller
        .state()
        .active_call
        .as_ref()
        .and_then(|c| c.debug.as_ref())
    {
        eprintln!(
            "[test] caller final: tx={} rx={} dropped={}",
            debug.tx_frames, debug.rx_frames, debug.rx_dropped
        );
    }

    eprintln!("[test] PASS: pikachat call test on {relay_url}");
    drop(daemon);
}

#[test]
#[ignore] // requires pikachat + moq-relay binaries (use `nix develop`)
fn call_with_pikachat_daemon() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let infra = support::TestInfra::start_relay_and_moq();
    let moq_url = infra.moq_url.as_ref().expect("moq_url required");
    run_pikachat_call_test(&infra.relay_url, moq_url);
}

// ---------------------------------------------------------------------------
// Deployed bot call (requires PIKA_TEST_NSEC -- production)
// ---------------------------------------------------------------------------

const DEFAULT_BOT_NPUB: &str = "npub1z6ujr8rad5zp9sr9w22rkxm0truulf2jntrks6rlwskhdmqsawpqmnjlcp";
const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.primal.net",
    "wss://nos.lol",
    "wss://relay.damus.io",
];
const DEFAULT_KP_RELAYS: &[&str] = &[
    "wss://nostr-pub.wellorder.net",
    "wss://nostr-01.yakihonne.com",
    "wss://nostr-02.yakihonne.com",
];
const DEFAULT_MOQ_URL: &str = "https://us-east.moq.logos.surf/anon";

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn env_csv_or(key: &str, defaults: &[&str]) -> Vec<String> {
    if let Ok(s) = std::env::var(key) {
        let v: Vec<String> = s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();
        if !v.is_empty() {
            return v;
        }
    }
    defaults.iter().map(|s| s.to_string()).collect()
}

#[test]
#[ignore] // requires PIKA_TEST_NSEC + production infrastructure
fn call_deployed_bot() {
    pika_core::init_rustls_crypto_provider();

    let nsec = match std::env::var("PIKA_TEST_NSEC")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        Some(v) => v.trim().to_string(),
        None => {
            eprintln!("SKIP: set PIKA_TEST_NSEC to run this test");
            return;
        }
    };
    let bot_npub = env_or("PIKA_BOT_NPUB", DEFAULT_BOT_NPUB);
    let relays = env_csv_or("PIKA_E2E_RELAYS", DEFAULT_RELAYS);
    let kp_relays = env_csv_or("PIKA_E2E_KP_RELAYS", DEFAULT_KP_RELAYS);
    let moq_url = env_or("PIKA_CALL_MOQ_URL", DEFAULT_MOQ_URL);

    if std::env::var("PIKA_AUDIO_FIXTURE").is_err() {
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/speech_prompt.wav"
        );
        if std::path::Path::new(fixture).exists() {
            std::env::set_var("PIKA_AUDIO_FIXTURE", fixture);
        }
    }

    eprintln!("[test] bot_npub={bot_npub}");
    eprintln!("[test] relays={relays:?}");
    eprintln!("[test] moq_url={moq_url}");

    let dir = tempdir().unwrap();
    write_config_multi(&dir.path().to_string_lossy(), &relays, &kp_relays, &moq_url);

    let app = FfiApp::new(dir.path().to_string_lossy().to_string(), String::new());

    app.dispatch(AppAction::Login { nsec });
    wait_until("logged in", Duration::from_secs(20), || {
        matches!(app.state().auth, AuthState::LoggedIn { .. })
    });

    app.dispatch(AppAction::CreateChat {
        peer_npub: bot_npub.clone(),
    });
    wait_until("chat opened", Duration::from_secs(120), || {
        app.state().current_chat.is_some()
    });
    let chat_id = app.state().current_chat.as_ref().unwrap().chat_id.clone();

    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let ping = format!("ping:{nonce}");
    let pong = format!("pong:{nonce}");
    app.dispatch(AppAction::SendMessage {
        chat_id: chat_id.clone(),
        content: ping,
        kind: None,
        reply_to_message_id: None,
    });
    wait_until("bot pong", Duration::from_secs(30), || {
        app.state()
            .current_chat
            .as_ref()
            .map(|c| c.messages.iter().any(|m| m.content == pong))
            .unwrap_or(false)
    });

    app.dispatch(AppAction::StartCall {
        chat_id: chat_id.clone(),
    });
    wait_until("call active", Duration::from_secs(60), || {
        app.state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Active))
            .unwrap_or(false)
    });

    wait_until("tx frames flowing", Duration::from_secs(10), || {
        app.state()
            .active_call
            .as_ref()
            .and_then(|c| c.debug.as_ref())
            .map(|d| d.tx_frames > 10)
            .unwrap_or(false)
    });

    let media_window = Duration::from_secs(20);
    let media_start = Instant::now();
    let mut max_rx: u64 = 0;
    while media_start.elapsed() < media_window {
        if let Some(dbg) = app
            .state()
            .active_call
            .as_ref()
            .and_then(|c| c.debug.as_ref())
        {
            if dbg.rx_frames > max_rx {
                max_rx = dbg.rx_frames;
            }
        }
        if max_rx >= 10 {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    assert!(
        max_rx >= 5,
        "expected at least 5 rx frames from bot (got {max_rx})"
    );

    app.dispatch(AppAction::EndCall);
    wait_until("call ended", Duration::from_secs(15), || {
        app.state()
            .active_call
            .as_ref()
            .map(|c| matches!(c.status, CallStatus::Ended { .. }))
            .unwrap_or(false)
    });

    eprintln!("[test] PASS: call_deployed_bot");
}
