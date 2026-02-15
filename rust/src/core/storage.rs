// Storage-derived state refresh + paging.

use super::*;
use crate::state::{resolve_mentions, MemberInfo, PollTally};

impl AppCore {
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
        let Some(sess) = self.session.as_mut() else {
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

        let my_pubkey = sess.keys.public_key();
        let mut index: HashMap<String, GroupIndexEntry> = HashMap::new();
        let mut list: Vec<ChatSummary> = Vec::new();
        let mut missing_profile_pubkeys: Vec<PublicKey> = Vec::new();

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

            // A group chat is anything with >1 other member, or explicitly named (not "DM").
            let explicit_name = if g.name != DEFAULT_GROUP_NAME && !g.name.is_empty() {
                Some(g.name.clone())
            } else {
                None
            };
            let is_group = other_members.len() > 1 || explicit_name.is_some();

            // Build member info with cached profiles.
            let now = crate::state::now_seconds();
            let mut member_infos: Vec<(PublicKey, Option<String>, Option<String>)> = Vec::new();
            for pk in &other_members {
                let hex = pk.to_hex();
                let cached = self.profiles.get(&hex);
                let name = cached.and_then(|p| p.name.clone());
                let picture_url = cached.and_then(|p| p.picture_url.clone());
                member_infos.push((*pk, name, picture_url));

                let needs_fetch = match cached {
                    None => true,
                    Some(p) => (now - p.fetched_at) > 3600,
                };
                if needs_fetch && !missing_profile_pubkeys.iter().any(|p| p == pk) {
                    missing_profile_pubkeys.push(*pk);
                }
            }

            let admin_pubkeys: Vec<String> = g.admin_pubkeys.iter().map(|p| p.to_hex()).collect();

            let members_for_state: Vec<MemberInfo> = member_infos
                .iter()
                .map(|(pk, name, pic)| MemberInfo {
                    pubkey: pk.to_hex(),
                    npub: pk.to_bech32().unwrap_or_else(|_| pk.to_hex()),
                    name: name.clone(),
                    picture_url: pic.clone(),
                })
                .collect();

            // Fetch newest message for preview.
            let newest = sess
                .mdk
                .get_messages(&g.mls_group_id, Some(Pagination::new(Some(1), Some(0))))
                .ok()
                .and_then(|v| v.into_iter().next());

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

            let last_message = last_message.map(|msg| {
                if msg.contains("```pika-prompt-response\n") {
                    "Voted in poll".to_string()
                } else if msg.contains("```pika-html-update ") {
                    "Updated content".to_string()
                } else {
                    msg
                }
            });

            list.push(ChatSummary {
                chat_id: chat_id.clone(),
                is_group,
                group_name: explicit_name.clone(),
                members: members_for_state,
                last_message,
                last_message_at,
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
        sess.groups = index;
        self.state.chat_list = list;
        self.emit_chat_list();

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
                    for ev in events.into_iter() {
                        let author_hex = ev.pubkey.to_hex();
                        let dominated = best
                            .get(&author_hex)
                            .map(|prev| ev.created_at > prev.created_at)
                            .unwrap_or(true);
                        if dominated {
                            best.insert(author_hex, ev);
                        }
                    }

                    let mut results: Vec<(String, Option<String>, Option<String>)> = Vec::new();
                    for (hex_pk, ev) in best {
                        // Parse kind:0 content as JSON.
                        let parsed: Option<(Option<String>, Option<String>)> =
                            serde_json::from_str::<serde_json::Value>(&ev.content)
                                .ok()
                                .map(|v| {
                                    let display_name = v
                                        .get("display_name")
                                        .and_then(|s| s.as_str())
                                        .filter(|s| !s.is_empty())
                                        .map(String::from);
                                    let name = v
                                        .get("name")
                                        .and_then(|s| s.as_str())
                                        .filter(|s| !s.is_empty())
                                        .map(String::from);
                                    let picture = v
                                        .get("picture")
                                        .and_then(|s| s.as_str())
                                        .filter(|s| !s.is_empty())
                                        .map(String::from);
                                    // Priority: display_name > name > None
                                    let best_name = display_name.or(name);
                                    (best_name, picture)
                                });
                        let (name, picture) = parsed.unwrap_or((None, None));
                        results.push((hex_pk, name, picture));
                    }

                    // Also record "no profile" for pubkeys with no kind:0 event, so we
                    // don't keep re-fetching them.
                    for pk in &pubkeys {
                        let hex = pk.to_hex();
                        if !results.iter().any(|(h, _, _)| h == &hex) {
                            results.push((hex, None, None));
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

    pub(super) fn refresh_current_chat(&mut self, chat_id: &str) {
        let Some(sess) = self.session.as_mut() else {
            self.state.current_chat = None;
            self.emit_current_chat();
            return;
        };
        let Some(entry) = sess.groups.get(chat_id).cloned() else {
            self.state.current_chat = None;
            self.emit_current_chat();
            return;
        };

        let my_pubkey_hex = sess.keys.public_key().to_hex();

        // Build a sender pubkey -> display name lookup from member info + profile cache.
        let mut sender_names: HashMap<String, String> = entry
            .members
            .iter()
            .filter_map(|(pk, name, _)| {
                let hex = pk.to_hex();
                let display = name
                    .clone()
                    .or_else(|| self.profiles.get(&hex).and_then(|p| p.name.clone()));
                display.map(|n| (hex, n))
            })
            .collect();

        // Include current user's name so poll tallies show it instead of hex.
        let my_name = &self.state.my_profile.name;
        if !my_name.is_empty() {
            sender_names.insert(my_pubkey_hex.clone(), my_name.clone());
        }

        let desired = *self.loaded_count.get(chat_id).unwrap_or(&50usize);
        let limit = desired.max(50);
        let messages = sess
            .mdk
            .get_messages(
                &entry.mls_group_id,
                Some(Pagination::new(Some(limit), Some(0))),
            )
            .unwrap_or_default();

        let storage_len = messages.len();

        // Separate reactions (kind 7) from regular messages.
        // reaction_target_id -> Vec<(emoji, sender_pubkey)>
        let mut reaction_map: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut regular_messages = Vec::new();
        for m in &messages {
            if m.kind == nostr_sdk::Kind::Reaction {
                // Find the target event id from the `e` tag.
                if let Some(target_id) = m.tags.iter().find_map(|t| {
                    if t.kind() == nostr_sdk::TagKind::e() {
                        t.content().map(|s| s.to_string())
                    } else {
                        None
                    }
                }) {
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
                continue;
            }
            regular_messages.push(m);
        }

        let mut msgs: Vec<ChatMessage> = regular_messages
            .into_iter()
            .rev()
            .map(|m| {
                let id = m.id.to_hex();
                let sender_hex = m.pubkey.to_hex();
                let is_mine = sender_hex == my_pubkey_hex;
                let sender_name = sender_names.get(&sender_hex).cloned();
                let delivery = self
                    .delivery_overrides
                    .get(chat_id)
                    .and_then(|map| map.get(&id))
                    .cloned()
                    .unwrap_or(MessageDeliveryState::Sent);
                let (display_content, mentions) = resolve_mentions(&m.content, &sender_names);

                // Aggregate reactions for this message.
                let reactions = if let Some(rxns) = reaction_map.get(&id) {
                    let mut emoji_counts: HashMap<String, (u32, bool)> = HashMap::new();
                    for (emoji, sender) in rxns {
                        let entry = emoji_counts.entry(emoji.clone()).or_insert((0, false));
                        entry.0 += 1;
                        if sender == &my_pubkey_hex {
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

                ChatMessage {
                    id,
                    sender_pubkey: sender_hex,
                    sender_name,
                    content: m.content.clone(),
                    display_content,
                    mentions,
                    timestamp: m.created_at.as_secs() as i64,
                    is_mine,
                    delivery,
                    reactions,
                    poll_tally: vec![],
                    my_poll_vote: None,
                }
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
                let delivery = self
                    .delivery_overrides
                    .get(chat_id)
                    .and_then(|map| map.get(&id))
                    .cloned()
                    .unwrap_or(MessageDeliveryState::Pending);
                let (display_content, mentions) = resolve_mentions(&lm.content, &sender_names);
                msgs.push(ChatMessage {
                    id,
                    sender_pubkey: lm.sender_pubkey,
                    sender_name: None,
                    content: lm.content,
                    display_content,
                    mentions,
                    timestamp: lm.timestamp,
                    is_mine: true,
                    delivery,
                    reactions: vec![],
                    poll_tally: vec![],
                    my_poll_vote: None,
                });
            }
            msgs.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));
        }

        if let Some(local) = self.local_outbox.get_mut(chat_id) {
            local.retain(|id, lm| !present_ids.contains(id) && lm.timestamp >= oldest_loaded_ts);
        }

        let can_load_older = storage_len == limit;
        self.loaded_count.insert(chat_id.to_string(), storage_len);

        process_html_updates(&mut msgs);
        process_poll_tallies(&mut msgs, &my_pubkey_hex);

        let is_admin = entry.admin_pubkeys.contains(&my_pubkey_hex);
        let members_for_state: Vec<MemberInfo> = entry
            .members
            .iter()
            .map(|(pk, name, pic)| MemberInfo {
                pubkey: pk.to_hex(),
                npub: pk.to_bech32().unwrap_or_else(|_| pk.to_hex()),
                name: name.clone(),
                picture_url: pic.clone(),
            })
            .collect();

        self.state.current_chat = Some(ChatViewState {
            chat_id: chat_id.to_string(),
            is_group: entry.is_group,
            group_name: entry.group_name,
            members: members_for_state,
            is_admin,
            messages: msgs,
            can_load_older,
        });
        self.emit_current_chat();
    }

    pub(super) fn load_older_messages(&mut self, chat_id: &str, limit: usize) {
        let Some(sess) = self.session.as_mut() else {
            return;
        };
        let Some(entry) = sess.groups.get(chat_id).cloned() else {
            return;
        };

        let my_pubkey_hex = sess.keys.public_key().to_hex();

        let mut sender_names: HashMap<String, String> = entry
            .members
            .iter()
            .filter_map(|(pk, name, _)| {
                let hex = pk.to_hex();
                let display = name
                    .clone()
                    .or_else(|| self.profiles.get(&hex).and_then(|p| p.name.clone()));
                display.map(|n| (hex, n))
            })
            .collect();

        let my_name = &self.state.my_profile.name;
        if !my_name.is_empty() {
            sender_names.insert(my_pubkey_hex.clone(), my_name.clone());
        }

        let offset = *self.loaded_count.get(chat_id).unwrap_or(&0);
        let page = sess
            .mdk
            .get_messages(
                &entry.mls_group_id,
                Some(Pagination::new(Some(limit), Some(offset))),
            )
            .unwrap_or_default();

        if page.is_empty() {
            if let Some(cur) = self.state.current_chat.as_mut() {
                if cur.chat_id == chat_id {
                    cur.can_load_older = false;
                    self.emit_current_chat();
                }
            }
            return;
        }

        let fetched_len = page.len();

        let mut older: Vec<ChatMessage> = page
            .into_iter()
            .rev()
            .map(|m| {
                let id = m.id.to_hex();
                let sender_hex = m.pubkey.to_hex();
                let is_mine = sender_hex == my_pubkey_hex;
                let sender_name = sender_names.get(&sender_hex).cloned();
                let delivery = self
                    .delivery_overrides
                    .get(chat_id)
                    .and_then(|map| map.get(&id))
                    .cloned()
                    .unwrap_or(MessageDeliveryState::Sent);
                let (display_content, mentions) = resolve_mentions(&m.content, &sender_names);
                ChatMessage {
                    id,
                    sender_pubkey: sender_hex,
                    sender_name,
                    content: m.content,
                    display_content,
                    mentions,
                    timestamp: m.created_at.as_secs() as i64,
                    is_mine,
                    delivery,
                    reactions: vec![],
                    poll_tally: vec![],
                    my_poll_vote: None,
                }
            })
            .collect();

        if let Some(cur) = self.state.current_chat.as_mut() {
            if cur.chat_id == chat_id {
                older.append(&mut cur.messages);
                cur.messages = older;
                cur.can_load_older = fetched_len == limit;
                self.loaded_count
                    .insert(chat_id.to_string(), offset + fetched_len);
                process_html_updates(&mut cur.messages);
                process_poll_tallies(&mut cur.messages, &my_pubkey_hex);
                self.emit_current_chat();
            }
        }
    }
}

/// Parse a `pika-prompt-response` code block from message content.
/// Returns `(prompt_id, selected_option)` if found.
fn parse_poll_response(content: &str) -> Option<(String, String)> {
    let marker = "```pika-prompt-response\n";
    let start = content.find(marker)?;
    let json_start = start + marker.len();
    let rest = &content[json_start..];
    let end = rest.find("```")?;
    let json_str = rest[..end].trim();
    let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let prompt_id = v.get("prompt_id")?.as_str()?.to_string();
    let selected = v.get("selected")?.as_str()?.to_string();
    Some((prompt_id, selected))
}

/// Scan messages for `pika-prompt-response` blocks, tally votes onto
/// the matching prompt messages, and remove the response messages.
fn process_poll_tallies(msgs: &mut Vec<ChatMessage>, my_pubkey_hex: &str) {
    // Collect votes: (prompt_id, sender_pubkey) -> (option, timestamp, sender_name)
    // Only keep the latest vote per (sender, prompt_id).
    let mut latest_votes: HashMap<(String, String), (String, i64, Option<String>)> = HashMap::new();
    let mut response_indices: Vec<usize> = Vec::new();

    for (i, msg) in msgs.iter().enumerate() {
        if let Some((prompt_id, selected)) = parse_poll_response(&msg.content) {
            let key = (prompt_id, msg.sender_pubkey.clone());
            if latest_votes
                .get(&key)
                .map(|(_, ts, _)| msg.timestamp > *ts)
                .unwrap_or(true)
            {
                latest_votes.insert(key, (selected, msg.timestamp, msg.sender_name.clone()));
            }
            response_indices.push(i);
        }
    }

    if response_indices.is_empty() {
        return;
    }

    // Build tallies: prompt_id -> option -> Vec<voter_name>
    let mut tallies: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    let mut my_votes: HashMap<String, String> = HashMap::new();

    for ((prompt_id, sender_pubkey), (option, _, sender_name)) in &latest_votes {
        let voter_name = sender_name
            .clone()
            .unwrap_or_else(|| sender_pubkey[..sender_pubkey.len().min(8)].to_string());
        tallies
            .entry(prompt_id.clone())
            .or_default()
            .entry(option.clone())
            .or_default()
            .push(voter_name);
        if sender_pubkey == my_pubkey_hex {
            my_votes.insert(prompt_id.clone(), option.clone());
        }
    }

    // Attach tallies to matching prompt messages.
    for msg in msgs.iter_mut() {
        if let Some(option_tallies) = tallies.get(&msg.id) {
            let mut poll_tally: Vec<PollTally> = option_tallies
                .iter()
                .map(|(option, voters)| PollTally {
                    option: option.clone(),
                    count: voters.len() as u32,
                    voter_names: voters.clone(),
                })
                .collect();
            poll_tally.sort_by(|a, b| b.count.cmp(&a.count));
            msg.poll_tally = poll_tally;
            msg.my_poll_vote = my_votes.get(&msg.id).cloned();
        }
    }

    // Remove response messages (reverse order to preserve indices).
    for i in response_indices.into_iter().rev() {
        msgs.remove(i);
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
            let dominated = latest_updates
                .get(&target_id)
                .map(|(_, ts)| msg.timestamp > *ts)
                .unwrap_or(true);
            if dominated {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::MessageDeliveryState;

    fn make_msg(id: &str, content: &str, timestamp: i64) -> ChatMessage {
        ChatMessage {
            id: id.to_string(),
            sender_pubkey: "aabb".to_string(),
            sender_name: None,
            content: content.to_string(),
            display_content: content.to_string(),
            mentions: vec![],
            timestamp,
            is_mine: false,
            delivery: MessageDeliveryState::Sent,
            reactions: vec![],
            poll_tally: vec![],
            my_poll_vote: None,
        }
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
}
