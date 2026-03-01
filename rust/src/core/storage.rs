// Storage-derived state refresh + paging.

use super::*;
use crate::state::{
    resolve_mentions, HypernoteResponder, HypernoteResponseTally, MemberInfo, MessageSegment,
};
use hypernote_protocol as hn;
use std::sync::OnceLock;

impl AppCore {
    /// Build a sender pubkey → display name lookup from member info + profile cache,
    /// including the current user's name for mention resolution.
    fn build_sender_names(
        &self,
        members: &[super::GroupMember],
        my_pubkey_hex: &str,
    ) -> HashMap<String, String> {
        let mut names: HashMap<String, String> = members
            .iter()
            .filter_map(|m| {
                let hex = m.pubkey.to_hex();
                let display = m
                    .name
                    .clone()
                    .or_else(|| self.profiles.get(&hex).and_then(|p| p.name.clone()));
                display.map(|n| (hex, n))
            })
            .collect();
        let my_name = &self.state.my_profile.name;
        if !my_name.is_empty() {
            names.insert(my_pubkey_hex.to_string(), my_name.clone());
        }
        names
    }

    /// Build a member profile lookup: pubkey_hex → (name, npub, picture_url),
    /// including the current user.
    fn build_member_profiles(
        &self,
        sess: &super::Session,
        members: &[super::GroupMember],
        my_pubkey_hex: &str,
    ) -> HashMap<String, (Option<String>, String, Option<String>)> {
        let mut profiles: HashMap<String, (Option<String>, String, Option<String>)> = members
            .iter()
            .map(|m| {
                let hex = m.pubkey.to_hex();
                let npub = m.pubkey.to_bech32().unwrap_or_else(|_| hex.clone());
                let name = m
                    .name
                    .clone()
                    .or_else(|| self.profiles.get(&hex).and_then(|p| p.name.clone()));
                let picture_url = m.picture_url.clone();
                (hex, (name, npub, picture_url))
            })
            .collect();
        if !profiles.contains_key(my_pubkey_hex) {
            let my_npub = sess
                .pubkey
                .to_bech32()
                .unwrap_or_else(|_| my_pubkey_hex.to_string());
            let my_pic = self.state.my_profile.picture_url.clone();
            let my_name = &self.state.my_profile.name;
            let name = if my_name.is_empty() {
                None
            } else {
                Some(my_name.clone())
            };
            profiles.insert(my_pubkey_hex.to_string(), (name, my_npub, my_pic));
        }
        profiles
    }

    pub(super) fn refresh_all_from_storage(&mut self) {
        self.refresh_chat_list_from_storage();
        if let Some(Screen::Chat { chat_id }) = self.state.router.screen_stack.last().cloned() {
            self.refresh_current_chat(&chat_id);
        }
        if self.network_enabled() {
            self.recompute_subscriptions();
        }
    }

    pub(super) fn refresh_chat_list_from_storage(&mut self) {
        let Some(sess) = self.session.as_ref() else {
            self.state.chat_list = vec![];
            self.emit_chat_list();
            return;
        };

        let groups = match sess.mdk.get_groups() {
            Ok(gs) => gs,
            Err(e) => {
                self.toast(format!("Storage error: {e}"));
                return;
            }
        };

        let my_pubkey = sess.pubkey;
        let mut index: HashMap<String, GroupIndexEntry> = HashMap::new();
        let mut list: Vec<ChatSummary> = Vec::new();
        let mut missing_profile_pubkeys: HashSet<PublicKey> = HashSet::new();

        for g in groups {
            let chat_id = hex::encode(g.nostr_group_id);

            if self.archived_chats.contains(&chat_id) {
                continue;
            }

            // Get all members except self.
            let all_members: BTreeSet<PublicKey> =
                sess.mdk.get_members(&g.mls_group_id).unwrap_or_default();
            let other_members: Vec<PublicKey> = all_members
                .iter()
                .filter(|p| *p != &my_pubkey)
                .cloned()
                .collect();

            // A group chat is anything with >1 other member, or explicitly named (not "DM") with
            // at least one other member (so "Note to self" doesn't get treated as a group).
            let explicit_name = if g.name != DEFAULT_GROUP_NAME && !g.name.is_empty() {
                Some(g.name.clone())
            } else {
                None
            };
            let is_group =
                other_members.len() > 1 || (explicit_name.is_some() && !other_members.is_empty());

            // Build member info with cached profiles.
            let now = crate::state::now_seconds();
            let mut member_infos: Vec<super::GroupMember> = Vec::new();
            for pk in &other_members {
                let hex = pk.to_hex();
                let cached = self.profiles.get(&hex);
                let name = cached.and_then(|p| p.name.clone());
                let picture_url = cached.and_then(|p| p.display_picture_url(&self.data_dir, &hex));
                member_infos.push(super::GroupMember {
                    pubkey: *pk,
                    name,
                    picture_url,
                });

                let needs_fetch = match cached {
                    None => true,
                    Some(p) => (now - p.last_checked_at) > 3600,
                };
                if needs_fetch {
                    missing_profile_pubkeys.insert(*pk);
                }
            }

            let admin_pubkeys: Vec<String> = g.admin_pubkeys.iter().map(|p| p.to_hex()).collect();

            let members_for_state: Vec<MemberInfo> = member_infos
                .iter()
                .map(|m| m.to_member_info(&admin_pubkeys))
                .collect();

            // Do not rely on `last_message_id` being populated in all MDK flows.
            // For MVP scale, fetching the newest message per group is cheap and robust.
            // Signal/control messages share the MLS app-message path; skip them in chat previews.
            let newest = sess
                .mdk
                .get_messages(&g.mls_group_id, Some(Pagination::new(Some(20), Some(0))))
                .ok()
                .and_then(|v| {
                    v.into_iter()
                        .find(|m| m.kind == Kind::ChatMessage || m.kind == super::HYPERNOTE_KIND)
                });

            let stored_last_message = newest.as_ref().map(|m| m.content.clone());
            let stored_last_message_at = newest
                .as_ref()
                .map(|m| m.created_at.as_secs() as i64)
                .or_else(|| g.last_message_at.map(|t| t.as_secs() as i64));

            let local_last = self.local_outbox.get(&chat_id).and_then(|m| {
                m.values()
                    .max_by(|a, b| {
                        a.timestamp
                            .cmp(&b.timestamp)
                            .then_with(|| a.seq.cmp(&b.seq))
                    })
                    .cloned()
            });
            let local_last_at = local_last.as_ref().map(|m| m.timestamp);

            let (last_message, last_message_at) = match (stored_last_message_at, local_last_at) {
                (Some(a), Some(b)) if b > a => {
                    (local_last.as_ref().map(|m| m.content.clone()), Some(b))
                }
                (None, Some(b)) => (local_last.as_ref().map(|m| m.content.clone()), Some(b)),
                _ => (stored_last_message, stored_last_message_at),
            };

            let unread_count = *self.unread_counts.get(&chat_id).unwrap_or(&0);

            let last_message = last_message.map(|msg: String| {
                if msg.contains("```pika-html-update ") {
                    "Updated content".to_string()
                } else if msg.contains("```pika-html-state-update ") {
                    "Updated widget".to_string()
                } else {
                    msg
                }
            });

            let member_count = members_for_state.len() + 1;
            let (display_name, subtitle) = if is_group {
                let display_name = explicit_name
                    .clone()
                    .unwrap_or_else(|| format!("Group ({member_count})"));
                let subtitle = Some(format!("{member_count} members"));
                (display_name, subtitle)
            } else {
                let peer = members_for_state.first();
                let display_name =
                    peer.and_then(|member| member.name.clone())
                        .unwrap_or_else(|| {
                            peer.map(|member| truncated_npub(&member.npub))
                                .unwrap_or_else(|| "Chat".to_string())
                        });
                let subtitle = if peer.and_then(|member| member.name.as_ref()).is_some() {
                    peer.map(|member| truncated_npub(&member.npub))
                } else {
                    None
                };
                (display_name, subtitle)
            };
            let last_message_preview = match &last_message {
                None => "No messages yet".to_string(),
                Some(msg) if msg.trim().is_empty() => "Media".to_string(),
                Some(msg) => msg.clone(),
            };

            list.push(ChatSummary {
                chat_id: chat_id.clone(),
                is_group,
                group_name: explicit_name.clone(),
                members: members_for_state,
                last_message,
                last_message_at,
                display_name,
                subtitle,
                last_message_preview,
                unread_count,
            });

            index.insert(
                chat_id,
                GroupIndexEntry {
                    mls_group_id: g.mls_group_id,
                    is_group,
                    group_name: explicit_name,
                    members: member_infos,
                    admin_pubkeys,
                },
            );
        }

        list.sort_by_key(|c| std::cmp::Reverse(c.last_message_at.unwrap_or(0)));
        if let Some(sess) = self.session.as_mut() {
            sess.groups = index;
        }
        self.state.chat_list = list;
        self.emit_chat_list();
        self.sync_push_subscriptions();

        // Fetch missing profiles asynchronously.
        if !missing_profile_pubkeys.is_empty() && self.network_enabled() {
            if let Some(sess) = self.session.as_ref() {
                let client = sess.client.clone();
                let tx = self.core_sender.clone();
                let pubkeys = missing_profile_pubkeys;
                self.runtime.spawn(async move {
                    let filter = Filter::new()
                        .authors(pubkeys.clone())
                        .kind(Kind::Metadata)
                        .limit(pubkeys.len());
                    let events = match client.fetch_events(filter, Duration::from_secs(8)).await {
                        Ok(evs) => evs,
                        Err(e) => {
                            tracing::debug!(%e, "profile fetch failed");
                            return;
                        }
                    };

                    // Keep only the newest event per author.
                    let mut best: HashMap<String, Event> = HashMap::new();
                    for ev in events.into_iter().filter(|e| e.verify().is_ok()) {
                        let author_hex = ev.pubkey.to_hex();
                        let is_newer = best
                            .get(&author_hex)
                            .map(|prev| ev.created_at > prev.created_at)
                            .unwrap_or(true);
                        if is_newer {
                            best.insert(author_hex, ev);
                        }
                    }

                    let mut results: Vec<(String, Option<String>, i64)> = Vec::new();
                    for (hex_pk, ev) in best {
                        let event_created_at = ev.created_at.as_secs() as i64;
                        results.push((hex_pk, Some(ev.content.clone()), event_created_at));
                    }

                    // Also record "no profile" for pubkeys with no kind:0 event, so we
                    // don't keep re-fetching them.
                    for pk in &pubkeys {
                        let hex = pk.to_hex();
                        if !results.iter().any(|(h, _, _)| h == &hex) {
                            results.push((hex, None, 0));
                        }
                    }

                    let _ = tx.send(CoreMsg::Internal(Box::new(
                        InternalEvent::ProfilesFetched { profiles: results },
                    )));
                });
            }
        }
    }

    pub(super) fn chat_exists(&self, chat_id: &str) -> bool {
        self.session
            .as_ref()
            .map(|s| s.groups.contains_key(chat_id))
            .unwrap_or(false)
    }

    pub(super) fn refresh_current_chat_if_open(&mut self, chat_id: &str) {
        if self.state.current_chat.as_ref().map(|c| c.chat_id.as_str()) == Some(chat_id) {
            self.refresh_current_chat(chat_id);
        }
    }

    /// Lightweight refresh that only updates typing indicators without re-fetching messages.
    pub(super) fn refresh_typing_if_open(&mut self, chat_id: &str) {
        let is_open = self
            .state
            .current_chat
            .as_ref()
            .map(|c| c.chat_id == chat_id)
            .unwrap_or(false);
        if is_open {
            let typing = self.get_active_typers(chat_id);
            if let Some(cur) = self.state.current_chat.as_mut() {
                cur.typing_members = typing;
            }
            self.emit_current_chat();
        }
    }

    pub(super) fn refresh_current_chat(&mut self, chat_id: &str) {
        let Some(sess) = self.session.as_ref() else {
            self.state.current_chat = None;
            self.emit_current_chat();
            return;
        };
        let Some(entry) = sess.groups.get(chat_id).cloned() else {
            self.state.current_chat = None;
            self.emit_current_chat();
            return;
        };

        let my_pubkey_hex = sess.pubkey.to_hex();

        let sender_names = self.build_sender_names(&entry.members, &my_pubkey_hex);
        let member_profiles = self.build_member_profiles(sess, &entry.members, &my_pubkey_hex);

        let desired = *self.loaded_count.get(chat_id).unwrap_or(&50usize);
        let target = desired.max(50);

        // Fetch in batches until we have enough visible messages (ChatMessage +
        // Reaction).  Stored typing-indicators and other non-visible kinds
        // consume pagination slots, so a single fetch may not return enough.
        let mut visible_messages = Vec::new();
        let mut fetch_offset = 0;
        let mut storage_len = 0;
        loop {
            let batch = sess
                .mdk
                .get_messages(
                    &entry.mls_group_id,
                    Some(Pagination::new(Some(target), Some(fetch_offset))),
                )
                .unwrap_or_default();
            let batch_len = batch.len();
            storage_len += batch_len;
            visible_messages.extend(
                batch
                    .into_iter()
                    .filter(|m| classify_app_message(m).is_some_and(|k| k.is_chat_visible())),
            );
            if batch_len < target || visible_messages.len() >= target {
                break;
            }
            fetch_offset += batch_len;
        }

        let separated = separate_messages(&visible_messages, &sender_names);
        let mut hypernote_responses = separated.hypernote_responses;

        // MDK returns descending by created_at; UI wants ascending.
        let mut msgs: Vec<ChatMessage> = separated
            .regular
            .into_iter()
            .rev()
            .map(|m| {
                let mut cm =
                    build_chat_message(m, &my_pubkey_hex, &sender_names, &separated.reaction_map);
                cm.delivery = self
                    .delivery_overrides
                    .get(chat_id)
                    .and_then(|map| map.get(&cm.id))
                    .cloned()
                    .unwrap_or(MessageDeliveryState::Sent);
                cm.media = self.chat_media_attachments_for_tags(
                    &sess.mdk,
                    &entry.mls_group_id,
                    chat_id,
                    &my_pubkey_hex,
                    &m.tags,
                );
                cm
            })
            .collect();

        let oldest_loaded_ts = msgs.first().map(|m| m.timestamp).unwrap_or(i64::MIN);
        let present_ids: std::collections::HashSet<String> =
            msgs.iter().map(|m| m.id.clone()).collect();
        if let Some(local) = self.local_outbox.get(chat_id).cloned() {
            for (id, lm) in local.into_iter() {
                if present_ids.contains(&id) {
                    continue;
                }
                if lm.timestamp < oldest_loaded_ts {
                    continue;
                }
                if !matches!(
                    lm.kind,
                    Kind::ChatMessage
                        | Kind::Reaction
                        | Kind::Custom(hn::HYPERNOTE_KIND)
                        | Kind::Custom(hn::HYPERNOTE_ACTION_RESPONSE_KIND)
                ) {
                    continue;
                }
                if lm.kind == super::HYPERNOTE_ACTION_RESPONSE_KIND {
                    if let Some(response) = parse_hypernote_response_message(
                        lm.sender_pubkey.clone(),
                        sender_names.get(&lm.sender_pubkey).cloned(),
                        lm.timestamp,
                        lm.reply_to_message_id.clone(),
                        &lm.content,
                    ) {
                        hypernote_responses.push(response);
                    }
                    continue;
                }
                let delivery = self
                    .delivery_overrides
                    .get(chat_id)
                    .and_then(|map| map.get(&id))
                    .cloned()
                    .unwrap_or(MessageDeliveryState::Pending);
                let (display_content, mentions) = resolve_mentions(&lm.content, &sender_names);
                let segments = parse_message_segments(&display_content);
                msgs.push(ChatMessage {
                    id,
                    sender_pubkey: lm.sender_pubkey,
                    sender_name: None,
                    content: lm.content,
                    display_content,
                    reply_to_message_id: lm.reply_to_message_id.clone(),
                    mentions,
                    timestamp: lm.timestamp,
                    display_timestamp: format_display_timestamp(lm.timestamp),
                    is_mine: true,
                    delivery,
                    reactions: vec![],
                    media: lm.media,
                    segments,
                    html_state: None,
                    hypernote: None,
                });
            }
            msgs.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));
        }

        if let Some(local) = self.local_outbox.get_mut(chat_id) {
            local.retain(|id, lm| !present_ids.contains(id) && lm.timestamp >= oldest_loaded_ts);
        }

        let can_load_older = visible_messages.len() >= target;
        self.loaded_count.insert(chat_id.to_string(), storage_len);

        process_hypernote_responses(
            &mut msgs,
            &hypernote_responses,
            &my_pubkey_hex,
            &member_profiles,
        );
        process_html_updates(&mut msgs);
        process_html_state_updates(&mut msgs);

        let unread_count = *self.unread_counts.get(chat_id).unwrap_or(&0) as usize;
        let first_unread_message_id = if unread_count > 0 && unread_count <= msgs.len() {
            Some(msgs[msgs.len() - unread_count].id.clone())
        } else {
            None
        };

        let is_admin = entry.admin_pubkeys.contains(&my_pubkey_hex);
        let members_for_state: Vec<MemberInfo> = entry
            .members
            .iter()
            .map(|m| m.to_member_info(&entry.admin_pubkeys))
            .collect();

        let typing = self.get_active_typers(chat_id);

        self.state.current_chat = Some(ChatViewState {
            chat_id: chat_id.to_string(),
            is_group: entry.is_group,
            group_name: entry.group_name,
            members: members_for_state,
            is_admin,
            messages: msgs,
            first_unread_message_id,
            can_load_older,
            typing_members: typing,
        });
        self.emit_current_chat();
    }

    pub(super) fn load_older_messages(&mut self, chat_id: &str, limit: usize) {
        let Some(sess) = self.session.as_ref() else {
            return;
        };
        let Some(entry) = sess.groups.get(chat_id).cloned() else {
            return;
        };

        let my_pubkey_hex = sess.pubkey.to_hex();

        let sender_names = self.build_sender_names(&entry.members, &my_pubkey_hex);
        let member_profiles = self.build_member_profiles(sess, &entry.members, &my_pubkey_hex);

        let base_offset = *self.loaded_count.get(chat_id).unwrap_or(&0);
        let mut visible_page = Vec::new();
        let mut total_fetched = 0;
        loop {
            let batch = sess
                .mdk
                .get_messages(
                    &entry.mls_group_id,
                    Some(Pagination::new(
                        Some(limit),
                        Some(base_offset + total_fetched),
                    )),
                )
                .unwrap_or_default();
            let batch_len = batch.len();
            total_fetched += batch_len;
            visible_page.extend(
                batch
                    .into_iter()
                    .filter(|m| classify_app_message(m).is_some_and(|k| k.is_chat_visible())),
            );
            if batch_len < limit || visible_page.len() >= limit {
                break;
            }
        }

        if visible_page.is_empty() {
            if let Some(cur) = self.state.current_chat.as_mut() {
                if cur.chat_id == chat_id {
                    cur.can_load_older = false;
                    self.emit_current_chat();
                }
            }
            return;
        }

        let separated = separate_messages(&visible_page, &sender_names);

        let mut older: Vec<ChatMessage> = separated
            .regular
            .into_iter()
            .rev()
            .map(|m| {
                let mut cm =
                    build_chat_message(m, &my_pubkey_hex, &sender_names, &separated.reaction_map);
                cm.delivery = self
                    .delivery_overrides
                    .get(chat_id)
                    .and_then(|map| map.get(&cm.id))
                    .cloned()
                    .unwrap_or(MessageDeliveryState::Sent);
                cm.media = self.chat_media_attachments_for_tags(
                    &sess.mdk,
                    &entry.mls_group_id,
                    chat_id,
                    &my_pubkey_hex,
                    &m.tags,
                );
                cm
            })
            .collect();

        process_hypernote_responses(
            &mut older,
            &separated.hypernote_responses,
            &my_pubkey_hex,
            &member_profiles,
        );

        if let Some(cur) = self.state.current_chat.as_mut() {
            if cur.chat_id == chat_id {
                older.append(&mut cur.messages);
                cur.messages = older;
                cur.can_load_older = total_fetched >= limit;
                self.loaded_count
                    .insert(chat_id.to_string(), base_offset + total_fetched);
                process_html_updates(&mut cur.messages);
                process_html_state_updates(&mut cur.messages);
                let unread_count = *self.unread_counts.get(chat_id).unwrap_or(&0) as usize;
                cur.first_unread_message_id =
                    if unread_count > 0 && unread_count <= cur.messages.len() {
                        Some(cur.messages[cur.messages.len() - unread_count].id.clone())
                    } else {
                        None
                    };
                self.emit_current_chat();
            }
        }
    }
}

fn truncated_npub(s: &str) -> String {
    if s.len() > 16 {
        format!("{}...", &s[..12])
    } else {
        s.to_string()
    }
}

fn format_display_timestamp(timestamp: i64) -> String {
    use chrono::TimeZone;
    let display = chrono::Utc
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|utc| utc.with_timezone(&chrono::Local))
        .unwrap_or_else(chrono::Local::now)
        .format("%l:%M %p")
        .to_string();
    display.trim().to_string()
}

fn parse_message_segments(content: &str) -> Vec<MessageSegment> {
    static SEGMENT_RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = SEGMENT_RE.get_or_init(|| {
        regex::Regex::new(r"```pika-([\w-]+)(?:[ \t]+(\S+))?\n([\s\S]*?)```")
            .expect("valid pika segment regex")
    });

    let mut segments = Vec::new();
    let mut last_end = 0usize;

    for caps in re.captures_iter(content) {
        let Some(full_match) = caps.get(0) else {
            continue;
        };

        let before = &content[last_end..full_match.start()];
        if !before.trim().is_empty() {
            segments.push(MessageSegment::Markdown {
                text: before.to_string(),
            });
        }

        let block_type = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        let block_id = caps.get(2).map(|m| m.as_str().trim().to_string());
        let block_body = caps
            .get(3)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();

        match block_type {
            "html" => segments.push(MessageSegment::PikaHtml {
                id: block_id,
                html: block_body,
            }),
            "html-update" | "html-state-update" | "prompt-response" => {}
            _ => segments.push(MessageSegment::Markdown {
                text: full_match.as_str().to_string(),
            }),
        }

        last_end = full_match.end();
    }

    let tail = &content[last_end..];
    if !tail.trim().is_empty() {
        segments.push(MessageSegment::Markdown {
            text: tail.to_string(),
        });
    }

    segments
}

fn first_event_tag_id(tags: &Tags) -> Option<String> {
    tags.iter().find_map(|tag| {
        if tag.kind() == TagKind::e() {
            tag.content().map(|s| s.to_string())
        } else {
            None
        }
    })
}

fn last_event_tag_id(tags: &Tags) -> Option<String> {
    tags.iter()
        .filter(|tag| tag.kind() == TagKind::e())
        .last()
        .and_then(|tag| tag.content().map(|s| s.to_string()))
}

struct SeparatedMessages<'a> {
    /// reaction_target_id → Vec<(emoji, sender_pubkey_hex)>
    reaction_map: HashMap<String, Vec<(String, String)>>,
    hypernote_responses: Vec<HypernoteResponseMessage>,
    regular: Vec<&'a message_types::Message>,
}

/// Separate a flat list of stored messages into reaction map, hypernote
/// responses, and regular (displayable) messages.
fn separate_messages<'a>(
    messages: &'a [message_types::Message],
    sender_names: &HashMap<String, String>,
) -> SeparatedMessages<'a> {
    let mut reaction_map: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut hypernote_responses: Vec<HypernoteResponseMessage> = Vec::new();
    let mut regular_messages = Vec::new();
    for m in messages {
        match classify_app_message(m) {
            Some(AppMessageKind::Reaction) => {
                if let Some(target_id) = first_event_tag_id(&m.tags) {
                    let emoji = if m.content.is_empty() || m.content == "+" {
                        "\u{2764}\u{FE0F}".to_string()
                    } else {
                        m.content.clone()
                    };
                    reaction_map
                        .entry(target_id)
                        .or_default()
                        .push((emoji, m.pubkey.to_hex()));
                }
            }
            Some(AppMessageKind::HypernoteResponse) => {
                let sender_hex = m.pubkey.to_hex();
                if let Some(response) = parse_hypernote_response_message(
                    sender_hex.clone(),
                    sender_names.get(&sender_hex).cloned(),
                    m.created_at.as_secs() as i64,
                    last_event_tag_id(&m.tags),
                    &m.content,
                ) {
                    hypernote_responses.push(response);
                }
            }
            Some(AppMessageKind::Chat | AppMessageKind::Hypernote) => {
                regular_messages.push(m);
            }
            _ => {}
        }
    }
    SeparatedMessages {
        reaction_map,
        hypernote_responses,
        regular: regular_messages,
    }
}

/// Convert a stored message into a ChatMessage for the UI, including
/// reaction aggregation and hypernote parsing.
fn build_chat_message(
    m: &super::message_types::Message,
    my_pubkey_hex: &str,
    sender_names: &HashMap<String, String>,
    reaction_map: &HashMap<String, Vec<(String, String)>>,
) -> ChatMessage {
    let id = m.id.to_hex();
    let sender_hex = m.pubkey.to_hex();
    let is_mine = sender_hex == my_pubkey_hex;
    let sender_name = sender_names.get(&sender_hex).cloned();
    let (display_content, mentions) = resolve_mentions(&m.content, sender_names);
    let segments = parse_message_segments(&display_content);
    let timestamp = m.created_at.as_secs() as i64;

    let reactions = if let Some(rxns) = reaction_map.get(&id) {
        let mut emoji_counts: HashMap<String, (u32, bool)> = HashMap::new();
        for (emoji, sender) in rxns {
            let entry = emoji_counts.entry(emoji.clone()).or_insert((0, false));
            entry.0 += 1;
            if sender == my_pubkey_hex {
                entry.1 = true;
            }
        }
        emoji_counts
            .into_iter()
            .map(
                |(emoji, (count, reacted_by_me))| crate::state::ReactionSummary {
                    emoji,
                    count,
                    reacted_by_me,
                },
            )
            .collect()
    } else {
        vec![]
    };

    let hypernote = if m.kind == super::HYPERNOTE_KIND {
        let ast_json = hypernote_mdx::serialize_tree(&hypernote_mdx::parse(&m.content));
        let declared_actions = hn::extract_submit_actions_from_ast_json(&ast_json);
        let title = m
            .tags
            .iter()
            .find(|t| t.kind() == TagKind::custom("title"))
            .and_then(|t| t.content().map(|s| s.to_string()));
        let default_state = m
            .tags
            .iter()
            .find(|t| t.kind() == TagKind::custom("state"))
            .and_then(|t| t.content().map(|s| s.to_string()));
        Some(crate::state::HypernoteData {
            ast_json,
            declared_actions,
            title,
            default_state,
            my_response: None,
            response_tallies: vec![],
            responders: vec![],
        })
    } else {
        None
    };

    ChatMessage {
        id,
        sender_pubkey: sender_hex,
        sender_name,
        content: m.content.clone(),
        display_content,
        reply_to_message_id: last_event_tag_id(&m.tags),
        mentions,
        timestamp,
        display_timestamp: format_display_timestamp(timestamp),
        is_mine,
        delivery: MessageDeliveryState::Sent,
        reactions,
        media: vec![],
        segments,
        html_state: None,
        hypernote,
    }
}

#[derive(Debug, Clone)]
struct HypernoteResponseMessage {
    sender_pubkey: String,
    sender_name: Option<String>,
    target_hypernote_id: String,
    action: String,
    timestamp: i64,
}

fn parse_hypernote_response_message(
    sender_pubkey: String,
    sender_name: Option<String>,
    timestamp: i64,
    target_hypernote_id: Option<String>,
    content: &str,
) -> Option<HypernoteResponseMessage> {
    let target_hypernote_id = target_hypernote_id?;
    let parsed = hn::parse_action_response(content)?;
    Some(HypernoteResponseMessage {
        sender_pubkey,
        sender_name,
        target_hypernote_id,
        action: parsed.action,
        timestamp,
    })
}

/// Tally explicit kind-9468 response messages onto matching kind-9467 hypernotes.
fn process_hypernote_responses(
    msgs: &mut [ChatMessage],
    responses: &[HypernoteResponseMessage],
    my_pubkey_hex: &str,
    member_profiles: &HashMap<String, (Option<String>, String, Option<String>)>,
) {
    if responses.is_empty() {
        return;
    }

    // Keep only the latest response per (sender, target hypernote).
    let mut latest_responses: HashMap<(String, String), (String, i64, Option<String>)> =
        HashMap::new();
    for response in responses {
        let key = (
            response.sender_pubkey.clone(),
            response.target_hypernote_id.clone(),
        );
        if latest_responses
            .get(&key)
            .map(|(_, ts, _)| response.timestamp > *ts)
            .unwrap_or(true)
        {
            latest_responses.insert(
                key,
                (
                    response.action.clone(),
                    response.timestamp,
                    response.sender_name.clone(),
                ),
            );
        }
    }

    // Attach tallies and responders to matching hypernote messages.
    for msg in msgs.iter_mut() {
        if msg.hypernote.is_none() {
            continue;
        }
        let hn = msg.hypernote.as_mut().unwrap();
        let declared: HashSet<String> = hn.declared_actions.iter().cloned().collect();

        let mut action_counts: HashMap<String, u32> = HashMap::new();
        let mut responder_pubkeys: Vec<String> = Vec::new();
        let mut my_response: Option<String> = None;

        for ((sender_pubkey, hypernote_id), (action, _, _sender_name)) in &latest_responses {
            if hypernote_id != &msg.id {
                continue;
            }
            if !declared.is_empty() && !declared.contains(action) {
                continue;
            }
            *action_counts.entry(action.clone()).or_insert(0) += 1;
            if !responder_pubkeys.contains(sender_pubkey) {
                responder_pubkeys.push(sender_pubkey.clone());
            }
            if sender_pubkey == my_pubkey_hex {
                my_response = Some(action.clone());
            }
        }

        hn.response_tallies = action_counts
            .iter()
            .map(|(action, count)| HypernoteResponseTally {
                action: action.clone(),
                count: *count,
            })
            .collect();
        hn.response_tallies
            .sort_by_key(|t| std::cmp::Reverse(t.count));
        hn.my_response = my_response;
        hn.responders = responder_pubkeys
            .iter()
            .map(|pk| {
                if let Some((name, npub, picture_url)) = member_profiles.get(pk) {
                    HypernoteResponder {
                        name: name.clone(),
                        npub: npub.clone(),
                        picture_url: picture_url.clone(),
                    }
                } else {
                    HypernoteResponder {
                        name: None,
                        npub: pk[..pk.len().min(16)].to_string(),
                        picture_url: None,
                    }
                }
            })
            .collect();
    }
}

/// Extract the application-level ID from a `pika-html <id>` fence line.
/// Returns `None` for plain `pika-html` blocks (no ID).
fn parse_html_id(content: &str) -> Option<String> {
    let marker = "```pika-html ";
    let start = content.find(marker)?;
    let rest = &content[start + marker.len()..];
    let line_end = rest.find('\n')?;
    let id = rest[..line_end].trim();
    if id.is_empty() {
        return None;
    }
    // Ensure it looks like a simple token (no spaces)
    if id.contains(' ') {
        return None;
    }
    Some(id.to_string())
}

/// Extract `(target_id, new_html_content)` from a `pika-html-update <id>` block.
fn parse_html_update(content: &str) -> Option<(String, String)> {
    let marker = "```pika-html-update ";
    let start = content.find(marker)?;
    let rest = &content[start + marker.len()..];
    let line_end = rest.find('\n')?;
    let id = rest[..line_end].trim().to_string();
    if id.is_empty() {
        return None;
    }
    let body_start = &rest[line_end + 1..];
    let end = body_start.find("```")?;
    let full_body = body_start[..end].to_string();
    // Reconstruct the updated content as a pika-html block with the same ID
    let new_content = format!("```pika-html {}\n{}```", id, full_body);
    Some((id, new_content))
}

/// Scan messages for `pika-html-update` blocks, merge them into the original
/// `pika-html` messages by ID, and remove the update messages.
fn process_html_updates(msgs: &mut Vec<ChatMessage>) {
    // Collect updates: target_id -> (new_content, index, timestamp)
    // Keep only the latest update per target_id.
    let mut latest_updates: HashMap<String, (String, i64)> = HashMap::new();
    let mut update_indices: Vec<usize> = Vec::new();

    for (i, msg) in msgs.iter().enumerate() {
        if let Some((target_id, new_content)) = parse_html_update(&msg.content) {
            tracing::debug!(target_id, msg_id = msg.id, "html-update found");
            let is_newer = latest_updates
                .get(&target_id)
                .map(|(_, ts)| msg.timestamp > *ts)
                .unwrap_or(true);
            if is_newer {
                latest_updates.insert(target_id, (new_content, msg.timestamp));
            }
            update_indices.push(i);
        }
    }

    if update_indices.is_empty() {
        return;
    }

    // Scan originals and apply updates.
    let mut matched = 0usize;
    for msg in msgs.iter_mut() {
        if let Some(html_id) = parse_html_id(&msg.content) {
            if let Some((new_content, _)) = latest_updates.get(&html_id) {
                tracing::debug!(html_id, msg_id = msg.id, "html-update applied to original");
                msg.content = new_content.clone();
                msg.display_content = new_content.clone();
                msg.segments = parse_message_segments(&msg.display_content);
                matched += 1;
            }
        }
    }

    if matched == 0 {
        tracing::warn!(
            update_count = update_indices.len(),
            ids = ?latest_updates.keys().collect::<Vec<_>>(),
            "html-update(s) found but no matching pika-html originals"
        );
    }

    // Remove update messages (reverse order to preserve indices).
    for i in update_indices.into_iter().rev() {
        msgs.remove(i);
    }
}

/// Extract `(target_id, state_body)` from a `pika-html-state-update <id>` block.
fn parse_html_state_update(content: &str) -> Option<(String, String)> {
    let marker = "```pika-html-state-update ";
    let start = content.find(marker)?;
    let rest = &content[start + marker.len()..];
    let line_end = rest.find('\n')?;
    let id = rest[..line_end].trim().to_string();
    if id.is_empty() {
        return None;
    }
    let body_start = &rest[line_end + 1..];
    let end = body_start.find("```")?;
    let body = body_start[..end].trim().to_string();
    Some((id, body))
}

/// Scan messages for `pika-html-state-update` blocks, store body in `html_state`
/// on the matching original `pika-html` message, and remove the state-update messages.
fn process_html_state_updates(msgs: &mut Vec<ChatMessage>) {
    // Collect state updates: target_id -> (state_body, timestamp)
    // Keep only the latest per target_id.
    let mut latest_states: HashMap<String, (String, i64)> = HashMap::new();
    let mut update_indices: Vec<usize> = Vec::new();

    for (i, msg) in msgs.iter().enumerate() {
        if let Some((target_id, state_body)) = parse_html_state_update(&msg.content) {
            let is_newer = latest_states
                .get(&target_id)
                .map(|(_, ts)| msg.timestamp > *ts)
                .unwrap_or(true);
            if is_newer {
                latest_states.insert(target_id, (state_body, msg.timestamp));
            }
            update_indices.push(i);
        }
    }

    if update_indices.is_empty() {
        return;
    }

    // Apply state to matching originals (do NOT touch content/display_content).
    for msg in msgs.iter_mut() {
        if let Some(html_id) = parse_html_id(&msg.content) {
            if let Some((state_body, _)) = latest_states.get(&html_id) {
                msg.html_state = Some(state_body.clone());
            }
        }
    }

    // Remove state-update messages (reverse order to preserve indices).
    for i in update_indices.into_iter().rev() {
        msgs.remove(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::MessageDeliveryState;

    fn make_msg(id: &str, content: &str, timestamp: i64) -> ChatMessage {
        let display_content = content.to_string();
        ChatMessage {
            id: id.to_string(),
            sender_pubkey: "aabb".to_string(),
            sender_name: None,
            content: content.to_string(),
            display_content: display_content.clone(),
            reply_to_message_id: None,
            mentions: vec![],
            timestamp,
            display_timestamp: format_display_timestamp(timestamp),
            is_mine: false,
            delivery: MessageDeliveryState::Sent,
            reactions: vec![],
            media: vec![],
            segments: parse_message_segments(&display_content),
            html_state: None,
            hypernote: None,
        }
    }

    #[test]
    fn parse_message_segments_splits_markdown_and_html() {
        let content = "hello\n```pika-html widget\n<div>Hi</div>\n```\nworld";
        let segments = parse_message_segments(content);
        assert_eq!(segments.len(), 3);
        assert!(matches!(
            &segments[0],
            MessageSegment::Markdown { text } if text.contains("hello")
        ));
        assert!(matches!(
            &segments[1],
            MessageSegment::PikaHtml { id: Some(id), html } if id == "widget" && html == "<div>Hi</div>"
        ));
        assert!(matches!(
            &segments[2],
            MessageSegment::Markdown { text } if text.contains("world")
        ));
    }

    #[test]
    fn parse_message_segments_drops_prompt_response_blocks() {
        let content =
            "before\n```pika-prompt-response\n{\"prompt_id\":\"a\",\"selected\":\"x\"}\n```\nafter";
        let segments = parse_message_segments(content);
        assert_eq!(segments.len(), 2);
        assert!(matches!(segments[0], MessageSegment::Markdown { .. }));
        assert!(matches!(segments[1], MessageSegment::Markdown { .. }));
    }

    #[test]
    fn first_event_tag_id_uses_first_e_tag() {
        let mut tags = Tags::new();
        tags.push(Tag::parse(vec!["e", "first"]).unwrap());
        tags.push(Tag::parse(vec!["e", "second"]).unwrap());

        assert_eq!(first_event_tag_id(&tags).as_deref(), Some("first"));
    }

    #[test]
    fn last_event_tag_id_uses_last_e_tag() {
        let mut tags = Tags::new();
        tags.push(Tag::parse(vec!["e", "first"]).unwrap());
        tags.push(Tag::parse(vec!["k", "9"]).unwrap());
        tags.push(Tag::parse(vec!["e", "second"]).unwrap());

        assert_eq!(last_event_tag_id(&tags).as_deref(), Some("second"));
    }

    #[test]
    fn parse_html_id_with_id() {
        let content = "```pika-html dashboard\n<h1>Loading...</h1>\n```";
        assert_eq!(parse_html_id(content), Some("dashboard".to_string()));
    }

    #[test]
    fn parse_html_id_no_id() {
        let content = "```pika-html\n<h1>Static</h1>\n```";
        assert_eq!(parse_html_id(content), None);
    }

    #[test]
    fn parse_html_id_ignores_update() {
        let content = "```pika-html-update dashboard\n<h1>New</h1>\n```";
        // "pika-html " marker doesn't match "pika-html-update "
        assert_eq!(parse_html_id(content), None);
    }

    #[test]
    fn parse_html_update_extracts_id_and_content() {
        let content = "```pika-html-update dashboard\n<h1>Results!</h1>\n```";
        let (id, new_content) = parse_html_update(content).unwrap();
        assert_eq!(id, "dashboard");
        assert!(new_content.contains("```pika-html dashboard\n"));
        assert!(new_content.contains("<h1>Results!</h1>"));
    }

    #[test]
    fn parse_html_update_no_match_for_plain_html() {
        let content = "```pika-html dashboard\n<h1>Loading</h1>\n```";
        assert!(parse_html_update(content).is_none());
    }

    #[test]
    fn process_html_updates_merges_and_removes() {
        let mut msgs = vec![
            make_msg(
                "m1",
                "```pika-html dashboard\n<h1>Loading...</h1>\n```",
                100,
            ),
            make_msg("m2", "Hello world", 101),
            make_msg(
                "m3",
                "```pika-html-update dashboard\n<h1>Results ready!</h1>\n```",
                102,
            ),
        ];

        process_html_updates(&mut msgs);

        // Should have 2 messages (update removed)
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].id, "m1");
        assert!(msgs[0].content.contains("Results ready!"));
        assert!(msgs[0].display_content.contains("Results ready!"));
        assert_eq!(msgs[1].id, "m2");
    }

    #[test]
    fn process_html_updates_last_update_wins() {
        let mut msgs = vec![
            make_msg("m1", "```pika-html dash\n<h1>V1</h1>\n```", 100),
            make_msg("m2", "```pika-html-update dash\n<h1>V2</h1>\n```", 101),
            make_msg("m3", "```pika-html-update dash\n<h1>V3</h1>\n```", 102),
        ];

        process_html_updates(&mut msgs);

        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].content.contains("V3"));
    }

    #[test]
    fn process_html_updates_no_op_without_updates() {
        let mut msgs = vec![
            make_msg("m1", "```pika-html dashboard\n<h1>Static</h1>\n```", 100),
            make_msg("m2", "Hello", 101),
        ];

        process_html_updates(&mut msgs);

        assert_eq!(msgs.len(), 2);
        assert!(msgs[0].content.contains("Static"));
    }

    #[test]
    fn process_html_updates_plain_html_unaffected() {
        let mut msgs = vec![
            make_msg("m1", "```pika-html\n<h1>No ID</h1>\n```", 100),
            make_msg(
                "m2",
                "```pika-html-update dashboard\n<h1>Orphan update</h1>\n```",
                101,
            ),
        ];

        process_html_updates(&mut msgs);

        // Update removed but original unchanged (no matching ID)
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].content.contains("No ID"));
    }

    #[test]
    fn parse_html_state_update_extracts_id_and_body() {
        let content = "```pika-html-state-update avatar\n{\"expression\":\"happy\"}\n```";
        let (id, body) = parse_html_state_update(content).unwrap();
        assert_eq!(id, "avatar");
        assert_eq!(body, "{\"expression\":\"happy\"}");
    }

    #[test]
    fn parse_html_state_update_no_match_for_plain_html() {
        let content = "```pika-html avatar\n<h1>Hello</h1>\n```";
        assert!(parse_html_state_update(content).is_none());
    }

    #[test]
    fn parse_html_state_update_no_match_for_html_update() {
        let content = "```pika-html-update avatar\n<h1>New</h1>\n```";
        assert!(parse_html_state_update(content).is_none());
    }

    #[test]
    fn process_html_state_updates_sets_state_preserves_content() {
        let mut msgs = vec![
            make_msg("m1", "```pika-html avatar\n<h1>3D Avatar</h1>\n```", 100),
            make_msg("m2", "Hello world", 101),
            make_msg(
                "m3",
                "```pika-html-state-update avatar\n{\"expression\":\"happy\"}\n```",
                102,
            ),
        ];

        process_html_state_updates(&mut msgs);

        // State-update message removed
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].id, "m1");
        // Content and display_content are preserved
        assert!(msgs[0].content.contains("3D Avatar"));
        assert!(msgs[0].display_content.contains("3D Avatar"));
        // html_state is set
        assert_eq!(
            msgs[0].html_state.as_deref(),
            Some("{\"expression\":\"happy\"}")
        );
        assert_eq!(msgs[1].id, "m2");
        assert!(msgs[1].html_state.is_none());
    }

    #[test]
    fn process_html_state_updates_last_state_wins() {
        let mut msgs = vec![
            make_msg("m1", "```pika-html dash\n<h1>Dashboard</h1>\n```", 100),
            make_msg("m2", "```pika-html-state-update dash\n{\"v\":1}\n```", 101),
            make_msg("m3", "```pika-html-state-update dash\n{\"v\":2}\n```", 102),
        ];

        process_html_state_updates(&mut msgs);

        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].html_state.as_deref(), Some("{\"v\":2}"));
    }

    #[test]
    fn process_html_state_updates_no_op_without_state_updates() {
        let mut msgs = vec![
            make_msg("m1", "```pika-html avatar\n<h1>Hello</h1>\n```", 100),
            make_msg("m2", "Just a text message", 101),
        ];

        process_html_state_updates(&mut msgs);

        assert_eq!(msgs.len(), 2);
        assert!(msgs[0].html_state.is_none());
    }

    fn make_hypernote_msg(id: &str, declared_actions: &[&str]) -> ChatMessage {
        let mut msg = make_msg(id, "# Note", 100);
        msg.hypernote = Some(crate::state::HypernoteData {
            ast_json: "{}".to_string(),
            declared_actions: declared_actions.iter().map(|a| a.to_string()).collect(),
            title: None,
            default_state: None,
            my_response: None,
            response_tallies: vec![],
            responders: vec![],
        });
        msg
    }

    #[test]
    fn parse_hypernote_response_requires_target_and_action() {
        assert!(parse_hypernote_response_message(
            "alice".to_string(),
            None,
            1,
            None,
            r#"{"action":"yes","form":{}}"#
        )
        .is_none());
        assert!(parse_hypernote_response_message(
            "alice".to_string(),
            None,
            1,
            Some("note1".to_string()),
            r#"{"form":{}}"#
        )
        .is_none());
    }

    #[test]
    fn process_hypernote_responses_uses_latest_and_declared_actions_only() {
        let mut msgs = vec![make_hypernote_msg("note1", &["yes", "no"])];
        let responses = vec![
            HypernoteResponseMessage {
                sender_pubkey: "alice".to_string(),
                sender_name: Some("Alice".to_string()),
                target_hypernote_id: "note1".to_string(),
                action: "yes".to_string(),
                timestamp: 10,
            },
            HypernoteResponseMessage {
                sender_pubkey: "alice".to_string(),
                sender_name: Some("Alice".to_string()),
                target_hypernote_id: "note1".to_string(),
                action: "no".to_string(),
                timestamp: 11,
            },
            HypernoteResponseMessage {
                sender_pubkey: "bob".to_string(),
                sender_name: Some("Bob".to_string()),
                target_hypernote_id: "note1".to_string(),
                action: "maybe".to_string(),
                timestamp: 12,
            },
            HypernoteResponseMessage {
                sender_pubkey: "carol".to_string(),
                sender_name: Some("Carol".to_string()),
                target_hypernote_id: "note1".to_string(),
                action: "yes".to_string(),
                timestamp: 13,
            },
        ];

        let member_profiles = HashMap::from([
            (
                "alice".to_string(),
                (
                    Some("Alice".to_string()),
                    "npub_alice".to_string(),
                    Some("https://img/alice.png".to_string()),
                ),
            ),
            (
                "bob".to_string(),
                (
                    Some("Bob".to_string()),
                    "npub_bob".to_string(),
                    Some("https://img/bob.png".to_string()),
                ),
            ),
            (
                "carol".to_string(),
                (
                    Some("Carol".to_string()),
                    "npub_carol".to_string(),
                    Some("https://img/carol.png".to_string()),
                ),
            ),
        ]);

        process_hypernote_responses(&mut msgs, &responses, "alice", &member_profiles);

        let hn = msgs[0].hypernote.as_ref().expect("has hypernote");
        assert_eq!(hn.my_response.as_deref(), Some("no"));

        let mut tallies = hn.response_tallies.clone();
        tallies.sort_by(|a, b| a.action.cmp(&b.action));
        assert_eq!(tallies.len(), 2);
        assert_eq!(tallies[0].action, "no");
        assert_eq!(tallies[0].count, 1);
        assert_eq!(tallies[1].action, "yes");
        assert_eq!(tallies[1].count, 1);

        // "maybe" is undeclared and must not appear in responders/tallies.
        let responder_npubs: Vec<String> = hn.responders.iter().map(|r| r.npub.clone()).collect();
        assert!(responder_npubs.contains(&"npub_alice".to_string()));
        assert!(responder_npubs.contains(&"npub_carol".to_string()));
        assert!(!responder_npubs.contains(&"npub_bob".to_string()));
    }

    // --- separate_messages / build_chat_message tests ---

    use mdk_core::prelude::message_types;
    use nostr_sdk::prelude::*;

    fn make_stored_msg(
        id_byte: u8,
        kind: Kind,
        content: &str,
        tags: Tags,
        timestamp: u64,
    ) -> message_types::Message {
        let pubkey = PublicKey::from_byte_array([id_byte; 32]);
        let created_at = Timestamp::from_secs(timestamp);
        let mut id_bytes = [0u8; 32];
        id_bytes[0] = id_byte;
        message_types::Message {
            id: EventId::from_byte_array(id_bytes),
            pubkey,
            kind,
            mls_group_id: mdk_core::prelude::GroupId::from_slice(&[1]),
            created_at,
            processed_at: created_at,
            content: content.to_string(),
            tags: tags.clone(),
            event: UnsignedEvent::new(pubkey, created_at, kind, tags, content.to_string()),
            wrapper_event_id: EventId::all_zeros(),
            epoch: None,
            state: message_types::MessageState::Processed,
        }
    }

    #[test]
    fn separate_messages_splits_by_kind() {
        let msgs = vec![
            make_stored_msg(1, Kind::ChatMessage, "hello", Tags::new(), 100),
            make_stored_msg(
                2,
                Kind::Reaction,
                "+",
                {
                    let mut t = Tags::new();
                    t.push(Tag::parse(vec!["e", "target1"]).unwrap());
                    t
                },
                101,
            ),
            make_stored_msg(
                3,
                Kind::Custom(hypernote_protocol::HYPERNOTE_KIND),
                "# Poll",
                Tags::new(),
                102,
            ),
            make_stored_msg(
                4,
                Kind::Custom(hypernote_protocol::HYPERNOTE_ACTION_RESPONSE_KIND),
                r#"{"action":"yes","form":{}}"#,
                {
                    let mut t = Tags::new();
                    t.push(Tag::parse(vec!["e", "note1"]).unwrap());
                    t
                },
                103,
            ),
        ];

        let sender_names = HashMap::new();
        let separated = separate_messages(&msgs, &sender_names);

        // Chat + Hypernote go to regular
        assert_eq!(separated.regular.len(), 2);
        assert_eq!(separated.regular[0].kind, Kind::ChatMessage);
        assert_eq!(
            separated.regular[1].kind,
            Kind::Custom(hypernote_protocol::HYPERNOTE_KIND)
        );

        // Reaction goes to reaction_map
        assert_eq!(separated.reaction_map.len(), 1);
        assert!(separated.reaction_map.contains_key("target1"));
        let rxns = &separated.reaction_map["target1"];
        assert_eq!(rxns.len(), 1);
        assert_eq!(rxns[0].0, "\u{2764}\u{FE0F}"); // "+" becomes heart

        // HypernoteResponse goes to hypernote_responses
        assert_eq!(separated.hypernote_responses.len(), 1);
        assert_eq!(separated.hypernote_responses[0].action, "yes");
        assert_eq!(
            separated.hypernote_responses[0].target_hypernote_id,
            "note1"
        );
    }

    #[test]
    fn separate_messages_ignores_unknown_kinds() {
        let msgs = vec![
            make_stored_msg(1, Kind::ChatMessage, "hello", Tags::new(), 100),
            make_stored_msg(2, Kind::Custom(9999), "unknown", Tags::new(), 101),
        ];

        let sender_names = HashMap::new();
        let separated = separate_messages(&msgs, &sender_names);

        assert_eq!(separated.regular.len(), 1);
        assert!(separated.reaction_map.is_empty());
        assert!(separated.hypernote_responses.is_empty());
    }

    #[test]
    fn is_chat_visible_excludes_typing_and_call_signals() {
        assert!(AppMessageKind::Chat.is_chat_visible());
        assert!(AppMessageKind::Reaction.is_chat_visible());
        assert!(AppMessageKind::Hypernote.is_chat_visible());
        assert!(AppMessageKind::HypernoteResponse.is_chat_visible());
        assert!(!AppMessageKind::TypingIndicator.is_chat_visible());
        assert!(!AppMessageKind::CallSignal.is_chat_visible());
    }

    #[test]
    fn typing_indicators_and_call_signals_not_chat_visible() {
        // Typing indicator: kind 20067 with "typing" content and "d"="pika" tag.
        let typing_msg = make_stored_msg(
            10,
            super::TYPING_INDICATOR_KIND,
            "typing",
            {
                let mut t = Tags::new();
                t.push(Tag::parse(vec!["d", "pika"]).unwrap());
                t
            },
            100,
        );
        let call_msg = make_stored_msg(11, super::CALL_SIGNAL_KIND, "signal", Tags::new(), 101);
        let chat_msg = make_stored_msg(12, Kind::ChatMessage, "hello", Tags::new(), 102);

        // classify_app_message + is_chat_visible should filter correctly.
        let visible: Vec<_> = [&typing_msg, &call_msg, &chat_msg]
            .into_iter()
            .filter(|m| super::classify_app_message(m).is_some_and(|k| k.is_chat_visible()))
            .collect();

        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].kind, Kind::ChatMessage);
    }

    #[test]
    fn separate_messages_reaction_custom_emoji() {
        let msgs = vec![make_stored_msg(
            1,
            Kind::Reaction,
            "🔥",
            {
                let mut t = Tags::new();
                t.push(Tag::parse(vec!["e", "msg1"]).unwrap());
                t
            },
            100,
        )];

        let sender_names = HashMap::new();
        let separated = separate_messages(&msgs, &sender_names);

        let rxns = &separated.reaction_map["msg1"];
        assert_eq!(rxns[0].0, "🔥"); // custom emoji preserved
    }

    #[test]
    fn build_chat_message_creates_basic_message() {
        let msg = make_stored_msg(1, Kind::ChatMessage, "hello world", Tags::new(), 100);
        let sender_names = HashMap::new();
        let reaction_map = HashMap::new();
        let pubkey_hex = msg.pubkey.to_hex();

        let cm = build_chat_message(&msg, &pubkey_hex, &sender_names, &reaction_map);

        assert_eq!(cm.content, "hello world");
        assert!(cm.is_mine);
        assert!(cm.reactions.is_empty());
        assert!(cm.hypernote.is_none());
    }

    #[test]
    fn build_chat_message_attaches_reactions() {
        let msg = make_stored_msg(1, Kind::ChatMessage, "hello", Tags::new(), 100);
        let msg_id = msg.id.to_hex();
        let sender_names = HashMap::new();
        let reaction_map = HashMap::from([(
            msg_id,
            vec![
                ("🔥".to_string(), "alice".to_string()),
                ("🔥".to_string(), "bob".to_string()),
                ("👍".to_string(), "alice".to_string()),
            ],
        )]);

        let cm = build_chat_message(&msg, "someone_else", &sender_names, &reaction_map);

        assert_eq!(cm.reactions.len(), 2);
        let fire = cm.reactions.iter().find(|r| r.emoji == "🔥").unwrap();
        assert_eq!(fire.count, 2);
        assert!(!fire.reacted_by_me);
        let thumbs = cm.reactions.iter().find(|r| r.emoji == "👍").unwrap();
        assert_eq!(thumbs.count, 1);
    }

    #[test]
    fn build_chat_message_parses_hypernote() {
        let msg = make_stored_msg(
            1,
            Kind::Custom(hypernote_protocol::HYPERNOTE_KIND),
            "# My Poll\n\nVote!",
            Tags::new(),
            100,
        );
        let sender_names = HashMap::new();
        let reaction_map = HashMap::new();

        let cm = build_chat_message(&msg, "someone", &sender_names, &reaction_map);

        assert!(cm.hypernote.is_some());
        let hn = cm.hypernote.unwrap();
        assert!(!hn.ast_json.is_empty());
    }
}
