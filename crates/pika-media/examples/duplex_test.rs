//! Full-duplex encrypted audio test over the real MOQ relay.
//!
//! Simulates two call parties (Alice and Bob), each running a publish+subscribe
//! loop with Opus encode/decrypt through the real us-east.moq.logos.surf relay.
//! This validates Layer 3 of the debugging ladder without needing the iOS app,
//! MLS signaling, or Nostr.

#[cfg(not(feature = "network"))]
fn main() {
    eprintln!("This example requires `--features network`.");
}

#[cfg(feature = "network")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(feature = "network")]
use std::sync::Arc;
#[cfg(feature = "network")]
use std::time::{Duration, Instant};

#[cfg(feature = "network")]
use pika_media::crypto::{decrypt_frame, encrypt_frame, FrameInfo, FrameKeyMaterial};
#[cfg(feature = "network")]
use pika_media::network::NetworkRelay;
#[cfg(feature = "network")]
use pika_media::session::MediaFrame;
#[cfg(feature = "network")]
use pika_media::tracks::TrackAddress;

#[cfg(feature = "network")]
const FRAME_DURATION: Duration = Duration::from_millis(20);
#[cfg(feature = "network")]
const TEST_DURATION: Duration = Duration::from_secs(10);

#[cfg(feature = "network")]
fn make_keys(label: &str) -> FrameKeyMaterial {
    let seed = b"test-shared-secret-for-duplex-validation-123456";
    FrameKeyMaterial::from_fallback_context(seed, label.as_bytes(), 1, 0, "audio0")
}

#[cfg(feature = "network")]
struct Party {
    name: String,
    relay: NetworkRelay,
    tx_track: TrackAddress,
    rx_track: TrackAddress,
    tx_keys: FrameKeyMaterial,
    rx_keys: FrameKeyMaterial,
    tx_frames: Arc<AtomicU64>,
    rx_frames: Arc<AtomicU64>,
    rx_crypto_errors: Arc<AtomicU64>,
}

#[cfg(feature = "network")]
fn main() {
    let relay_url = "https://us-east.moq.logos.surf/anon";
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    let broadcast_base = format!("pika/calls/duplex-test-{unique}");

    println!("=== Full-duplex encrypted audio test ===");
    println!("relay: {relay_url}");
    println!("broadcast_base: {broadcast_base}");
    println!("duration: {}s", TEST_DURATION.as_secs());

    let alice_label = "alice";
    let bob_label = "bob";

    // Alice publishes to alice's broadcast, subscribes to bob's
    let alice = Party {
        name: "Alice".to_string(),
        relay: NetworkRelay::new(relay_url).expect("alice relay"),
        tx_track: TrackAddress {
            broadcast_path: format!("{broadcast_base}/{alice_label}"),
            track_name: "audio0".to_string(),
        },
        rx_track: TrackAddress {
            broadcast_path: format!("{broadcast_base}/{bob_label}"),
            track_name: "audio0".to_string(),
        },
        tx_keys: make_keys(alice_label),
        rx_keys: make_keys(bob_label),
        tx_frames: Arc::new(AtomicU64::new(0)),
        rx_frames: Arc::new(AtomicU64::new(0)),
        rx_crypto_errors: Arc::new(AtomicU64::new(0)),
    };

    // Bob publishes to bob's broadcast, subscribes to alice's
    let bob = Party {
        name: "Bob".to_string(),
        relay: NetworkRelay::new(relay_url).expect("bob relay"),
        tx_track: TrackAddress {
            broadcast_path: format!("{broadcast_base}/{bob_label}"),
            track_name: "audio0".to_string(),
        },
        rx_track: TrackAddress {
            broadcast_path: format!("{broadcast_base}/{alice_label}"),
            track_name: "audio0".to_string(),
        },
        tx_keys: make_keys(bob_label),
        rx_keys: make_keys(alice_label),
        tx_frames: Arc::new(AtomicU64::new(0)),
        rx_frames: Arc::new(AtomicU64::new(0)),
        rx_crypto_errors: Arc::new(AtomicU64::new(0)),
    };

    // Connect both
    alice.relay.connect().expect("alice connect");
    bob.relay.connect().expect("bob connect");
    println!("both parties connected to relay");

    // Publish a few warmup frames so broadcasts exist before subscribing
    for i in 0..3 {
        let frame = MediaFrame {
            seq: i,
            timestamp_us: 0,
            keyframe: true,
            payload: vec![0u8; 10],
        };
        alice.relay.publish(&alice.tx_track, frame.clone()).ok();
        bob.relay.publish(&bob.tx_track, frame).ok();
    }

    std::thread::sleep(Duration::from_millis(1000));

    // Subscribe both
    let alice_rx = alice
        .relay
        .subscribe(&alice.rx_track)
        .expect("alice subscribe");
    let bob_rx = bob.relay.subscribe(&bob.rx_track).expect("bob subscribe");
    println!("both parties subscribed, waiting for propagation...");

    std::thread::sleep(Duration::from_secs(2));

    let stop = Arc::new(AtomicBool::new(false));

    // Spawn TX threads
    let alice_tx_handle = spawn_tx_thread(&alice, stop.clone());
    let bob_tx_handle = spawn_tx_thread(&bob, stop.clone());

    // Spawn RX threads
    let alice_rx_handle = spawn_rx_thread("Alice", alice_rx, &alice, stop.clone());
    let bob_rx_handle = spawn_rx_thread("Bob", bob_rx, &bob, stop.clone());

    // Run for test duration, printing stats every 2s
    let start = Instant::now();
    while start.elapsed() < TEST_DURATION {
        std::thread::sleep(Duration::from_secs(2));
        print_stats(&alice, &bob);
    }

    stop.store(true, Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(500));

    // Final stats
    println!("\n=== Final Results ===");
    print_stats(&alice, &bob);

    let alice_tx = alice.tx_frames.load(Ordering::Relaxed);
    let alice_rx = alice.rx_frames.load(Ordering::Relaxed);
    let bob_tx = bob.tx_frames.load(Ordering::Relaxed);
    let bob_rx = bob.rx_frames.load(Ordering::Relaxed);

    // Require at least 10 rx frames per direction to catch the "1-frame" bug.
    // With 10s at 20ms/frame, we expect ~500 frames; 10 is a very conservative floor.
    let min_rx = 10;
    let pass = alice_tx > 0 && alice_rx >= min_rx && bob_tx > 0 && bob_rx >= min_rx;
    if pass {
        println!("\nPASS: Full-duplex encrypted audio works over real MOQ relay");
    } else {
        println!("\nFAIL: Insufficient frames (need rx>={min_rx} each direction)");
        println!("  alice: tx={alice_tx} rx={alice_rx}");
        println!("  bob:   tx={bob_tx} rx={bob_rx}");
        std::process::exit(1);
    }

    // Check crypto errors
    let alice_crypto_err = alice.rx_crypto_errors.load(Ordering::Relaxed);
    let bob_crypto_err = bob.rx_crypto_errors.load(Ordering::Relaxed);
    if alice_crypto_err > 0 || bob_crypto_err > 0 {
        println!("WARNING: crypto errors: alice={alice_crypto_err}, bob={bob_crypto_err}");
    }

    alice.relay.disconnect();
    bob.relay.disconnect();
    drop(alice_tx_handle);
    drop(bob_tx_handle);
    drop(alice_rx_handle);
    drop(bob_rx_handle);
}

#[cfg(feature = "network")]
fn spawn_tx_thread(party: &Party, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    let relay = party.relay.clone();
    let track = party.tx_track.clone();
    let keys = party.tx_keys.clone();
    let counter = party.tx_frames.clone();

    std::thread::spawn(move || {
        let mut seq = 10u64; // skip warmup seqs
        let mut tx_counter = 0u32;
        let mut next_tick = Instant::now();

        while !stop.load(Ordering::Relaxed) {
            // Generate synthetic audio (just zeros for testing)
            let pcm_data = vec![0u8; 160]; // simulated opus packet

            let frame_info = FrameInfo {
                counter: tx_counter,
                group_seq: seq,
                frame_idx: 0,
                keyframe: true,
            };
            tx_counter += 1;

            let encrypted = match encrypt_frame(&pcm_data, &keys, frame_info) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let frame = MediaFrame {
                seq,
                timestamp_us: seq * 20_000,
                keyframe: true,
                payload: encrypted,
            };

            if relay.publish(&track, frame).is_ok() {
                counter.fetch_add(1, Ordering::Relaxed);
                seq += 1;
            }

            next_tick += FRAME_DURATION;
            let now = Instant::now();
            if next_tick > now {
                std::thread::sleep(next_tick.saturating_duration_since(now));
            } else {
                next_tick = now;
            }
        }
    })
}

#[cfg(feature = "network")]
fn spawn_rx_thread(
    _name: &str,
    rx: pika_media::subscription::MediaFrameSubscription,
    party: &Party,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    let counter = party.rx_frames.clone();
    let crypto_errors = party.rx_crypto_errors.clone();
    let keys = party.rx_keys.clone();
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(frame) => match decrypt_frame(&frame.payload, &keys) {
                    Ok(_decrypted) => {
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        crypto_errors.fetch_add(1, Ordering::Relaxed);
                    }
                },
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    })
}

#[cfg(feature = "network")]
fn print_stats(alice: &Party, bob: &Party) {
    let a_tx = alice.tx_frames.load(Ordering::Relaxed);
    let a_rx = alice.rx_frames.load(Ordering::Relaxed);
    let a_err = alice.rx_crypto_errors.load(Ordering::Relaxed);
    let b_tx = bob.tx_frames.load(Ordering::Relaxed);
    let b_rx = bob.rx_frames.load(Ordering::Relaxed);
    let b_err = bob.rx_crypto_errors.load(Ordering::Relaxed);
    println!(
        "  Alice: tx={a_tx} rx={a_rx} crypto_err={a_err} | Bob: tx={b_tx} rx={b_rx} crypto_err={b_err}"
    );
}
