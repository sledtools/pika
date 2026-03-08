#![allow(dead_code)]

use std::collections::HashSet;
use std::io::Write;
use std::time::Duration;

use anyhow::Context;
use mdk_core::prelude::*;
use nostr_sdk::JsonUtil;
use nostr_sdk::prelude::*;
use pika_marmot_runtime::welcome::create_group_and_publish_welcomes as create_group_and_publish_shared_welcomes;
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
        match relay_util::fetch_latest_key_package_for_mdk(
            client,
            &bot_pubkey,
            relays,
            plan.fetch_timeout,
        )
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

    let created = create_group_and_publish_welcomes_with_publisher(
        keys,
        mdk,
        relays,
        bot_key_package,
        bot_pubkey,
        plan,
        |_, giftwrap| {
            let client = client.clone();
            let relays = relays.to_vec();
            async move {
                relay_util::publish_and_confirm(
                    &client,
                    &relays,
                    &giftwrap,
                    plan.welcome_publish_label,
                )
                .await
            }
        },
    )
    .await?;

    eprintln!(" done");
    Ok(created)
}

async fn create_group_and_publish_welcomes_with_publisher<F, Fut>(
    keys: &Keys,
    mdk: &mdk_util::PikaMdk,
    relays: &[RelayUrl],
    bot_key_package: Event,
    bot_pubkey: PublicKey,
    plan: GroupCreatePlan,
    publish_giftwrap: F,
) -> anyhow::Result<CreatedChatGroup>
where
    F: FnMut(PublicKey, Event) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let config = NostrGroupConfigData::new(
        "Agent Chat".to_string(),
        String::new(),
        None,
        None,
        None,
        relays.to_vec(),
        vec![keys.public_key(), bot_pubkey],
    );
    let created = match create_group_and_publish_shared_welcomes(
        keys,
        mdk,
        vec![bot_key_package],
        config,
        &[bot_pubkey],
        vec![],
        publish_giftwrap,
    )
    .await
    {
        Ok(created) => created,
        Err(err) => return Err(map_group_create_error(err, plan)),
    };

    // Agent create mirrors CLI invite semantics: create locally, then wait for
    // welcome delivery, with no extra subscribe/merge workflow at this layer.
    Ok(CreatedChatGroup {
        mls_group_id: created.group.mls_group_id.clone(),
        nostr_group_id_hex: hex::encode(created.group.nostr_group_id),
        published_welcomes: created
            .published_welcomes
            .into_iter()
            .map(|welcome| PublishedWelcome {
                wrapper_event_id_hex: welcome.wrapper_event_id.to_hex(),
                rumor_json: welcome.rumor.as_json(),
            })
            .collect(),
    })
}

fn map_group_create_error(err: anyhow::Error, plan: GroupCreatePlan) -> anyhow::Error {
    if err
        .chain()
        .any(|cause| cause.to_string().contains("build welcome giftwrap"))
    {
        err.context(plan.build_welcome_context)
    } else if err
        .chain()
        .any(|cause| cause.to_string().contains("create group"))
    {
        err.context(plan.create_group_context)
    } else {
        err
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key_package_event(mdk: &mdk_util::PikaMdk, keys: &Keys) -> Event {
        let relay = RelayUrl::parse("wss://test.relay").expect("relay url");
        let (content, tags, _hash_ref) = mdk
            .create_key_package_for_event(&keys.public_key(), vec![relay])
            .expect("create key package");
        EventBuilder::new(Kind::MlsKeyPackage, content)
            .tags(tags)
            .sign_with_keys(keys)
            .expect("sign key package")
    }

    fn group_create_plan() -> GroupCreatePlan {
        GroupCreatePlan {
            progress_message: "",
            create_group_context: "agent create group",
            build_welcome_context: "agent build welcome",
            welcome_publish_label: "agent_welcome",
        }
    }

    #[tokio::test]
    async fn agent_create_group_uses_shared_runtime_helper() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let bot_dir = tempfile::tempdir().expect("bot tempdir");
        let inviter_keys = Keys::generate();
        let bot_keys = Keys::generate();
        let inviter_mdk = mdk_util::open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let bot_mdk = mdk_util::open_mdk(bot_dir.path()).expect("open bot mdk");
        let bot_kp = make_key_package_event(&bot_mdk, &bot_keys);
        let relays = vec![RelayUrl::parse("wss://test.relay").expect("relay url")];
        let published = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Event>::new()));
        let published_capture = std::sync::Arc::clone(&published);

        let created = create_group_and_publish_welcomes_with_publisher(
            &inviter_keys,
            &inviter_mdk,
            &relays,
            bot_kp,
            bot_keys.public_key(),
            group_create_plan(),
            move |_receiver, giftwrap| {
                let published_capture = std::sync::Arc::clone(&published_capture);
                async move {
                    published_capture
                        .lock()
                        .expect("published lock")
                        .push(giftwrap);
                    Ok(())
                }
            },
        )
        .await
        .expect("agent create group");

        assert_eq!(created.published_welcomes.len(), 1);
        assert_eq!(
            created.published_welcomes[0].wrapper_event_id_hex,
            published.lock().expect("published lock")[0].id.to_hex()
        );
        assert!(
            created.published_welcomes[0]
                .rumor_json
                .contains("\"kind\":444"),
            "agent helper should still surface the welcome rumor json"
        );
        assert!(!created.nostr_group_id_hex.is_empty());
    }
}
