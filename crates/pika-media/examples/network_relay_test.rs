#[cfg(not(feature = "network"))]
fn main() {
    eprintln!("This example requires `--features network`.");
}

#[cfg(feature = "network")]
fn main() {
    use std::time::Duration;

    use pika_media::network::NetworkRelay;
    use pika_media::session::MediaFrame;
    use pika_media::tracks::TrackAddress;

    let relay_url = "https://us-east.moq.logos.surf/anon";
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    let broadcast_path = format!("pika/test/network-relay-{unique}");
    let track_name = "audio0".to_string();

    println!("Testing NetworkRelay against {relay_url}");
    println!("  broadcast: {broadcast_path}");
    println!("  track: {track_name}");

    // Publisher
    let pub_relay = NetworkRelay::new(relay_url).expect("publisher relay");
    pub_relay.connect().expect("publisher connect");
    println!("publisher connected");

    let pub_track = TrackAddress {
        broadcast_path: broadcast_path.clone(),
        track_name: track_name.clone(),
    };

    // Subscriber
    let sub_relay = NetworkRelay::new(relay_url).expect("subscriber relay");
    sub_relay.connect().expect("subscriber connect");
    println!("subscriber connected");

    let sub_track = TrackAddress {
        broadcast_path: broadcast_path.clone(),
        track_name: track_name.clone(),
    };

    // Publish a few frames first so the broadcast exists
    for i in 0u64..3 {
        let frame = MediaFrame {
            seq: i,
            timestamp_us: i * 20_000,
            keyframe: true,
            payload: format!("warmup-{i}").into_bytes(),
        };
        pub_relay
            .publish(&pub_track, frame)
            .expect("publish warmup");
    }

    // Give time for broadcast to propagate
    std::thread::sleep(Duration::from_millis(1000));

    let rx = sub_relay.subscribe(&sub_track).expect("subscribe");
    println!("subscribed, waiting for subscription to propagate...");

    // Wait for subscription propagation
    std::thread::sleep(Duration::from_millis(2000));

    // Publish frames on a background thread while collecting on the main thread.
    let total = 20u64;
    let pub_relay_clone = pub_relay.clone();
    let pub_track_clone = pub_track.clone();
    let publish_handle = std::thread::spawn(move || {
        for i in 0..total {
            let frame = MediaFrame {
                seq: 100 + i,
                timestamp_us: (100 + i) * 20_000,
                keyframe: true,
                payload: format!("frame-{i}").into_bytes(),
            };
            pub_relay_clone
                .publish(&pub_track_clone, frame)
                .expect("publish");
            std::thread::sleep(Duration::from_millis(50));
        }
        println!("publishing done ({total} frames sent)");
    });

    // Collect frames as they arrive.
    // Budget: publish takes ~1s, plus extra time for stragglers.
    println!("publishing {total} frames + collecting...");
    let collect_deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut received = 0u32;
    loop {
        let remaining = collect_deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining) {
            Ok(frame) => {
                let text = String::from_utf8_lossy(&frame.payload);
                println!("  rx: seq={} payload={text}", frame.seq);
                received += 1;
                if received >= total as u32 {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                println!("  rx channel disconnected after {received} frames");
                break;
            }
        }
    }
    publish_handle.join().unwrap();

    println!("\nReceived {received}/{total} frames");

    pub_relay.disconnect();
    sub_relay.disconnect();

    // QUIC/relay drops ~10-20% of groups under normal conditions.
    // Require 50% as a floor to catch real regressions (e.g. subscriber stalling).
    let min_expected = total / 2;
    if received >= min_expected as u32 {
        println!("PASS: NetworkRelay pub/sub works ({received}/{total} frames received)");
    } else {
        println!("FAIL: Only {received}/{total} frames received (need at least {min_expected})");
        std::process::exit(1);
    }
}
