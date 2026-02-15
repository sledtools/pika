use std::time::{Duration, Instant};

use nostr_sdk::prelude::*;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let mut args = std::env::args().skip(1);
    let relay = args.next().ok_or_else(|| {
        anyhow::anyhow!("usage: relay_probe <relay_url> [--kind N] [--unprotected]")
    })?;
    let relay_url = RelayUrl::parse(&relay)?;

    let mut kind: Kind = Kind::TextNote;
    let mut protected = true;
    let mut quick = false;
    while let Some(a) = args.next() {
        if a == "--unprotected" {
            protected = false;
            continue;
        }
        if a == "--quick" {
            quick = true;
            continue;
        }
        if a == "--kind" {
            let n = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("--kind requires a number"))?;
            let v: u16 = n.parse()?;
            kind = Kind::Custom(v);
            continue;
        }
        return Err(anyhow::anyhow!("unknown arg: {a}"));
    }

    let keys = Keys::generate();
    let client = Client::new(keys.clone());

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let connect_timeout = if quick {
            Duration::from_secs(4)
        } else {
            Duration::from_secs(8)
        };
        let send_timeout = if quick {
            Duration::from_secs(4)
        } else {
            Duration::from_secs(8)
        };
        let auth_wait = if quick {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(3)
        };
        let tail_wait = if quick {
            Duration::from_secs(0)
        } else {
            Duration::from_secs(8)
        };

        // Defensive timeouts: some relays can hang during connect or publish.
        tokio::time::timeout(connect_timeout, client.add_relay(relay_url.clone()))
            .await
            .map_err(|_| anyhow::anyhow!("timeout adding relay"))??;
        client.connect().await;
        // Ensure we don't immediately publish before the websocket handshake completes.
        client.wait_for_connection(connect_timeout).await;

        // Listen for a short period for relay messages (AUTH, NOTICE, OK, etc).
        let mut rx = client.notifications();

        // By default try sending a protected event (NIP-70) to see whether the relay accepts it.
        // Use `--unprotected` to verify whether a relay blocks only the protected tag or also blocks
        // the event kind.
        let mut builder =
            EventBuilder::new(kind, "pika relay_probe").custom_created_at(Timestamp::now());
        if protected {
            builder = builder.tags([Tag::protected()]);
        }
        let event = client.sign_event_builder(builder).await?;

        // Some relays require NIP-42 AUTH before accepting protected events (NIP-70).
        // Try publish, handle AUTH if requested, then retry once.
        for attempt in 1..=2 {
            match tokio::time::timeout(
                send_timeout,
                client.send_event_to([relay_url.clone()], &event),
            )
            .await
            {
                Ok(Ok(out)) => {
                    eprintln!(
                        "send_event_to(attempt={attempt}): success={} failed={}",
                        out.success.len(),
                        out.failed.len()
                    );
                    if !out.failed.is_empty() {
                        eprintln!("failed: {:#?}", out.failed);
                    }
                    if !out.success.is_empty() {
                        break;
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("send_event_to(attempt={attempt}): error={e:#}");
                }
                Err(_) => {
                    eprintln!("send_event_to(attempt={attempt}): timeout");
                }
            }

            // Give the relay a chance to ask for AUTH and handle it.
            let auth_deadline = Instant::now() + auth_wait;
            while Instant::now() < auth_deadline {
                match tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
                    Ok(Ok(RelayPoolNotification::Message { relay_url, message })) => {
                        eprintln!("msg from {relay_url}: {message:?}");
                        if let RelayMessage::Auth { challenge } = message {
                            let auth = client
                                .sign_event_builder(EventBuilder::auth(
                                    challenge,
                                    relay_url.clone(),
                                ))
                                .await?;
                            let _ = client
                                .send_msg_to([relay_url.clone()], ClientMessage::auth(auth))
                                .await;
                        }
                    }
                    Ok(Ok(n)) => {
                        // Print other notifications too.
                        match &n {
                            RelayPoolNotification::Event {
                                relay_url,
                                subscription_id,
                                event,
                            } => {
                                eprintln!(
                                    "event from {relay_url} sub={subscription_id}: kind={}",
                                    event.kind
                                );
                            }
                            RelayPoolNotification::Shutdown => break,
                            RelayPoolNotification::Message { .. } => {}
                        }
                    }
                    Ok(Err(_)) => break,
                    Err(_) => {}
                }
            }
        }

        if tail_wait.is_zero() {
            client.shutdown().await;
            return Ok::<(), anyhow::Error>(());
        }

        let start = Instant::now();
        while start.elapsed() < tail_wait {
            match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
                Ok(Ok(n)) => match &n {
                    RelayPoolNotification::Message { relay_url, message } => {
                        eprintln!("msg from {relay_url}: {message:?}");
                    }
                    RelayPoolNotification::Event {
                        relay_url,
                        subscription_id,
                        event,
                    } => {
                        eprintln!(
                            "event from {relay_url} sub={subscription_id}: kind={}",
                            event.kind
                        );
                    }
                    RelayPoolNotification::Shutdown => {
                        eprintln!("shutdown");
                        break;
                    }
                },
                Ok(Err(_)) => break,
                Err(_) => {}
            }
        }

        client.shutdown().await;
        Ok::<(), anyhow::Error>(())
    })?;

    Ok(())
}
