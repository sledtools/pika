use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, anyhow};
use mdk_core::prelude::*;
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::warn;

const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum InCmd {
    PublishKeypackage {
        #[serde(default)]
        request_id: Option<String>,
        #[serde(default)]
        relays: Vec<String>,
    },
    SetRelays {
        #[serde(default)]
        request_id: Option<String>,
        relays: Vec<String>,
    },
    ListPendingWelcomes {
        #[serde(default)]
        request_id: Option<String>,
    },
    AcceptWelcome {
        #[serde(default)]
        request_id: Option<String>,
        wrapper_event_id: String,
    },
    ListGroups {
        #[serde(default)]
        request_id: Option<String>,
    },
    SendMessage {
        #[serde(default)]
        request_id: Option<String>,
        nostr_group_id: String,
        content: String,
    },
    Shutdown {
        #[serde(default)]
        request_id: Option<String>,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutMsg {
    Ready {
        protocol_version: u32,
        pubkey: String,
        npub: String,
    },
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
    },
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        code: String,
        message: String,
    },
    KeypackagePublished {
        event_id: String,
    },
    WelcomeReceived {
        wrapper_event_id: String,
        welcome_event_id: String,
        from_pubkey: String,
        nostr_group_id: String,
        group_name: String,
    },
    GroupJoined {
        nostr_group_id: String,
        mls_group_id: String,
    },
    MessageReceived {
        nostr_group_id: String,
        from_pubkey: String,
        content: String,
        created_at: u64,
        message_id: String,
    },
}

fn out_error(request_id: Option<String>, code: &str, message: impl Into<String>) -> OutMsg {
    OutMsg::Error {
        request_id,
        code: code.to_string(),
        message: message.into(),
    }
}

fn out_ok(request_id: Option<String>, result: Option<serde_json::Value>) -> OutMsg {
    OutMsg::Ok { request_id, result }
}

async fn publish_and_confirm_multi(
    client: &Client,
    relays: &[RelayUrl],
    event: &Event,
    label: &str,
) -> anyhow::Result<RelayUrl> {
    let out = client
        .send_event_to(relays.to_vec(), event)
        .await
        .with_context(|| format!("send_event_to failed ({label})"))?;
    if out.success.is_empty() {
        return Err(anyhow!(
            "event publish had no successful relays ({label}): {out:?}"
        ));
    }

    // Confirm we can fetch it back from at least one relay that reported success.
    for relay_url in out.success.iter().cloned() {
        let fetched = client
            .fetch_events_from(
                [relay_url.clone()],
                Filter::new().id(event.id),
                Duration::from_secs(5),
            )
            .await
            .with_context(|| format!("fetch_events_from failed ({label}) relay={relay_url}"))?;
        if fetched.iter().any(|e| e.id == event.id) {
            return Ok(relay_url);
        }
    }

    Err(anyhow!(
        "published event not found on any successful relay after send ({label}) id={}",
        event.id
    ))
}

async fn stdout_writer(mut rx: mpsc::UnboundedReceiver<OutMsg>) -> anyhow::Result<()> {
    let mut stdout = tokio::io::stdout();
    while let Some(msg) = rx.recv().await {
        let line = serde_json::to_string(&msg).context("encode out msg")?;
        stdout.write_all(line.as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }
    Ok(())
}

fn parse_relay_list(relay: &str, relays_override: &[String]) -> anyhow::Result<Vec<RelayUrl>> {
    let mut out = Vec::new();
    if relays_override.is_empty() {
        out.push(RelayUrl::parse(relay).context("parse relay url")?);
        return Ok(out);
    }
    for r in relays_override {
        let trimmed = r.trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push(RelayUrl::parse(trimmed).with_context(|| format!("parse relay url: {trimmed}"))?);
    }
    if out.is_empty() {
        return Err(anyhow!("relays list is empty"));
    }
    Ok(out)
}

fn event_h_tag_hex(ev: &Event) -> Option<String> {
    for t in ev.tags.iter() {
        if t.kind() == TagKind::h()
            && let Some(v) = t.content()
            && !v.is_empty()
        {
            return Some(v.to_string());
        }
    }
    None
}

pub async fn daemon_main(
    relays_arg: &[String],
    state_dir: &Path,
    giftwrap_lookback_sec: u64,
    allow_pubkeys: &[String],
) -> anyhow::Result<()> {
    crate::ensure_dir(state_dir).context("create state dir")?;

    // Use the first relay for initial connectivity check; all relays are added to the client below.
    let primary_relay = relays_arg.first().map(|s| s.as_str()).unwrap_or("ws://127.0.0.1:18080");
    crate::check_relay_ready(primary_relay, Duration::from_secs(90))
        .await
        .with_context(|| format!("relay readiness check failed for {primary_relay}"))?;

    let keys = crate::load_or_create_keys(&state_dir.join("identity.json"))?;
    let pubkey_hex = keys.public_key().to_hex().to_lowercase();
    let npub = keys
        .public_key()
        .to_bech32()
        .unwrap_or_else(|_| "<npub_err>".to_string());

    let (out_tx, out_rx) = mpsc::unbounded_channel::<OutMsg>();
    tokio::spawn(async move {
        if let Err(err) = stdout_writer(out_rx).await {
            eprintln!("[marmotd] stdout writer failed: {err:#}");
        }
    });

    // Build pubkey allowlist. Empty = open (allow all).
    let allowlist: HashSet<String> = allow_pubkeys
        .iter()
        .map(|pk| pk.trim().to_lowercase())
        .filter(|pk| !pk.is_empty())
        .collect();
    let is_open = allowlist.is_empty();
    if is_open {
        eprintln!(
            "[marmotd] WARNING: no --allow-pubkey specified, accepting all senders (open mode)"
        );
    } else {
        eprintln!("[marmotd] allowlist: {} pubkeys", allowlist.len());
        for pk in &allowlist {
            eprintln!("[marmotd]   allow: {pk}");
        }
    }
    let sender_allowed = |pubkey_hex: &str| -> bool {
        is_open || allowlist.contains(&pubkey_hex.trim().to_lowercase())
    };

    out_tx
        .send(OutMsg::Ready {
            protocol_version: PROTOCOL_VERSION,
            pubkey: pubkey_hex.clone(),
            npub,
        })
        .ok();

    let mut relay_urls: Vec<RelayUrl> = Vec::new();
    for r in relays_arg {
        relay_urls.push(RelayUrl::parse(r.trim()).with_context(|| format!("parse relay url: {r}"))?);
    }
    if relay_urls.is_empty() {
        relay_urls.push(RelayUrl::parse("ws://127.0.0.1:18080").context("parse default relay url")?);
    }
    // Connect to the primary relay first, then add the rest.
    let client = crate::connect_client(&keys, primary_relay).await?;
    for r in relay_urls.iter().skip(1) {
        let _ = client.add_relay(r.clone()).await;
    }
    client.connect().await;
    let mdk = crate::new_mdk(state_dir, "daemon")?;

    let mut rx = client.notifications();

    // Subscribe to welcomes (GiftWrap kind 1059) addressed to us.
    // NOTE: `pubkey()` filter matches the event author, not the recipient.
    // GiftWraps can be authored by anyone, so we must filter by the recipient `p` tag.
    let since = Timestamp::now() - Duration::from_secs(giftwrap_lookback_sec);
    let gift_filter = Filter::new()
        .kind(Kind::GiftWrap)
        .custom_tag(SingleLetterTag::lowercase(Alphabet::P), pubkey_hex.clone())
        .since(since)
        .limit(200);
    let gift_sub = client.subscribe(gift_filter, None).await?;

    // Track which wrapper events and group message wrapper events we've already processed.
    let mut seen_welcomes: HashSet<EventId> = HashSet::new();
    let mut seen_group_events: HashSet<EventId> = HashSet::new();

    // Track group subscriptions.
    let mut group_subs: HashMap<SubscriptionId, String> = HashMap::new();

    // On startup, subscribe to any groups already present in state, so the daemon is restart-safe.
    if let Ok(groups) = mdk.get_groups() {
        for g in groups.iter() {
            let nostr_group_id_hex = hex::encode(g.nostr_group_id);
            match crate::subscribe_group_msgs(&client, &nostr_group_id_hex).await {
                Ok(sid) => {
                    group_subs.insert(sid.clone(), nostr_group_id_hex.clone());
                }
                Err(err) => {
                    warn!(
                        "[marmotd] subscribe existing group failed nostr_group_id={nostr_group_id_hex} err={err:#}"
                    );
                }
            }
        }
    }

    // stdin command reader
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<InCmd>();
    tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut lines = tokio::io::BufReader::new(stdin).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<InCmd>(trimmed) {
                Ok(cmd) => {
                    cmd_tx.send(cmd).ok();
                }
                Err(err) => {
                    eprintln!("[marmotd] invalid cmd json: {err} line={trimmed}");
                }
            }
        }
    });

    let mut shutdown = false;
    while !shutdown {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { break; };
                match cmd {
                    InCmd::PublishKeypackage { request_id, relays } => {
                        let selected = match parse_relay_list(primary_relay, &relays) {
                            Ok(v) => v,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "bad_relays", e.to_string())).ok();
                                continue;
                            }
                        };
                        relay_urls = selected.clone();
                        // Ensure client knows about relays.
                        for r in selected.iter() {
                            let _ = client.add_relay(r.clone()).await;
                        }
                        client.connect().await;

                        let (kp_content, kp_tags) = match mdk
                            .create_key_package_for_event(&keys.public_key(), selected.clone())
                        {
                            Ok(v) => v,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "mdk_error", format!("{e:#}"))).ok();
                                continue;
                            }
                        };
                        // Many public relays reject NIP-70 "protected" events. Keypackages and MLS
                        // wrapper events are safe to publish without protection, so strip it to keep
                        // public-relay deployments working.
                        let kp_tags: Tags = kp_tags
                            .into_iter()
                            .filter(|t| !matches!(t.kind(), TagKind::Protected))
                            .collect();
                        let ev = match EventBuilder::new(Kind::MlsKeyPackage, kp_content)
                            .tags(kp_tags)
                            .sign_with_keys(&keys)
                        {
                            Ok(v) => v,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "sign_failed", format!("{e:#}"))).ok();
                                continue;
                            }
                        };

                        match publish_and_confirm_multi(&client, &selected, &ev, "keypackage").await {
                            Ok(_relay_confirmed) => {
                                out_tx.send(out_ok(request_id, Some(json!({"event_id": ev.id.to_hex()})))).ok();
                                out_tx.send(OutMsg::KeypackagePublished { event_id: ev.id.to_hex() }).ok();
                            }
                            Err(e) => {
                                out_tx.send(out_error(request_id, "publish_failed", format!("{e:#}"))).ok();
                            }
                        };
                    }
                    InCmd::SetRelays { request_id, relays } => {
                        match parse_relay_list(primary_relay, &relays) {
                            Ok(v) => {
                                relay_urls = v.clone();
                                for r in v.iter() {
                                    let _ = client.add_relay(r.clone()).await;
                                }
                                client.connect().await;
                                out_tx.send(out_ok(request_id, Some(json!({"relays": v.iter().map(|r| r.to_string()).collect::<Vec<_>>()})))).ok();
                            }
                            Err(e) => {
                                out_tx.send(out_error(request_id, "bad_relays", e.to_string())).ok();
                            }
                        }
                    }
                    InCmd::ListPendingWelcomes { request_id } => {
                        match mdk.get_pending_welcomes(None) {
                            Ok(list) => {
                                let out = list
                                    .iter()
                                    .map(|w| {
                                    json!({
                                        "wrapper_event_id": w.wrapper_event_id.to_hex(),
                                        "welcome_event_id": w.id.to_hex(),
                                        "from_pubkey": w.welcomer.to_hex().to_lowercase(),
                                        "nostr_group_id": hex::encode(w.nostr_group_id),
                                        "group_name": w.group_name,
                                    })
                                    })
                                    .collect::<Vec<_>>();
                                let _ = out_tx.send(out_ok(request_id, Some(json!({ "welcomes": out }))));
                            }
                            Err(e) => {
                                let _ = out_tx.send(out_error(request_id, "mdk_error", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::AcceptWelcome { request_id, wrapper_event_id } => {
                        let wrapper = match EventId::from_hex(&wrapper_event_id) {
                            Ok(id) => id,
                            Err(_) => {
                                out_tx.send(out_error(request_id, "bad_event_id", "wrapper_event_id must be hex")).ok();
                                continue;
                            }
                        };
                        match mdk.get_pending_welcomes(None) {
                            Ok(list) => {
                                let found = list.into_iter().find(|w| w.wrapper_event_id == wrapper);
                                let Some(w) = found else {
                                    out_tx.send(out_error(request_id, "not_found", "pending welcome not found")).ok();
                                    continue;
                                };
                                let nostr_group_id_hex = hex::encode(w.nostr_group_id);
                                let mls_group_id_hex = hex::encode(w.mls_group_id.as_slice());
                                match mdk.accept_welcome(&w) {
                                    Ok(_) => {
                                        // Subscribe to group messages for this group.
                                        match crate::subscribe_group_msgs(&client, &nostr_group_id_hex).await {
                                            Ok(sid) => {
                                                group_subs.insert(sid.clone(), nostr_group_id_hex.clone());
                                            }
                                            Err(err) => {
                                                warn!("[marmotd] subscribe group msgs failed: {err:#}");
                                            }
                                        }

                                        // Backfill recent group messages, but dedupe by wrapper id.
                                        if let Some(relay0) = relay_urls.first().cloned() {
                                            let filter = Filter::new()
                                                .kind(Kind::MlsGroupMessage)
                                                .custom_tag(SingleLetterTag::lowercase(Alphabet::H), &nostr_group_id_hex)
                                                .since(Timestamp::now() - Duration::from_secs(60 * 60))
                                                .limit(200);
                                            if let Ok(events) = client.fetch_events_from([relay0], filter, Duration::from_secs(10)).await {
                                                for ev in events.iter() {
                                                    if !seen_group_events.insert(ev.id) {
                                                        continue;
                                                    }
                                                    if let Ok(MessageProcessingResult::ApplicationMessage(msg)) = mdk.process_message(ev) {
                                                        if !sender_allowed(&msg.pubkey.to_hex()) {
                                                            continue;
                                                        }
                                                        out_tx.send(OutMsg::MessageReceived{
                                                            nostr_group_id: event_h_tag_hex(ev).unwrap_or_else(|| nostr_group_id_hex.clone()),
                                                            from_pubkey: msg.pubkey.to_hex().to_lowercase(),
                                                            content: msg.content,
                                                            created_at: msg.created_at.as_secs(),
                                                            message_id: msg.id.to_hex(),
                                                        }).ok();
                                                    }
                                                }
                                            }
                                        }

                                        out_tx.send(out_ok(request_id, Some(json!({
                                            "nostr_group_id": nostr_group_id_hex,
                                            "mls_group_id": mls_group_id_hex,
                                        })))).ok();
                                        out_tx.send(OutMsg::GroupJoined { nostr_group_id: nostr_group_id_hex, mls_group_id: mls_group_id_hex }).ok();
                                    }
                                    Err(e) => {
                                        out_tx.send(out_error(request_id, "mdk_error", format!("{e:#}"))).ok();
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = out_tx.send(out_error(request_id, "mdk_error", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::ListGroups { request_id } => {
                        match mdk.get_groups() {
                            Ok(gs) => {
                                let out = gs.iter().map(|g| {
                                    json!({
                                        "nostr_group_id": hex::encode(g.nostr_group_id),
                                        "mls_group_id": hex::encode(g.mls_group_id.as_slice()),
                                        "name": g.name,
                                        "description": g.description,
                                    })
                                }).collect::<Vec<_>>();
                                let _ = out_tx.send(out_ok(request_id, Some(json!({"groups": out}))));
                            }
                            Err(e) => {
                                let _ = out_tx.send(out_error(request_id, "mdk_error", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::SendMessage { request_id, nostr_group_id, content } => {
                        let group_id_bytes = match hex::decode(&nostr_group_id) {
                            Ok(b) => b,
                            Err(_) => {
                                out_tx.send(out_error(request_id, "bad_group_id", "nostr_group_id must be hex")).ok();
                                continue;
                            }
                        };
                        if group_id_bytes.len() != 32 {
                            out_tx.send(out_error(request_id, "bad_group_id", "nostr_group_id must be 32 bytes hex")).ok();
                            continue;
                        }
                        let groups = mdk.get_groups().context("get_groups")?;
                        let found = groups.iter().find(|g| g.nostr_group_id.as_slice() == group_id_bytes.as_slice());
                        let Some(g) = found else {
                            out_tx.send(out_error(request_id, "not_found", "group not found")).ok();
                            continue;
                        };
                        let mls_group_id = g.mls_group_id.clone();

                        let rumor = EventBuilder::new(Kind::Custom(9), content).build(keys.public_key());
                        let msg_event = match mdk.create_message(&mls_group_id, rumor) {
                            Ok(ev) => ev,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "mdk_error", format!("{e:#}"))).ok();
                                continue;
                            }
                        };
                        let msg_tags: Tags = msg_event
                            .tags
                            .clone()
                            .into_iter()
                            .filter(|t| !matches!(t.kind(), TagKind::Protected))
                            .collect();
                        let msg_event = match EventBuilder::new(msg_event.kind, msg_event.content)
                            .tags(msg_tags)
                            .sign_with_keys(&keys)
                        {
                            Ok(ev) => ev,
                            Err(e) => {
                                out_tx.send(out_error(request_id, "sign_failed", format!("{e:#}"))).ok();
                                continue;
                            }
                        };

                        if relay_urls.is_empty() {
                            out_tx.send(out_error(request_id, "bad_relays", "no relays configured")).ok();
                            continue;
                        }
                        match publish_and_confirm_multi(&client, &relay_urls, &msg_event, "daemon_send").await {
                            Ok(_relay_confirmed) => {
                                let _ = out_tx.send(out_ok(request_id, Some(json!({"event_id": msg_event.id.to_hex()}))));
                            }
                            Err(e) => {
                                let _ = out_tx.send(out_error(request_id, "publish_failed", format!("{e:#}")));
                            }
                        }
                    }
                    InCmd::Shutdown { request_id } => {
                        out_tx.send(out_ok(request_id, None)).ok();
                        shutdown = true;
                    }
                }
            }
            notification = rx.recv() => {
                let notification = match notification {
                    Ok(n) => n,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                };

                let RelayPoolNotification::Event { subscription_id, event, .. } = notification else {
                    continue;
                };
                let event = *event;

                if subscription_id == gift_sub.val {
                    if event.kind != Kind::GiftWrap {
                        continue;
                    }
                    if !seen_welcomes.insert(event.id) {
                        continue;
                    }

                    // Unwrap and stage welcome in the MDK pending welcome store.
                    let unwrapped = match nostr_sdk::nostr::nips::nip59::extract_rumor(&keys, &event).await {
                        Ok(u) => u,
                        Err(e) => {
                            warn!("[marmotd] giftwrap unwrap failed id={} err={e:#}", event.id.to_hex());
                            continue;
                        }
                    };
                    if unwrapped.rumor.kind != Kind::MlsWelcome {
                        continue;
                    }

                    let wrapper_event_id = event.id;
                    let mut rumor = unwrapped.rumor;
                    let from = unwrapped.sender;

                    if !sender_allowed(&from.to_hex()) {
                        warn!("[marmotd] reject welcome (sender not allowed) from={}", from.to_hex());
                        continue;
                    }

                    if let Err(e) = mdk.process_welcome(&wrapper_event_id, &rumor) {
                        warn!("[marmotd] process_welcome failed wrapper_id={} err={e:#}", wrapper_event_id.to_hex());
                        continue;
                    }

                    // Read back the stored welcome record so we can surface group metadata.
                    let pending = match mdk.get_pending_welcomes(None) {
                        Ok(p) => p,
                        Err(e) => {
                            warn!("[marmotd] get_pending_welcomes failed err={e:#}");
                            continue;
                        }
                    };
                    let stored = pending.into_iter().find(|w| w.wrapper_event_id == wrapper_event_id);
                    let (nostr_group_id, group_name) = match stored {
                        Some(w) => (hex::encode(w.nostr_group_id), w.group_name),
                        None => ("".to_string(), "".to_string()),
                    };

                    out_tx.send(OutMsg::WelcomeReceived {
                        wrapper_event_id: wrapper_event_id.to_hex(),
                        welcome_event_id: rumor.id().to_hex(),
                        from_pubkey: from.to_hex().to_lowercase(),
                        nostr_group_id,
                        group_name,
                    }).ok();

                    continue;
                }

                if event.kind == Kind::MlsGroupMessage {
                    // Only process messages for subscriptions we created.
                    if !group_subs.contains_key(&subscription_id) {
                        continue;
                    }
                    if !seen_group_events.insert(event.id) {
                        continue;
                    }

                    let nostr_group_id = event_h_tag_hex(&event).unwrap_or_else(|| group_subs.get(&subscription_id).cloned().unwrap_or_default());
                    match mdk.process_message(&event) {
                        Ok(MessageProcessingResult::ApplicationMessage(msg)) => {
                            if !sender_allowed(&msg.pubkey.to_hex()) {
                                warn!("[marmotd] drop message (sender not allowed) from={}", msg.pubkey.to_hex());
                                continue;
                            }
                            out_tx.send(OutMsg::MessageReceived {
                                nostr_group_id,
                                from_pubkey: msg.pubkey.to_hex().to_lowercase(),
                                content: msg.content,
                                created_at: msg.created_at.as_secs(),
                                message_id: msg.id.to_hex(),
                            }).ok();
                        }
                        Ok(_) => {}
                        Err(e) => {
                            warn!("[marmotd] process_message failed id={} err={e:#}", event.id.to_hex());
                        }
                    }
                }
            }
        }
    }

    // Best-effort cleanup
    let _ = client.unsubscribe(&gift_sub.val).await;
    client.unsubscribe_all().await;
    client.shutdown().await;
    Ok(())
}
