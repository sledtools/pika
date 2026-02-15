#[cfg(not(feature = "network"))]
fn main() {
    eprintln!("This example requires `--features network`.");
}

#[cfg(feature = "network")]
fn main() {
    use std::time::{Duration, Instant};

    use pika_media::network::NetworkRelay;
    use pika_media::session::MediaFrame;
    use pika_media::tracks::TrackAddress;

    let relays = [
        ("us-east (ash)", "https://us-east.moq.logos.surf/anon"),
        ("us-west (hil)", "https://us-west.moq.logos.surf/anon"),
        ("germany (fsn)", "https://germany.moq.logos.surf/anon"),
        ("singapore (sin)", "https://singapore.moq.logos.surf/anon"),
    ];

    let total = 50u64;
    let runs = 3;

    println!("=== MOQ Relay Benchmark ===");
    println!("  frames per run: {total}  |  publish interval: 20ms  |  runs per relay: {runs}");
    println!();

    let mut results: Vec<(&str, Vec<(u32, Duration)>)> = Vec::new();

    for (name, url) in &relays {
        println!("--- {name} ({url}) ---");
        let mut run_results = Vec::new();

        for run in 1..=runs {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros();
            let broadcast_path = format!("pika/bench/{unique}");
            let track_name = "audio0";

            // Connect publisher
            let pub_relay = match NetworkRelay::new(url) {
                Ok(r) => r,
                Err(e) => {
                    println!("  run {run}: CONNECT FAILED (pub): {e}");
                    run_results.push((0, Duration::ZERO));
                    continue;
                }
            };
            if let Err(e) = pub_relay.connect() {
                println!("  run {run}: CONNECT FAILED (pub): {e}");
                run_results.push((0, Duration::ZERO));
                continue;
            }

            // Connect subscriber
            let sub_relay = match NetworkRelay::new(url) {
                Ok(r) => r,
                Err(e) => {
                    println!("  run {run}: CONNECT FAILED (sub): {e}");
                    run_results.push((0, Duration::ZERO));
                    pub_relay.disconnect();
                    continue;
                }
            };
            if let Err(e) = sub_relay.connect() {
                println!("  run {run}: CONNECT FAILED (sub): {e}");
                run_results.push((0, Duration::ZERO));
                pub_relay.disconnect();
                continue;
            }

            let pub_track = TrackAddress {
                broadcast_path: broadcast_path.clone(),
                track_name: track_name.to_string(),
            };
            let sub_track = pub_track.clone();

            // Warmup
            for i in 0..3u64 {
                let frame = MediaFrame {
                    seq: i,
                    timestamp_us: 0,
                    keyframe: true,
                    payload: vec![0u8; 10],
                };
                let _ = pub_relay.publish(&pub_track, frame);
            }
            std::thread::sleep(Duration::from_millis(500));

            let rx = match sub_relay.subscribe(&sub_track) {
                Ok(r) => r,
                Err(e) => {
                    println!("  run {run}: SUBSCRIBE FAILED: {e}");
                    run_results.push((0, Duration::ZERO));
                    pub_relay.disconnect();
                    sub_relay.disconnect();
                    continue;
                }
            };
            std::thread::sleep(Duration::from_secs(1));

            // Publish on background thread at 20ms (real audio cadence)
            let pub_relay_bg = pub_relay.clone();
            let pub_track_bg = pub_track.clone();
            let publish_handle = std::thread::spawn(move || {
                for i in 0..total {
                    let frame = MediaFrame {
                        seq: 100 + i,
                        timestamp_us: (100 + i) * 20_000,
                        keyframe: true,
                        payload: vec![i as u8; 160],
                    };
                    let _ = pub_relay_bg.publish(&pub_track_bg, frame);
                    std::thread::sleep(Duration::from_millis(20));
                }
            });

            // Collect
            let t0 = Instant::now();
            let deadline = t0 + Duration::from_secs(5);
            let mut received = 0u32;
            let mut first_frame_latency = None;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match rx.recv_timeout(remaining) {
                    Ok(_frame) => {
                        received += 1;
                        if first_frame_latency.is_none() {
                            first_frame_latency = Some(t0.elapsed());
                        }
                        if received >= total as u32 {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            let elapsed = t0.elapsed();
            publish_handle.join().unwrap();

            let pct = (received as f64 / total as f64) * 100.0;
            let latency_str = first_frame_latency
                .map(|d| format!("{:.0}ms", d.as_secs_f64() * 1000.0))
                .unwrap_or_else(|| "n/a".to_string());
            println!(
                "  run {run}: {received}/{total} ({pct:.0}%)  first_frame={latency_str}  elapsed={:.1}s",
                elapsed.as_secs_f64()
            );

            run_results.push((received, elapsed));
            pub_relay.disconnect();
            sub_relay.disconnect();
            std::thread::sleep(Duration::from_millis(500));
        }

        results.push((name, run_results));
        println!();
    }

    // Summary table
    println!("=== Summary ===");
    println!(
        "{:<22} {:>8} {:>8} {:>8} {:>8}",
        "Relay", "Run1", "Run2", "Run3", "Avg%"
    );
    println!("{}", "-".repeat(60));
    for (name, runs_data) in &results {
        let counts: Vec<String> = runs_data
            .iter()
            .map(|(r, _)| format!("{r}/{total}"))
            .collect();
        let avg_pct = if runs_data.is_empty() {
            0.0
        } else {
            let sum: u32 = runs_data.iter().map(|(r, _)| *r).sum();
            (sum as f64 / (runs_data.len() as f64 * total as f64)) * 100.0
        };
        let c1 = counts.first().map(|s| s.as_str()).unwrap_or("--");
        let c2 = counts.get(1).map(|s| s.as_str()).unwrap_or("--");
        let c3 = counts.get(2).map(|s| s.as_str()).unwrap_or("--");
        println!(
            "{:<22} {:>8} {:>8} {:>8} {:>7.0}%",
            name, c1, c2, c3, avg_pct
        );
    }
}
