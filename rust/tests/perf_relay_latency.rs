//! Latency comparison: Nostr WebSocket relays vs MoQ QUIC relays.
//!
//! Measures pub→sub round-trip time for small payloads (typing-indicator sized)
//! across popular Nostr relays and deployed MoQ relays.
//!
//! Run:  cargo test -p pika_core --test perf_relay_latency -- --ignored --nocapture

use std::collections::HashMap;
use std::time::{Duration, Instant};

use nostr_sdk::nostr::{EventBuilder, EventId, Filter, Keys, Kind};
use nostr_sdk::{Client, RelayPoolNotification};
use pika_media::network::NetworkRelay;
use pika_media::session::MediaFrame;
use pika_media::tracks::TrackAddress;
use tokio::runtime::Runtime;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const NOSTR_RELAYS: &[(&str, &str)] = &[
    ("relay.primal.net", "wss://relay.primal.net"),
    ("nos.lol", "wss://nos.lol"),
    ("relay.damus.io", "wss://relay.damus.io"),
];

const MOQ_RELAYS: &[(&str, &str)] = &[
    ("us-east (ash)", "https://us-east.moq.logos.surf/anon"),
    ("us-west (hil)", "https://us-west.moq.logos.surf/anon"),
    ("germany (fsn)", "https://germany.moq.logos.surf/anon"),
    ("singapore (sin)", "https://singapore.moq.logos.surf/anon"),
];

/// Number of messages per run.
const MSG_COUNT: usize = 20;
/// Runs per relay.
const RUNS: usize = 3;
/// Small payload (~typing indicator JSON).
const PAYLOAD_SIZE: usize = 64;

// ---------------------------------------------------------------------------
// Nostr latency measurement
// ---------------------------------------------------------------------------

/// Publishes `MSG_COUNT` ephemeral events on a relay and measures how long
/// until the subscriber receives each one. Returns per-message latencies.
fn measure_nostr_relay(relay_url: &str) -> Result<Vec<Duration>, String> {
    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let pub_keys = Keys::generate();
        let sub_keys = Keys::generate();

        // Use kind 10_000 + 7777 = ephemeral-ish custom kind to avoid polluting relays.
        // Actually use kind 20_000-range (ephemeral) so relays don't store them.
        let kind = Kind::from(20_444);

        // Random tag to scope our subscription.
        let scope = format!("pika-bench-{}", rand::random::<u64>());

        // --- Subscriber ---
        let sub_client = Client::builder().signer(sub_keys.clone()).build();
        sub_client
            .add_relay(relay_url)
            .await
            .map_err(|e| format!("sub add_relay: {e}"))?;
        sub_client.connect().await;
        // Brief pause for connection to establish.
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Subscribe to events with our scope tag.
        let filter = Filter::new()
            .kind(kind)
            .custom_tag(
                nostr_sdk::nostr::SingleLetterTag::lowercase(nostr_sdk::nostr::Alphabet::Z),
                scope.clone(),
            )
            .since(nostr_sdk::nostr::Timestamp::now());

        sub_client
            .subscribe(filter, None)
            .await
            .map_err(|e| format!("subscribe: {e}"))?;

        // Channel to collect received event IDs with timestamps.
        let (rx_tx, mut rx_rx) = tokio::sync::mpsc::unbounded_channel::<(EventId, Instant)>();

        let notifications = sub_client.notifications();
        let rx_tx_clone = rx_tx.clone();
        let listener = tokio::spawn(async move {
            let mut notifications = notifications;
            loop {
                match notifications.recv().await {
                    Ok(RelayPoolNotification::Event { event, .. }) => {
                        let _ = rx_tx_clone.send((event.id, Instant::now()));
                    }
                    Ok(RelayPoolNotification::Shutdown) => break,
                    Err(_) => break,
                    _ => {}
                }
            }
        });

        // --- Publisher ---
        let pub_client = Client::builder().signer(pub_keys.clone()).build();
        pub_client
            .add_relay(relay_url)
            .await
            .map_err(|e| format!("pub add_relay: {e}"))?;
        pub_client.connect().await;
        tokio::time::sleep(Duration::from_millis(500)).await;

        // --- Publish and measure ---
        let payload = "x".repeat(PAYLOAD_SIZE);
        let mut send_times: HashMap<EventId, Instant> = HashMap::new();

        for i in 0..MSG_COUNT {
            let event = EventBuilder::new(kind, format!("{payload}:{i}"))
                .tag(nostr_sdk::nostr::Tag::custom(
                    nostr_sdk::nostr::TagKind::SingleLetter(
                        nostr_sdk::nostr::SingleLetterTag::lowercase(nostr_sdk::nostr::Alphabet::Z),
                    ),
                    vec![scope.clone()],
                ))
                .sign_with_keys(&pub_keys)
                .map_err(|e| format!("sign: {e}"))?;

            let eid = event.id;
            let t0 = Instant::now();
            pub_client
                .send_event_to(vec![relay_url], &event)
                .await
                .map_err(|e| format!("send: {e}"))?;
            send_times.insert(eid, t0);

            // Space out like typing indicator updates (~50ms).
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Wait for messages to arrive (up to 10s).
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut latencies: Vec<Duration> = Vec::new();
        let mut received = 0usize;

        while received < MSG_COUNT && Instant::now() < deadline {
            match tokio::time::timeout(
                deadline.saturating_duration_since(Instant::now()),
                rx_rx.recv(),
            )
            .await
            {
                Ok(Some((eid, recv_time))) => {
                    if let Some(send_time) = send_times.get(&eid) {
                        latencies.push(recv_time.duration_since(*send_time));
                        received += 1;
                    }
                }
                _ => break,
            }
        }

        listener.abort();
        let _ = pub_client.disconnect().await;
        let _ = sub_client.disconnect().await;

        if latencies.is_empty() {
            return Err("no messages received".to_string());
        }
        Ok(latencies)
    })
}

// ---------------------------------------------------------------------------
// MoQ latency measurement
// ---------------------------------------------------------------------------

/// Publishes `MSG_COUNT` frames through a MoQ relay and measures sub latency.
fn measure_moq_relay(relay_url: &str) -> Result<Vec<Duration>, String> {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros();
    let broadcast_path = format!("pika/bench/{unique}");
    let track_name = "typing0";

    let pub_relay =
        NetworkRelay::new(relay_url).map_err(|e| format!("pub NetworkRelay::new: {e}"))?;
    pub_relay
        .connect()
        .map_err(|e| format!("pub connect: {e}"))?;

    let sub_relay =
        NetworkRelay::new(relay_url).map_err(|e| format!("sub NetworkRelay::new: {e}"))?;
    sub_relay
        .connect()
        .map_err(|e| format!("sub connect: {e}"))?;

    let track = TrackAddress {
        broadcast_path: broadcast_path.clone(),
        track_name: track_name.to_string(),
    };

    // Warmup: publish a few frames so the track exists on the relay.
    for i in 0..3u64 {
        let frame = MediaFrame {
            seq: i,
            timestamp_us: 0,
            keyframe: true,
            payload: vec![0u8; 10],
        };
        let _ = pub_relay.publish(&track, frame);
    }
    std::thread::sleep(Duration::from_millis(500));

    let rx = sub_relay
        .subscribe(&track)
        .map_err(|e| format!("subscribe: {e}"))?;
    // Let subscription settle.
    std::thread::sleep(Duration::from_secs(1));

    // Drain any warmup frames.
    while rx.try_recv().is_ok() {}

    let mut latencies = Vec::new();

    for i in 0..MSG_COUNT as u64 {
        let frame = MediaFrame {
            seq: 100 + i,
            timestamp_us: (100 + i) * 20_000,
            keyframe: true,
            payload: vec![i as u8; PAYLOAD_SIZE],
        };

        let t0 = Instant::now();
        pub_relay
            .publish(&track, frame)
            .map_err(|e| format!("publish: {e}"))?;

        // Wait for this frame to arrive.
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(_) => {
                latencies.push(t0.elapsed());
            }
            Err(_) => {
                // Timed out waiting for frame.
            }
        }

        // Space out like typing indicators (~50ms).
        std::thread::sleep(Duration::from_millis(50));
    }

    pub_relay.disconnect();
    sub_relay.disconnect();

    if latencies.is_empty() {
        return Err("no frames received".to_string());
    }
    Ok(latencies)
}

// ---------------------------------------------------------------------------
// Stats helpers
// ---------------------------------------------------------------------------

fn median(vals: &mut [Duration]) -> Duration {
    vals.sort();
    if vals.is_empty() {
        return Duration::ZERO;
    }
    vals[vals.len() / 2]
}

fn mean(vals: &[Duration]) -> Duration {
    if vals.is_empty() {
        return Duration::ZERO;
    }
    let sum: Duration = vals.iter().sum();
    sum / vals.len() as u32
}

fn p95(vals: &mut [Duration]) -> Duration {
    vals.sort();
    if vals.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((vals.len() as f64) * 0.95).ceil() as usize - 1;
    vals[idx.min(vals.len() - 1)]
}

fn fmt_ms(d: Duration) -> String {
    format!("{:.1}ms", d.as_secs_f64() * 1000.0)
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

#[test]
#[ignore] // network-dependent; run explicitly
fn relay_latency_comparison() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    println!();
    println!("========================================================");
    println!("  Relay Latency Comparison: Nostr (WS) vs MoQ (QUIC)");
    println!("  msgs/run: {MSG_COUNT}  |  runs: {RUNS}  |  payload: {PAYLOAD_SIZE}B");
    println!("========================================================");
    println!();

    // ---- Nostr relays ----
    println!("--- Nostr Relays (WebSocket) ---");
    println!();

    let mut nostr_results: Vec<(&str, Vec<Duration>)> = Vec::new();

    for (name, url) in NOSTR_RELAYS {
        print!("{name:<24}");
        let mut all_latencies = Vec::new();

        for run in 0..RUNS {
            match measure_nostr_relay(url) {
                Ok(lats) => {
                    let n = lats.len();
                    all_latencies.extend(lats);
                    print!("  run{}: {n}/{MSG_COUNT}", run + 1);
                }
                Err(e) => {
                    print!("  run{}: FAIL({e})", run + 1);
                }
            }
            // Pause between runs.
            std::thread::sleep(Duration::from_millis(500));
        }
        println!();

        if !all_latencies.is_empty() {
            let med = median(&mut all_latencies.clone());
            let avg = mean(&all_latencies);
            let p = p95(&mut all_latencies.clone());
            let min = *all_latencies.iter().min().unwrap();
            let max = *all_latencies.iter().max().unwrap();
            println!(
                "  {:<22} median={}  mean={}  p95={}  min={}  max={}  n={}",
                "",
                fmt_ms(med),
                fmt_ms(avg),
                fmt_ms(p),
                fmt_ms(min),
                fmt_ms(max),
                all_latencies.len()
            );
        }
        nostr_results.push((name, all_latencies));
        println!();
    }

    // ---- MoQ relays ----
    println!("--- MoQ Relays (QUIC) ---");
    println!();

    let mut moq_results: Vec<(&str, Vec<Duration>)> = Vec::new();

    for (name, url) in MOQ_RELAYS {
        print!("{name:<24}");
        let mut all_latencies = Vec::new();

        for run in 0..RUNS {
            match measure_moq_relay(url) {
                Ok(lats) => {
                    let n = lats.len();
                    all_latencies.extend(lats);
                    print!("  run{}: {n}/{MSG_COUNT}", run + 1);
                }
                Err(e) => {
                    print!("  run{}: FAIL({e})", run + 1);
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        println!();

        if !all_latencies.is_empty() {
            let med = median(&mut all_latencies.clone());
            let avg = mean(&all_latencies);
            let p = p95(&mut all_latencies.clone());
            let min = *all_latencies.iter().min().unwrap();
            let max = *all_latencies.iter().max().unwrap();
            println!(
                "  {:<22} median={}  mean={}  p95={}  min={}  max={}  n={}",
                "",
                fmt_ms(med),
                fmt_ms(avg),
                fmt_ms(p),
                fmt_ms(min),
                fmt_ms(max),
                all_latencies.len()
            );
        }
        moq_results.push((name, all_latencies));
        println!();
    }

    // ---- Summary table ----
    println!("========================================================");
    println!("  Summary");
    println!("========================================================");
    println!(
        "{:<28} {:>10} {:>10} {:>10} {:>10} {:>6}",
        "Relay", "Median", "Mean", "P95", "Min", "Recv%"
    );
    println!("{}", "-".repeat(78));

    for (name, lats) in nostr_results {
        if lats.is_empty() {
            println!("{:<28} {:>10}", format!("[nostr] {name}"), "FAIL");
            continue;
        }
        let total = RUNS * MSG_COUNT;
        let pct = (lats.len() as f64 / total as f64) * 100.0;
        println!(
            "{:<28} {:>10} {:>10} {:>10} {:>10} {:>5.0}%",
            format!("[nostr] {name}"),
            fmt_ms(median(&mut lats.clone())),
            fmt_ms(mean(&lats)),
            fmt_ms(p95(&mut lats.clone())),
            fmt_ms(*lats.iter().min().unwrap()),
            pct,
        );
    }

    for (name, lats) in moq_results {
        if lats.is_empty() {
            println!("{:<28} {:>10}", format!("[moq]   {name}"), "FAIL");
            continue;
        }
        let total = RUNS * MSG_COUNT;
        let pct = (lats.len() as f64 / total as f64) * 100.0;
        println!(
            "{:<28} {:>10} {:>10} {:>10} {:>10} {:>5.0}%",
            format!("[moq]   {name}"),
            fmt_ms(median(&mut lats.clone())),
            fmt_ms(mean(&lats)),
            fmt_ms(p95(&mut lats.clone())),
            fmt_ms(*lats.iter().min().unwrap()),
            pct,
        );
    }

    println!();
}
