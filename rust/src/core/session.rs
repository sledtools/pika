// Session lifecycle + networking side effects.

use super::*;

impl AppCore {
    pub(super) fn start_session(&mut self, keys: Keys) -> anyhow::Result<()> {
        // Tear down any existing session first.
        self.stop_session();

        // Ensure profile pics directory exists (may have been cleared on logout).
        profile_pics::ensure_dir(&self.data_dir);

        let pubkey = keys.public_key();
        let pubkey_hex = pubkey.to_hex();
        let npub = pubkey.to_bech32().unwrap_or(pubkey_hex.clone());

        tracing::info!(pubkey = %pubkey_hex, npub = %npub, "start_session");

        // MDK per-identity encrypted sqlite DB.
        let mdk = open_mdk(&self.data_dir, &pubkey, &self.keychain_group)?;
        tracing::info!("mdk opened");

        let client = Client::new(keys.clone());

        if self.network_enabled() {
            let relays = self.all_session_relays();
            tracing::info!(relays = ?relays.iter().map(|r| r.to_string()).collect::<Vec<_>>(), "connecting_relays");
            let c = client.clone();
            self.runtime.spawn(async move {
                for r in relays {
                    let _ = c.add_relay(r).await;
                }
                c.connect().await;
            });
            tracing::info!("relays connect scheduled");
        }

        let sess = Session {
            keys: keys.clone(),
            mdk,
            client: client.clone(),
            alive: Arc::new(AtomicBool::new(true)),
            giftwrap_sub: None,
            group_sub: None,
            groups: HashMap::new(),
        };

        self.session = Some(sess);

        self.state.auth = AuthState::LoggedIn {
            npub,
            pubkey: pubkey_hex.clone(),
        };

        // Build own profile from cached data (already loaded from DB in AppCore::new).
        self.state.my_profile = self.my_profile_state();
        self.emit_auth();
        self.handle_auth_transition(true);

        // Start notifications processing (async -> internal events).
        if self.network_enabled() {
            self.start_notifications_loop();
        }

        // Build the chat list. Profiles are already in memory, so names and
        // cached picture URLs will be present from the first emission.
        self.load_archived_chats();
        self.refresh_all_from_storage();
        self.cache_missing_profile_pics();
        self.refresh_my_profile(false);
        self.refresh_follow_list();

        if self.network_enabled() {
            self.publish_key_package_relays_best_effort();
            self.ensure_key_package_published_best_effort();
            self.recompute_subscriptions();
        }

        self.register_push_device();

        Ok(())
    }

    /// Re-open the MDK database to pick up any ratchet state changes made by the
    /// Notification Service Extension while the app was in the background.
    pub(super) fn reopen_mdk(&mut self) {
        let Some(sess) = self.session.as_mut() else {
            return;
        };
        match open_mdk(
            &self.data_dir,
            &sess.keys.public_key(),
            &self.keychain_group,
        ) {
            Ok(new_mdk) => {
                sess.mdk = new_mdk;
            }
            Err(e) => {
                tracing::warn!(%e, "failed to reopen mdk");
            }
        }
    }

    pub(super) fn stop_session(&mut self) {
        // Invalidate/stop any in-flight subscription recompute tasks.
        self.subs_recompute_token = self.subs_recompute_token.wrapping_add(1);
        self.subs_recompute_in_flight = false;
        self.subs_recompute_dirty = false;

        if let Some(sess) = self.session.take() {
            sess.alive.store(false, Ordering::SeqCst);
            if self.network_enabled() {
                let client = sess.client.clone();
                self.runtime.spawn(async move {
                    client.unsubscribe_all().await;
                    client.shutdown().await;
                });
            }
        }
    }

    pub(super) fn start_notifications_loop(&mut self) {
        let Some(sess) = self.session.as_ref() else {
            return;
        };
        let mut rx = sess.client.notifications();
        let client = sess.client.clone();
        let tx = self.core_sender.clone();
        self.runtime.spawn(async move {
            // Relay pools can redeliver the same event id (reconnects, multi-relay fanout).
            // Keep a small bounded cache to avoid duplicate MDK processing and noisy logs.
            const SEEN_CAP: usize = 2048;
            let mut seen: HashSet<String> = HashSet::new();
            let mut seen_order: VecDeque<String> = VecDeque::new();

            loop {
                match rx.recv().await {
                    Ok(RelayPoolNotification::Event { event, .. }) => {
                        let ev: Event = (*event).clone();
                        let id_hex = ev.id.to_hex();
                        if seen.contains(&id_hex) {
                            continue;
                        }
                        seen.insert(id_hex.clone());
                        seen_order.push_back(id_hex);
                        if seen_order.len() > SEEN_CAP {
                            if let Some(old) = seen_order.pop_front() {
                                seen.remove(&old);
                            }
                        }

                        match ev.kind {
                            Kind::GiftWrap => {
                                match client.unwrap_gift_wrap(&ev).await {
                                    Ok(unwrapped) => {
                                        let _ = tx.send(CoreMsg::Internal(Box::new(
                                            InternalEvent::GiftWrapReceived {
                                                wrapper: ev,
                                                rumor: unwrapped.rumor,
                                            },
                                        )));
                                    }
                                    Err(_) => {
                                        // Ignore malformed/unreadable giftwrap.
                                    }
                                }
                            }
                            Kind::MlsGroupMessage => {
                                let _ = tx.send(CoreMsg::Internal(Box::new(
                                    InternalEvent::GroupMessageReceived { event: ev },
                                )));
                            }
                            _ => {}
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    pub(super) fn ensure_key_package_published_best_effort(&mut self) {
        let key_package_relays = self.key_package_relays();
        let mut publish_relay_set: BTreeSet<RelayUrl> = BTreeSet::new();
        for r in key_package_relays.iter().cloned() {
            publish_relay_set.insert(r);
        }
        for r in self.default_relays() {
            publish_relay_set.insert(r);
        }
        let publish_relays: Vec<RelayUrl> = publish_relay_set.into_iter().collect();
        let relays_for_tags = if key_package_relays.is_empty() {
            publish_relays.clone()
        } else {
            key_package_relays
        };
        let Some(sess) = self.session.as_mut() else {
            return;
        };
        let (content, tags, _hash_ref) = match sess
            .mdk
            .create_key_package_for_event(&sess.keys.public_key(), relays_for_tags)
        {
            Ok(v) => v,
            Err(e) => {
                self.toast(format!("Key package create failed: {e}"));
                return;
            }
        };

        let builder = EventBuilder::new(Kind::MlsKeyPackage, content).tags(tags);
        let event = match builder.sign_with_keys(&sess.keys) {
            Ok(e) => e,
            Err(e) => {
                self.toast(format!("Key package sign failed: {e}"));
                return;
            }
        };

        let client = sess.client.clone();
        let tx = self.core_sender.clone();
        self.runtime.spawn(async move {
            let relay_list: Vec<String> = publish_relays.iter().map(|r| r.to_string()).collect();
            if publish_relays.is_empty() {
                let _ = tx.send(CoreMsg::Internal(Box::new(
                    InternalEvent::KeyPackagePublished {
                        ok: false,
                        error: Some("no relays configured".to_string()),
                    },
                )));
                return;
            }

            for r in publish_relays.iter().cloned() {
                let _ = client.add_relay(r).await;
            }

            // Best-effort with retries:
            // - relays can be slow to connect right after login/restore
            // - some relays require NIP-42 auth before accepting protected events
            let mut last_err: Option<String> = None;
            for attempt in 0..6u8 {
                client.connect().await;
                client.wait_for_connection(Duration::from_secs(5)).await;

                match client.send_event_to(&publish_relays, &event).await {
                    Ok(output) if !output.success.is_empty() => {
                        let _ = tx.send(CoreMsg::Internal(Box::new(
                            InternalEvent::KeyPackagePublished {
                                ok: true,
                                error: None,
                            },
                        )));
                        return;
                    }
                    Ok(output) => {
                        let errors: Vec<&str> =
                            output.failed.values().map(|s| s.as_str()).collect();
                        let summary = if errors.is_empty() {
                            "no relay accepted event".to_string()
                        } else {
                            errors.join("; ")
                        };
                        let any_retryable = errors.iter().any(|e| {
                            e.contains("protected") || e.contains("auth") || e.contains("AUTH")
                        });
                        let maybe_not_connected = errors.iter().any(|e| {
                            e.contains("no relays")
                                || e.contains("not ready")
                                || e.contains("not connected")
                        });
                        last_err = Some(format!("{summary}; relays={}", relay_list.join(",")));
                        if !any_retryable && !maybe_not_connected {
                            break;
                        }
                    }
                    Err(e) => {
                        let es = e.to_string();
                        let maybe_not_connected = es.contains("no relays")
                            || es.contains("not ready")
                            || es.contains("not connected");
                        last_err = Some(format!("{es}; relays={}", relay_list.join(",")));
                        if !maybe_not_connected {
                            break;
                        }
                    }
                }
                let delay_ms = 250u64.saturating_mul(1u64 << attempt);
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
            let _ = tx.send(CoreMsg::Internal(Box::new(
                InternalEvent::KeyPackagePublished {
                    ok: false,
                    error: last_err,
                },
            )));
        });
    }

    pub(super) fn publish_key_package_relays_best_effort(&mut self) {
        let general_relays = self.default_relays();
        let kp_relays = self.key_package_relays();
        let Some(sess) = self.session.as_ref() else {
            return;
        };

        if general_relays.is_empty() || kp_relays.is_empty() {
            return;
        }

        let tags: Vec<Tag> = kp_relays.iter().cloned().map(Tag::relay).collect();

        let builder = EventBuilder::new(Kind::MlsKeyPackageRelays, "").tags(tags);
        let event = match builder.sign_with_keys(&sess.keys) {
            Ok(e) => e,
            Err(_) => return,
        };

        let client = sess.client.clone();
        self.runtime.spawn(async move {
            // Ensure general relays exist.
            for r in general_relays.iter().cloned() {
                let _ = client.add_relay(r).await;
            }
            client.connect().await;
            client.wait_for_connection(Duration::from_secs(4)).await;
            let _ = client.send_event_to(general_relays, &event).await;
        });
    }

    pub(super) fn recompute_subscriptions(&mut self) {
        let network_enabled = self.network_enabled();
        if !network_enabled {
            return;
        }
        if self.subs_recompute_in_flight {
            self.subs_recompute_dirty = true;
            return;
        }
        // Ensure the client is connected to all relays referenced by joined groups.
        // Without this, we may subscribe to #h filters but never actually see events because
        // the relay URLs were never added to the client pool.
        let mut needed_relays: Vec<RelayUrl> = self.all_session_relays();
        if let Some(sess) = self.session.as_ref() {
            for entry in sess.groups.values() {
                if let Ok(set) = sess.mdk.get_relays(&entry.mls_group_id) {
                    for r in set.into_iter() {
                        if !needed_relays.contains(&r) {
                            needed_relays.push(r);
                        }
                    }
                }
            }
        }

        let Some(sess) = self.session.as_mut() else {
            return;
        };

        self.subs_recompute_in_flight = true;
        self.subs_recompute_dirty = false;
        self.subs_recompute_token = self.subs_recompute_token.wrapping_add(1);
        let token = self.subs_recompute_token;

        let client = sess.client.clone();
        let tx = self.core_sender.clone();
        let my_hex = sess.keys.public_key().to_hex();
        let prev_giftwrap_sub = sess.giftwrap_sub.clone();
        let prev_group_sub = sess.group_sub.clone();
        let h_values: Vec<String> = sess.groups.keys().cloned().collect();
        let alive = sess.alive.clone();

        self.runtime.spawn(async move {
            // Session lifecycle guard: if the user logs out while this task is in-flight, avoid
            // side effects like reconnecting or re-subscribing for a dead session.
            if !alive.load(Ordering::SeqCst) {
                return;
            }
            for r in needed_relays {
                let _ = client.add_relay(r).await;
            }
            if !alive.load(Ordering::SeqCst) {
                return;
            }
            client.connect().await;
            client.wait_for_connection(Duration::from_secs(4)).await;
            if !alive.load(Ordering::SeqCst) {
                return;
            }

            // Tear down previous subscriptions for a clean recompute.
            if let Some(id) = prev_giftwrap_sub {
                let _ = client.unsubscribe(&id).await;
            }
            if let Some(id) = prev_group_sub {
                let _ = client.unsubscribe(&id).await;
            }
            if !alive.load(Ordering::SeqCst) {
                return;
            }

            // GiftWrap inbox subscription (kind GiftWrap, #p = me).
            // NOTE: Filter `pubkey` matches the event author; GiftWraps can be authored by anyone,
            // so we must filter by the recipient `p` tag (spec-v2).
            if !alive.load(Ordering::SeqCst) {
                return;
            }
            let gift_filter = Filter::new()
                .kind(Kind::GiftWrap)
                .custom_tags(SingleLetterTag::lowercase(Alphabet::P), vec![my_hex]);
            let giftwrap_sub = client
                .subscribe(gift_filter, None)
                .await
                .ok()
                .map(|o| o.val);

            // Group subscription: kind 445 filtered by #h for all joined groups.
            if !alive.load(Ordering::SeqCst) {
                return;
            }
            let group_sub = if h_values.is_empty() {
                None
            } else {
                let group_filter = Filter::new()
                    .kind(Kind::MlsGroupMessage)
                    .custom_tags(SingleLetterTag::lowercase(Alphabet::H), h_values);
                client
                    .subscribe(group_filter, None)
                    .await
                    .ok()
                    .map(|o| o.val)
            };

            if !alive.load(Ordering::SeqCst) {
                return;
            }
            let _ = tx.send(CoreMsg::Internal(Box::new(
                InternalEvent::SubscriptionsRecomputed {
                    token,
                    giftwrap_sub,
                    group_sub,
                },
            )));
        });
    }

    pub(super) fn publish_welcomes_to_peer(
        &mut self,
        peer_pubkey: PublicKey,
        welcome_rumors: Vec<UnsignedEvent>,
        relays: Vec<RelayUrl>,
    ) {
        let Some(sess) = self.session.as_ref() else {
            return;
        };
        let client = sess.client.clone();
        self.runtime.spawn(async move {
            for r in relays.iter().cloned() {
                let _ = client.add_relay(r).await;
            }
            client.connect().await;
            client.wait_for_connection(Duration::from_secs(4)).await;

            let expires = Timestamp::from_secs(Timestamp::now().as_secs() + 30 * 24 * 60 * 60);
            let tags = vec![Tag::expiration(expires)];
            for rumor in welcome_rumors {
                let _ = client
                    .gift_wrap_to(relays.clone(), &peer_pubkey, rumor, tags.clone())
                    .await;
            }
        });
    }

    pub(super) fn refresh_follow_list(&mut self) {
        if !self.is_logged_in() || !self.network_enabled() {
            return;
        }
        // Don't double-fetch.
        if self.state.busy.fetching_follow_list {
            return;
        }
        self.set_busy(|b| b.fetching_follow_list = true);

        let Some(sess) = self.session.as_ref() else {
            self.set_busy(|b| b.fetching_follow_list = false);
            return;
        };

        let my_pubkey = sess.keys.public_key();
        let client = sess.client.clone();
        let tx = self.core_sender.clone();
        let existing_profiles = self.profiles.clone();

        self.runtime.spawn(async move {
            let empty = |tx: &flume::Sender<CoreMsg>| {
                let _ = tx.send(CoreMsg::Internal(Box::new(
                    InternalEvent::FollowListFetched {
                        followed_pubkeys: vec![],
                        fetched_profiles: vec![],
                        checked_pubkeys: HashSet::new(),
                    },
                )));
            };

            // 1) Fetch kind 3 (ContactList) for the user's own pubkey.
            let contact_filter = Filter::new()
                .author(my_pubkey)
                .kind(Kind::ContactList)
                .limit(1);

            let contact_events = match client
                .fetch_events(contact_filter, Duration::from_secs(8))
                .await
            {
                Ok(evs) => evs,
                Err(e) => {
                    tracing::debug!(%e, "follow list fetch failed");
                    empty(&tx);
                    return;
                }
            };

            let newest = contact_events.into_iter().max_by_key(|e| e.created_at);
            let Some(contact_event) = newest else {
                empty(&tx);
                return;
            };

            // 2) Extract all `p` tags -> list of PublicKey.
            let followed_pubkeys: Vec<PublicKey> = contact_event
                .tags
                .iter()
                .filter_map(|tag| {
                    let values = tag.as_slice();
                    if values.first().map(|s| s.as_str()) == Some("p") {
                        values.get(1).and_then(|hex| PublicKey::from_hex(hex).ok())
                    } else {
                        None
                    }
                })
                .collect();

            if followed_pubkeys.is_empty() {
                empty(&tx);
                return;
            }

            // 3) Determine which profiles need fetching.
            let now = crate::state::now_seconds();
            let needs_fetch: Vec<PublicKey> = followed_pubkeys
                .iter()
                .filter(|pk| {
                    let hex = pk.to_hex();
                    match existing_profiles.get(&hex) {
                        None => true,
                        Some(p) => (now - p.last_checked_at) > 3600,
                    }
                })
                .cloned()
                .collect();

            // 4) Batch-fetch kind 0 (Metadata) for stale profiles.
            let mut fetched: Vec<(String, Option<String>, i64)> = Vec::new();
            if !needs_fetch.is_empty() {
                let profile_filter = Filter::new()
                    .authors(needs_fetch.clone())
                    .kind(Kind::Metadata)
                    .limit(needs_fetch.len());
                if let Ok(events) = client
                    .fetch_events(profile_filter, Duration::from_secs(10))
                    .await
                {
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
                    for (hex_pk, ev) in best {
                        let event_created_at = ev.created_at.as_secs() as i64;
                        fetched.push((hex_pk, Some(ev.content.clone()), event_created_at));
                    }
                }
            }

            let followed_hex: Vec<String> = followed_pubkeys.iter().map(|pk| pk.to_hex()).collect();
            let checked_pubkeys: HashSet<String> =
                needs_fetch.iter().map(|pk| pk.to_hex()).collect();
            let _ = tx.send(CoreMsg::Internal(Box::new(
                InternalEvent::FollowListFetched {
                    followed_pubkeys: followed_hex,
                    fetched_profiles: fetched,
                    checked_pubkeys,
                },
            )));
        });
    }

    pub(super) fn fetch_peer_profile(&mut self, pubkey_hex: &str) {
        if !self.is_logged_in() || !self.network_enabled() {
            return;
        }
        let Some(sess) = self.session.as_ref() else {
            return;
        };
        let pk = match PublicKey::from_hex(pubkey_hex) {
            Ok(pk) => pk,
            Err(_) => return,
        };
        let client = sess.client.clone();
        let tx = self.core_sender.clone();
        let pubkey_hex = pubkey_hex.to_string();

        self.runtime.spawn(async move {
            let filter = Filter::new().author(pk).kind(Kind::Metadata).limit(1);
            let best_event = client
                .fetch_events(filter, Duration::from_secs(8))
                .await
                .ok()
                .and_then(|evs| evs.into_iter().max_by_key(|e| e.created_at));

            let event_created_at = best_event
                .as_ref()
                .map(|e| e.created_at.as_secs() as i64)
                .unwrap_or(0);
            let metadata_json = best_event.map(|e| e.content.clone());

            let _ = tx.send(CoreMsg::Internal(Box::new(
                InternalEvent::PeerProfileFetched {
                    pubkey: pubkey_hex,
                    metadata_json,
                    event_created_at,
                },
            )));
        });
    }

    pub(super) fn follow_user(&mut self, pubkey_hex: &str) {
        self.modify_contact_list(pubkey_hex, true);
    }

    pub(super) fn unfollow_user(&mut self, pubkey_hex: &str) {
        self.modify_contact_list(pubkey_hex, false);
    }

    /// Safely modify the user's contact list (kind 3).
    ///
    /// CRITICAL: Kind 3 is a replaceable event -- publishing a new one
    /// completely replaces the old one. We MUST fetch the absolute latest
    /// version from relays before modifying, and REFUSE to publish if the
    /// fetch fails. All existing tags and content are preserved verbatim.
    fn modify_contact_list(&mut self, pubkey_hex: &str, add: bool) {
        if !self.is_logged_in() || !self.network_enabled() {
            return;
        }

        let target_pk = match PublicKey::from_hex(pubkey_hex) {
            Ok(pk) => pk,
            Err(_) => {
                self.toast("Invalid pubkey");
                return;
            }
        };

        // Extract session fields before mutable borrow for optimistic update.
        let (my_pubkey, client, keys) = {
            let Some(sess) = self.session.as_ref() else {
                return;
            };
            (
                sess.keys.public_key(),
                sess.client.clone(),
                sess.keys.clone(),
            )
        };

        // Optimistically update the peer_profile.is_followed flag.
        if let Some(ref mut pp) = self.state.peer_profile {
            if pp.pubkey == pubkey_hex {
                pp.is_followed = add;
                self.emit_state();
            }
        }

        let relays = self.default_relays();
        let tx = self.core_sender.clone();
        let action_label = if add { "follow" } else { "unfollow" };
        let pubkey_for_revert = pubkey_hex.to_string();

        self.runtime.spawn(async move {
            let revert = |tx: &Sender<CoreMsg>| {
                let _ = tx.send(CoreMsg::Internal(Box::new(
                    InternalEvent::ContactListModifyFailed {
                        pubkey: pubkey_for_revert.clone(),
                        revert_to: !add,
                    },
                )));
            };

            // SAFETY: Always fetch the latest contact list from relays.
            // Never use a cached version -- stale data would wipe follows.
            let filter = Filter::new()
                .author(my_pubkey)
                .kind(Kind::ContactList)
                .limit(1);
            let current = match client.fetch_events(filter, Duration::from_secs(10)).await {
                Ok(evs) => evs.into_iter().max_by_key(|e| e.created_at),
                Err(e) => {
                    tracing::error!(
                        %e, action = action_label,
                        "REFUSED to modify contact list: fetch failed, would risk wiping follows"
                    );
                    revert(&tx);
                    return;
                }
            };

            // Preserve ALL existing tags and content verbatim.
            let mut tags: Vec<Tag> = current
                .as_ref()
                .map(|e| e.tags.clone().to_vec())
                .unwrap_or_default();
            let content = current
                .as_ref()
                .map(|e| e.content.clone())
                .unwrap_or_default();

            let target_hex = target_pk.to_hex();

            if add {
                let already = tags.iter().any(|t| {
                    let v = t.as_slice();
                    v.first().map(|s| s.as_str()) == Some("p")
                        && v.get(1).map(|s| s.as_str()) == Some(target_hex.as_str())
                });
                if already {
                    return;
                }
                tags.push(Tag::public_key(target_pk));
                tracing::info!(
                    target = %target_hex,
                    total_follows = tags.iter()
                        .filter(|t| t.as_slice().first().map(|s| s.as_str()) == Some("p"))
                        .count(),
                    "adding follow"
                );
            } else {
                let before = tags.len();
                tags.retain(|t| {
                    let v = t.as_slice();
                    !(v.first().map(|s| s.as_str()) == Some("p")
                        && v.get(1).map(|s| s.as_str()) == Some(target_hex.as_str()))
                });
                if tags.len() == before {
                    return;
                }
                tracing::info!(
                    target = %target_hex,
                    total_follows = tags.iter()
                        .filter(|t| t.as_slice().first().map(|s| s.as_str()) == Some("p"))
                        .count(),
                    "removing follow"
                );
            }

            let event = match EventBuilder::new(Kind::ContactList, &content)
                .tags(tags)
                .sign_with_keys(&keys)
            {
                Ok(ev) => ev,
                Err(e) => {
                    tracing::error!(%e, "failed to build contact list event");
                    revert(&tx);
                    return;
                }
            };

            match client.send_event_to(relays, &event).await {
                Ok(output) if !output.success.is_empty() => {
                    tracing::info!(action = action_label, "contact list published");
                }
                Ok(output) => {
                    let err = output
                        .failed
                        .values()
                        .next()
                        .cloned()
                        .unwrap_or_else(|| "no relay accepted".into());
                    tracing::error!(action = action_label, %err, "contact list publish rejected");
                    revert(&tx);
                    return;
                }
                Err(e) => {
                    tracing::error!(%e, action = action_label, "contact list publish failed");
                    revert(&tx);
                    return;
                }
            }

            // Refresh follow list to sync UI.
            let _ = tx.send(CoreMsg::Action(AppAction::RefreshFollowList));
        });
    }

    pub(super) fn delete_event_best_effort(&mut self, id: EventId) {
        let Some(sess) = self.session.as_ref() else {
            return;
        };
        let client = sess.client.clone();
        let keys = sess.keys.clone();
        let relays = self.default_relays();
        self.runtime.spawn(async move {
            let req = EventDeletionRequest::new()
                .id(id)
                .reason("rotated key package");
            if let Ok(ev) = EventBuilder::delete(req).sign_with_keys(&keys) {
                let _ = client.send_event_to(relays, &ev).await;
            }
        });
    }
}
