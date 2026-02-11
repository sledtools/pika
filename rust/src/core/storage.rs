// Storage-derived state refresh + paging.

use super::*;

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
            // Map to chat_id = hex(nostr_group_id)
            let chat_id = hex::encode(g.nostr_group_id);

            // Determine peer for 1:1 (or note-to-self).
            let peer_pubkey = sess.mdk.get_members(&g.mls_group_id).ok().and_then(
                |members: BTreeSet<PublicKey>| members.into_iter().find(|p| p != &my_pubkey),
            );
            let peer_npub = peer_pubkey
                .as_ref()
                .and_then(|p| p.to_bech32().ok())
                .unwrap_or_else(|| my_pubkey.to_bech32().unwrap_or_else(|_| my_pubkey.to_hex()));

            // Look up cached Nostr profile for peer.
            let peer_hex = peer_pubkey
                .as_ref()
                .map(|p| p.to_hex())
                .unwrap_or_else(|| my_pubkey.to_hex());
            let cached_profile = self.profiles.get(&peer_hex);
            let peer_name = cached_profile.and_then(|p| p.name.clone());
            let peer_picture_url = cached_profile.and_then(|p| p.picture_url.clone());

            // Track peers missing from cache (or stale > 1 hour) for async fetch.
            let now = crate::state::now_seconds();
            let needs_fetch = match cached_profile {
                None => true,
                Some(p) => (now - p.fetched_at) > 3600,
            };
            if needs_fetch {
                if let Some(pk) = peer_pubkey.as_ref() {
                    if !missing_profile_pubkeys.iter().any(|p| p == pk) {
                        missing_profile_pubkeys.push(*pk);
                    }
                }
            }

            // Do not rely on `last_message_id` being populated in all MDK flows.
            // For MVP scale, fetching the newest message per group is cheap and robust.
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

            // Merge with local optimistic outbox (if any). If storage doesn't show the new message
            // yet, we still want chat list previews to update immediately.
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

            list.push(ChatSummary {
                chat_id: chat_id.clone(),
                peer_npub: peer_npub.clone(),
                peer_name: peer_name.clone(),
                peer_picture_url: peer_picture_url.clone(),
                last_message,
                last_message_at,
                unread_count,
            });

            index.insert(
                chat_id,
                GroupIndexEntry {
                    mls_group_id: g.mls_group_id,
                    peer_npub,
                    peer_name,
                    peer_picture_url,
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

        // Default initial load: newest 50, and preserve paging by reloading the already-loaded count.
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
        // MDK returns descending by created_at; UI wants ascending.
        let mut msgs: Vec<ChatMessage> = messages
            .into_iter()
            .rev()
            .map(|m| {
                let id = m.id.to_hex();
                let is_mine = m.pubkey == sess.keys.public_key();
                let delivery = self
                    .delivery_overrides
                    .get(chat_id)
                    .and_then(|map| map.get(&id))
                    .cloned()
                    .unwrap_or(MessageDeliveryState::Sent);
                ChatMessage {
                    id,
                    sender_pubkey: m.pubkey.to_hex(),
                    content: m.content,
                    timestamp: m.created_at.as_secs() as i64,
                    is_mine,
                    delivery,
                }
            })
            .collect();

        // Add optimistic local messages not yet visible through MDK storage.
        //
        // Important: do not inject messages older than the oldest storage-backed message in the
        // current window, or we'd break paging by showing older content "for free".
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
                msgs.push(ChatMessage {
                    id,
                    sender_pubkey: lm.sender_pubkey,
                    content: lm.content,
                    timestamp: lm.timestamp,
                    is_mine: true,
                    delivery,
                });
            }
            msgs.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.id.cmp(&b.id)));
        }

        // Prune optimistic outbox entries once they show up in storage-backed messages, and also
        // drop anything older than the current loaded storage window (paging correctness).
        if let Some(local) = self.local_outbox.get_mut(chat_id) {
            local.retain(|id, lm| !present_ids.contains(id) && lm.timestamp >= oldest_loaded_ts);
        }

        let can_load_older = storage_len == limit;
        // loaded_count tracks the number of storage-backed messages loaded (used for paging offsets).
        self.loaded_count.insert(chat_id.to_string(), storage_len);

        self.state.current_chat = Some(ChatViewState {
            chat_id: chat_id.to_string(),
            peer_npub: entry.peer_npub,
            peer_name: entry.peer_name,
            peer_picture_url: entry.peer_picture_url,
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

        // Reverse page to ascending.
        let mut older: Vec<ChatMessage> = page
            .into_iter()
            .rev()
            .map(|m| {
                let id = m.id.to_hex();
                let is_mine = m.pubkey == sess.keys.public_key();
                let delivery = self
                    .delivery_overrides
                    .get(chat_id)
                    .and_then(|map| map.get(&id))
                    .cloned()
                    .unwrap_or(MessageDeliveryState::Sent);
                ChatMessage {
                    id,
                    sender_pubkey: m.pubkey.to_hex(),
                    content: m.content,
                    timestamp: m.created_at.as_secs() as i64,
                    is_mine,
                    delivery,
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
                self.emit_current_chat();
            }
        }
    }
}
