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

    let relay_url = "https://moq.justinmoon.com/anon";
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

    // Now publish test frames
    let total = 20u64;
    println!("publishing {total} frames...");
    for i in 0..total {
        let frame = MediaFrame {
            seq: 100 + i,
            timestamp_us: (100 + i) * 20_000,
            keyframe: true,
            payload: format!("frame-{i}").into_bytes(),
        };
        pub_relay.publish(&pub_track, frame).expect("publish");
        std::thread::sleep(Duration::from_millis(50));
    }

    println!("waiting for frames to arrive...");
    std::thread::sleep(Duration::from_secs(2));

    let mut received = 0u32;
    loop {
        match rx.try_recv() {
            Ok(frame) => {
                let text = String::from_utf8_lossy(&frame.payload);
                println!("  rx: seq={} payload={text}", frame.seq);
                received += 1;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
        }
    }

    println!("\nReceived {received} frames");

    pub_relay.disconnect();
    sub_relay.disconnect();

    if received > 0 {
        println!("PASS: NetworkRelay pub/sub works through real MOQ relay");
    } else {
        println!("FAIL: No frames received through NetworkRelay");
        std::process::exit(1);
    }
}
