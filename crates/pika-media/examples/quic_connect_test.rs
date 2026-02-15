/// Minimal QUIC connect test. Build for aarch64-apple-ios and run on device
/// to isolate TLS/QUIC failures without the full app.
///
/// Uses the same QUIC/webtransport stack as the app's call runtime.
fn main() {
    eprintln!("[quic-test] starting...");

    let url = "https://us-east.moq.logos.surf/anon";
    eprintln!("[quic-test] connecting to {url}...");

    let relay = match pika_media::network::NetworkRelay::new(url) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[quic-test] FAIL: relay init error: {e:?}");
            return;
        }
    };

    match relay.connect() {
        Ok(()) => eprintln!("[quic-test] PASS: connected"),
        Err(e) => eprintln!("[quic-test] FAIL: connect error: {e:?}"),
    }
}
