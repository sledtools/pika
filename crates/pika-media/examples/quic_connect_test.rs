/// Minimal QUIC connect test. Build for aarch64-apple-ios and run on device
/// to isolate TLS/QUIC failures without the full app.
///
/// Build:
///   just ios-rust-example quic_connect_test
/// (or manually with the right SDK flags)
use std::time::Duration;

fn main() {
    eprintln!("[quic-test] starting...");

    eprintln!("[quic-test] installing crypto provider...");
    let _ = moq_native::rustls::crypto::aws_lc_rs::default_provider().install_default();
    eprintln!("[quic-test] crypto provider OK");

    let rt = tokio::runtime::Runtime::new().unwrap();
    eprintln!("[quic-test] tokio runtime OK");

    rt.block_on(async {
        let url = url::Url::parse("https://moq.justinmoon.com/anon").unwrap();

        // Test 1: default config (uses rustls_native_certs)
        eprintln!("[quic-test] === Test 1: default TLS (native certs) ===");
        {
            let client_config = moq_native::ClientConfig::default();
            match client_config.init() {
                Ok(client) => {
                    eprintln!("[quic-test] client created, connecting to {url}...");
                    let origin = moq_lite::Origin::produce();
                    match tokio::time::timeout(
                        Duration::from_secs(10),
                        client.with_publish(origin.consume()).connect(url.clone()),
                    )
                    .await
                    {
                        Ok(Ok(_session)) => {
                            eprintln!("[quic-test] PASS: connected with native certs")
                        }
                        Ok(Err(e)) => eprintln!("[quic-test] FAIL: connect error: {e:#}"),
                        Err(_) => eprintln!("[quic-test] FAIL: connect timed out (10s)"),
                    }
                }
                Err(e) => eprintln!("[quic-test] FAIL: client init error: {e:#}"),
            }
        }

        // Test 2: disable TLS verify
        eprintln!("[quic-test] === Test 2: TLS verify disabled ===");
        {
            let mut client_config = moq_native::ClientConfig::default();
            client_config.tls.disable_verify = Some(true);
            match client_config.init() {
                Ok(client) => {
                    eprintln!("[quic-test] client created (no-verify), connecting to {url}...");
                    let origin = moq_lite::Origin::produce();
                    match tokio::time::timeout(
                        Duration::from_secs(10),
                        client.with_publish(origin.consume()).connect(url.clone()),
                    )
                    .await
                    {
                        Ok(Ok(_session)) => {
                            eprintln!("[quic-test] PASS: connected with verify disabled")
                        }
                        Ok(Err(e)) => eprintln!("[quic-test] FAIL: connect error: {e:#}"),
                        Err(_) => eprintln!("[quic-test] FAIL: connect timed out (10s)"),
                    }
                }
                Err(e) => eprintln!("[quic-test] FAIL: client init error: {e:#}"),
            }
        }

        eprintln!("[quic-test] done");
    });
}
