use std::collections::HashSet;
use std::io::Write;
use std::time::Duration;

use anyhow::Context;
use mdk_core::prelude::*;
use nostr_sdk::JsonUtil;
use nostr_sdk::prelude::*;
use tokio::io::AsyncBufReadExt;

use pika_agent_protocol::projection::{ProjectedContent, project_message};

use crate::agent::provider::{ChatLoopPlan, GroupCreatePlan, KeyPackageWaitPlan};
use crate::{mdk_util, relay_util};

#[derive(Debug)]
#[allow(dead_code)]
pub struct PublishedWelcome {
    pub wrapper_event_id_hex: String,
    pub rumor_json: String,
}

#[derive(Debug)]
pub struct CreatedChatGroup {
    pub mls_group_id: GroupId,
    pub nostr_group_id_hex: String,
    #[allow(dead_code)]
    pub published_welcomes: Vec<PublishedWelcome>,
}

pub struct ChatLoopContext<'a> {
    pub keys: &'a Keys,
    pub mdk: &'a mdk_util::PikaMdk,
    pub send_client: &'a Client,
    pub listen_client: &'a Client,
    pub relays: &'a [RelayUrl],
    pub bot_pubkey: PublicKey,
    pub mls_group_id: &'a GroupId,
    pub nostr_group_id_hex: &'a str,
    pub plan: ChatLoopPlan,
    pub seen_mls_event_ids: Option<&'a mut HashSet<EventId>>,
}

pub async fn wait_for_latest_key_package(
    client: &Client,
    bot_pubkey: PublicKey,
    relays: &[RelayUrl],
    plan: KeyPackageWaitPlan,
) -> anyhow::Result<Event> {
    eprint!("{}", plan.progress_message);
    std::io::stderr().flush().ok();
    let start = tokio::time::Instant::now();
    loop {
        match relay_util::fetch_latest_key_package(client, &bot_pubkey, relays, plan.fetch_timeout)
            .await
        {
            Ok(kp) => {
                eprintln!(" done");
                return Ok(kp);
            }
            Err(err) => {
                if start.elapsed() >= plan.timeout {
                    anyhow::bail!(
                        "timed out waiting for bot key package after {}s: {err}",
                        plan.timeout.as_secs()
                    );
                }
                eprint!(".");
                std::io::stderr().flush().ok();
                tokio::time::sleep(plan.retry_delay).await;
            }
        }
    }
}

pub async fn create_group_and_publish_welcomes(
    keys: &Keys,
    mdk: &mdk_util::PikaMdk,
    client: &Client,
    relays: &[RelayUrl],
    bot_key_package: Event,
    bot_pubkey: PublicKey,
    plan: GroupCreatePlan,
) -> anyhow::Result<CreatedChatGroup> {
    eprint!("{}", plan.progress_message);
    std::io::stderr().flush().ok();

    let config = NostrGroupConfigData::new(
        "Agent Chat".to_string(),
        String::new(),
        None,
        None,
        None,
        relays.to_vec(),
        vec![keys.public_key(), bot_pubkey],
    );
    let result = mdk
        .create_group(&keys.public_key(), vec![bot_key_package], config)
        .context(plan.create_group_context)?;

    let mls_group_id = result.group.mls_group_id.clone();
    let nostr_group_id_hex = hex::encode(result.group.nostr_group_id);
    let mut published_welcomes = Vec::new();
    for rumor in result.welcome_rumors {
        let rumor_json = rumor.as_json();
        let giftwrap = EventBuilder::gift_wrap(keys, &bot_pubkey, rumor, [])
            .await
            .context(plan.build_welcome_context)?;
        relay_util::publish_and_confirm(client, relays, &giftwrap, plan.welcome_publish_label)
            .await?;
        published_welcomes.push(PublishedWelcome {
            wrapper_event_id_hex: giftwrap.id.to_hex(),
            rumor_json,
        });
    }

    eprintln!(" done");
    Ok(CreatedChatGroup {
        mls_group_id,
        nostr_group_id_hex,
        published_welcomes,
    })
}

pub async fn run_interactive_chat_loop(mut ctx: ChatLoopContext<'_>) -> anyhow::Result<()> {
    let keys = ctx.keys;
    let mdk = ctx.mdk;
    let send_client = ctx.send_client;
    let listen_client = ctx.listen_client;
    let relays = ctx.relays;
    let bot_pubkey = ctx.bot_pubkey;
    let mls_group_id = ctx.mls_group_id;
    let nostr_group_id_hex = ctx.nostr_group_id_hex;
    let plan = ctx.plan;
    let seen_mls_event_ids = &mut ctx.seen_mls_event_ids;

    let group_filter = Filter::new()
        .kind(Kind::MlsGroupMessage)
        .custom_tag(SingleLetterTag::lowercase(Alphabet::H), nostr_group_id_hex)
        .since(Timestamp::now());
    let sub = listen_client.subscribe(group_filter, None).await?;
    let mut rx = listen_client.notifications();

    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut stdin_closed = false;
    let mut pending_replies: usize = 0;
    let mut eof_wait_started: Option<tokio::time::Instant> = None;
    eprint!("you> ");
    std::io::stderr().flush().ok();

    loop {
        tokio::select! {
            line = stdin.next_line(), if !stdin_closed => {
                let Some(line) = line? else {
                    stdin_closed = true;
                    if !plan.wait_for_pending_replies_on_eof {
                        break;
                    }
                    eof_wait_started = Some(tokio::time::Instant::now());
                    if pending_replies == 0 {
                        break;
                    }
                    continue;
                };
                let line = line.trim().to_string();
                if line.is_empty() {
                    eprint!("you> ");
                    std::io::stderr().flush().ok();
                    continue;
                }

                let rumor = EventBuilder::new(Kind::ChatMessage, &line).build(keys.public_key());
                let msg_event = mdk
                    .create_message(mls_group_id, rumor)
                    .context("create user chat message")?;
                relay_util::publish_and_confirm(send_client, relays, &msg_event, plan.outbound_publish_label).await?;
                if plan.wait_for_pending_replies_on_eof {
                    pending_replies = pending_replies.saturating_add(1);
                }
                eprint!("you> ");
                std::io::stderr().flush().ok();
            }
            notification = rx.recv() => {
                let notification = match notification {
                    Ok(n) => n,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                };
                let RelayPoolNotification::Event { subscription_id, event, .. } = notification else { continue };
                if subscription_id != sub.val {
                    continue;
                }
                let event = *event;
                if event.kind != Kind::MlsGroupMessage {
                    continue;
                }
                if let Some(seen) = seen_mls_event_ids.as_deref_mut()
                    && !seen.insert(event.id)
                {
                    continue;
                }
                let mut printed = false;
                if let Ok(MessageProcessingResult::ApplicationMessage(msg)) =
                    mdk.process_message(&event)
                    && msg.pubkey == bot_pubkey
                {
                    match project_message(&msg.content, plan.projection_mode) {
                        ProjectedContent::Text(text) => {
                            printed = true;
                            if plan.wait_for_pending_replies_on_eof {
                                pending_replies = pending_replies.saturating_sub(1);
                            }
                            eprint!("\r");
                            println!("pi> {text}");
                            println!();
                        }
                        ProjectedContent::Status(status) => {
                            eprint!("\r{status}\r");
                            std::io::stderr().flush().ok();
                        }
                        ProjectedContent::Hidden => {}
                    }
                }
                if printed {
                    if !stdin_closed {
                        eprint!("you> ");
                        std::io::stderr().flush().ok();
                    } else if pending_replies == 0 {
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(200)), if stdin_closed && plan.wait_for_pending_replies_on_eof && pending_replies > 0 => {
                if let Some(started) = eof_wait_started
                    && started.elapsed() > plan.eof_reply_timeout
                {
                    anyhow::bail!("timed out waiting for relay reply");
                }
            }
            _ = tokio::signal::ctrl_c() => {
                break;
            }
        }
    }

    Ok(())
}
