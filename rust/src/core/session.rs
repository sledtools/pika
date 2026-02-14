// Session lifecycle + networking side effects.

use super::*;

impl AppCore {
    pub(super) fn start_session(&mut self, keys: Keys) -> anyhow::Result<()> {
        // Tear down any existing session first.
        self.stop_session();

        let pubkey = keys.public_key();
        let pubkey_hex = pubkey.to_hex();
        let npub = pubkey.to_bech32().unwrap_or(pubkey_hex.clone());

        tracing::info!(pubkey = %pubkey_hex, npub = %npub, "start_session");

        // MDK per-identity encrypted sqlite DB.
        let mdk = open_mdk(&self.data_dir, &pubkey)?;
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
            pubkey: pubkey_hex,
        };
        self.my_metadata = None;
        self.state.my_profile = crate::state::MyProfileState::empty();
        self.emit_auth();
        self.handle_auth_transition(true);

        // Start notifications processing (async -> internal events).
        if self.network_enabled() {
            self.start_notifications_loop();
        }

        self.refresh_all_from_storage();
        self.refresh_my_profile(false);
        self.refresh_follow_list();

        if self.network_enabled() {
            self.publish_key_package_relays_best_effort();
            self.ensure_key_package_published_best_effort();
            self.recompute_subscriptions();
        }

        Ok(())
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
                    Ok(RelayPoolNotification::Message { relay_url, message }) => {
                        // NIP-42 auth is required by many relays to publish NIP-70 "protected" events.
                        // MDK marks key packages (kind 443) as protected, so we must respond to AUTH
                        // challenges or publishing will be rejected ("blocked: event marked as protected").
                        if let RelayMessage::Auth { challenge } = message {
                            // nostr-sdk 0.44 doesn't expose a `Client::auth` helper; build/sign/send.
                            if let Ok(event) = client
                                .sign_event_builder(EventBuilder::auth(
                                    challenge,
                                    relay_url.clone(),
                                ))
                                .await
                            {
                                let _ = client
                                    .send_msg_to([relay_url], ClientMessage::auth(event))
                                    .await;
                            }
                        }
                    }
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
        let relays = self.key_package_relays();
        let Some(sess) = self.session.as_mut() else {
            return;
        };
        let (content, tags) = match sess
            .mdk
            .create_key_package_for_event(&sess.keys.public_key(), relays.clone())
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
            // Ensure these relays exist in the pool. (Session startup adds defaults, but config can change.)
            for r in relays.iter().cloned() {
                let _ = client.add_relay(r).await;
            }
            client.connect().await;
            client.wait_for_connection(Duration::from_secs(4)).await;

            // Best-effort with retries: some relays require NIP-42 auth before accepting
            // protected events. They will emit an AUTH challenge; we respond in the
            // notifications loop, then retry publishing.
            let mut last_err: Option<String> = None;
            for attempt in 0..5u8 {
                let out = client.send_event_to(&relays, &event).await;
                match out {
                    Ok(output) => {
                        if !output.success.is_empty() {
                            let _ = tx.send(CoreMsg::Internal(Box::new(
                                InternalEvent::KeyPackagePublished {
                                    ok: true,
                                    error: None,
                                },
                            )));
                            return;
                        }
                        // Aggregate a representative error string.
                        let err = output
                            .failed
                            .values()
                            .next()
                            .cloned()
                            .unwrap_or_else(|| "no relay accepted event".into());
                        last_err = Some(err.clone());

                        // Retry on common transient causes (auth-required / policy blocks).
                        // This keeps v2 usable on relays that require a NIP-42 AUTH handshake
                        // before accepting NIP-70 protected events.
                        let should_retry = err.contains("event marked as protected")
                            || err.contains("protected")
                            || err.contains("auth")
                            || err.contains("AUTH");
                        if !should_retry {
                            break;
                        }
                    }
                    Err(e) => {
                        last_err = Some(e.to_string());
                    }
                }

                // Backoff: 250ms, 500ms, 1s, 2s, 4s (bounded).
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
                    let _ = tx.send(CoreMsg::Internal(Box::new(
                        InternalEvent::FollowListFetched { entries: vec![] },
                    )));
                    return;
                }
            };

            // Pick the newest kind 3 event.
            let newest = contact_events.into_iter().max_by_key(|e| e.created_at);
            let Some(contact_event) = newest else {
                // No contact list published yet.
                let _ = tx.send(CoreMsg::Internal(Box::new(
                    InternalEvent::FollowListFetched { entries: vec![] },
                )));
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
                let _ = tx.send(CoreMsg::Internal(Box::new(
                    InternalEvent::FollowListFetched { entries: vec![] },
                )));
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
                        Some(p) => (now - p.fetched_at) > 3600,
                    }
                })
                .cloned()
                .collect();

            // 4) Batch-fetch kind 0 (Metadata) for missing profiles.
            let mut fetched_profiles: HashMap<String, (Option<String>, Option<String>)> =
                HashMap::new();
            if !needs_fetch.is_empty() {
                let profile_filter = Filter::new()
                    .authors(needs_fetch.clone())
                    .kind(Kind::Metadata)
                    .limit(needs_fetch.len());
                if let Ok(events) = client
                    .fetch_events(profile_filter, Duration::from_secs(10))
                    .await
                {
                    // Keep only newest per author.
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
                                    (display_name.or(name), picture)
                                });
                        let (name, picture) = parsed.unwrap_or((None, None));
                        fetched_profiles.insert(hex_pk, (name, picture));
                    }
                }
            }

            // 5) Build result entries combining cached + freshly fetched profiles.
            let entries: Vec<(String, Option<String>, Option<String>)> = followed_pubkeys
                .iter()
                .map(|pk| {
                    let hex = pk.to_hex();
                    if let Some((name, picture)) = fetched_profiles.get(&hex) {
                        (hex, name.clone(), picture.clone())
                    } else if let Some(cached) = existing_profiles.get(&hex) {
                        (hex, cached.name.clone(), cached.picture_url.clone())
                    } else {
                        (hex, None, None)
                    }
                })
                .collect();

            let _ = tx.send(CoreMsg::Internal(Box::new(
                InternalEvent::FollowListFetched { entries },
            )));
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
