// Session lifecycle + networking side effects.

use std::future::Future;

use super::*;
#[cfg(test)]
use pika_marmot_runtime::runtime::RuntimeGroupSubscriptionPlan;
use pika_marmot_runtime::runtime::{
    bootstrap_runtime_session, classify_inbound_relay_event, connect_runtime_relays,
    subscribe_group_messages_combined, subscribe_welcome_inbox,
    temporary_client_from_session_signer, BootstrappedRuntimeSession, InboundRelayEvent,
    InboundRelaySeenCache, RuntimeSessionOpenRequest, RuntimeSessionSyncPlan,
    RuntimeWelcomeInboxSubscriptionIntent,
};
use pika_marmot_runtime::welcome::publish_welcome_rumors;

fn app_open_request(
    long_lived_session_relays: Vec<RelayUrl>,
    temporary_key_package_relays: Vec<RelayUrl>,
) -> RuntimeSessionOpenRequest {
    RuntimeSessionOpenRequest {
        subscribed_group_ids: Vec::new(),
        long_lived_session_relays,
        temporary_key_package_relays,
        welcome_inbox: app_welcome_inbox_intent(),
    }
}

fn bootstrap_runtime_for_app(
    data_dir: &str,
    keychain_group: &str,
    pubkey: PublicKey,
    signer: Arc<dyn NostrSigner>,
    long_lived_session_relays: Vec<RelayUrl>,
    temporary_key_package_relays: Vec<RelayUrl>,
) -> anyhow::Result<BootstrappedRuntimeSession> {
    bootstrap_runtime_session(
        pubkey,
        signer,
        || open_mdk(data_dir, &pubkey, keychain_group),
        app_open_request(long_lived_session_relays, temporary_key_package_relays),
    )
}

async fn classify_app_notification_event(
    client: &Client,
    seen: &mut InboundRelaySeenCache,
    event: Event,
) -> anyhow::Result<Option<InternalEvent>> {
    match classify_inbound_relay_event(client, seen, event).await? {
        Some(InboundRelayEvent::Welcome { wrapper, rumor, .. }) => {
            Ok(Some(InternalEvent::GiftWrapReceived { wrapper, rumor }))
        }
        Some(InboundRelayEvent::GroupMessage { event }) => {
            Ok(Some(InternalEvent::GroupMessageReceived { event }))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
fn plan_app_group_subscriptions(sess: &Session) -> anyhow::Result<RuntimeGroupSubscriptionPlan> {
    sess.host_context()
        .plan_group_subscriptions(sess.groups.keys().cloned().collect())
}

fn app_welcome_inbox_intent() -> RuntimeWelcomeInboxSubscriptionIntent {
    RuntimeWelcomeInboxSubscriptionIntent::default()
}

fn seed_app_groups_from_open_state(
    local_pubkey: &PublicKey,
    snapshots: &[pika_marmot_runtime::conversation::RuntimeJoinedGroupSnapshot],
) -> HashMap<String, GroupIndexEntry> {
    snapshots
        .iter()
        .map(|snapshot| {
            let other_members = snapshot.other_member_snapshots(local_pubkey);
            let explicit_name = if snapshot.name != DEFAULT_GROUP_NAME && !snapshot.name.is_empty()
            {
                Some(snapshot.name.clone())
            } else {
                None
            };
            let is_group =
                other_members.len() > 1 || (explicit_name.is_some() && !other_members.is_empty());
            let members = other_members
                .into_iter()
                .map(|member| GroupMember {
                    pubkey: member.pubkey,
                    is_admin: member.is_admin,
                    name: None,
                    picture_url: None,
                })
                .collect();
            (
                snapshot.nostr_group_id_hex.clone(),
                GroupIndexEntry {
                    mls_group_id: snapshot.mls_group_id.clone(),
                    is_group,
                    group_name: explicit_name,
                    self_is_admin: snapshot.is_admin(local_pubkey),
                    members,
                },
            )
        })
        .collect()
}

fn plan_app_session_sync(core: &AppCore, sess: &Session) -> anyhow::Result<RuntimeSessionSyncPlan> {
    sess.host_context().plan_session_sync(
        sess.groups.keys().cloned().collect(),
        core.long_lived_session_relays(),
        core.temporary_key_package_relays(),
        app_welcome_inbox_intent(),
    )
}

impl AppCore {
    pub(super) fn start_session(&mut self, keys: Keys) -> anyhow::Result<()> {
        let pubkey = keys.public_key();
        self.start_session_with_signer(
            pubkey,
            Arc::new(keys.clone()),
            Some(keys),
            SessionAuthMode::LocalNsec,
        )
    }

    pub(super) fn start_session_with_signer(
        &mut self,
        pubkey: PublicKey,
        signer: Arc<dyn NostrSigner>,
        local_keys: Option<Keys>,
        auth_mode: SessionAuthMode,
    ) -> anyhow::Result<()> {
        // Tear down any existing session first.
        self.stop_session();

        // Ensure profile pics directory exists (may have been cleared on logout).
        profile_pics::ensure_dir(&self.data_dir);

        let pubkey_hex = pubkey.to_hex();
        let npub = pubkey.to_bech32().unwrap_or(pubkey_hex.clone());

        tracing::info!(pubkey = %pubkey_hex, npub = %npub, "start_session");

        let bootstrapped = bootstrap_runtime_for_app(
            &self.data_dir,
            &self.keychain_group,
            pubkey,
            signer,
            self.long_lived_session_relays(),
            self.temporary_key_package_relays(),
        )?;
        tracing::info!("mdk opened");
        let initial_sync_plan = bootstrapped.open.sync_plan.clone();
        let initial_groups =
            seed_app_groups_from_open_state(&pubkey, &bootstrapped.open.joined_group_snapshots);
        let initial_seen_welcomes = bootstrapped.startup.seen_welcomes;
        let runtime_session = bootstrapped.session;

        if self.network_enabled() {
            let relays = initial_sync_plan.relay_roles.session_connect_relays.clone();
            tracing::info!(relays = ?relays.iter().map(|r| r.to_string()).collect::<Vec<_>>(), "connecting_relays");
            let client = runtime_session.client.clone();
            self.runtime.spawn(async move {
                connect_runtime_relays(&client, &relays, false, None).await;
            });
            tracing::info!("relays connect scheduled");
        }

        let sess = Session {
            pubkey: runtime_session.pubkey,
            local_keys,
            mdk: runtime_session.mdk,
            client: runtime_session.client,
            alive: Arc::new(AtomicBool::new(true)),
            giftwrap_sub: None,
            group_sub: None,
            groups: initial_groups,
        };

        self.session = Some(sess);

        self.state.auth = AuthState::LoggedIn {
            npub,
            pubkey: pubkey_hex.clone(),
            mode: auth_mode.to_state_mode(&pubkey_hex),
        };

        // Build own profile from cached data (already loaded from DB in AppCore::new).
        self.state.my_profile = self.my_profile_state();
        self.emit_auth();
        self.handle_auth_transition(true);
        self.refresh_agent_allowlist();

        // Start notifications processing (async -> internal events).
        if self.network_enabled() {
            self.start_notifications_loop(initial_seen_welcomes);
        }

        // Build the chat list. Profiles are already in memory, so names and
        // cached picture URLs will be present from the first emission.
        self.load_archived_chats();
        self.load_call_timeline();
        self.refresh_all_from_storage();

        // Defer remaining init work so any user actions that queued while the
        // actor was busy (e.g. chat taps during loading) are processed first.
        self.deferred_session_init_pending = true;
        let _ = self.core_sender.send(CoreMsg::Internal(Box::new(
            InternalEvent::CompleteSessionInit,
        )));

        Ok(())
    }

    /// Re-open the MDK database to pick up any ratchet state changes made by the
    /// Notification Service Extension while the app was in the background.
    pub(super) fn reopen_mdk(&mut self) {
        let Some(sess) = self.session.as_mut() else {
            return;
        };
        match open_mdk(&self.data_dir, &sess.pubkey, &self.keychain_group) {
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
        self.subs_force_reconnect = false;
        self.invalidate_agent_flow();
        self.invalidate_key_package_publish();
        self.invalidate_direct_chat_creation();
        self.pending_direct_chat_creation = None;
        self.agent_allowlist_state = AgentAllowlistState::Unknown;
        self.invalidate_agent_allowlist_probe();
        self.state.agent_button = None;
        self.state.agent_provisioning = None;
        self.agent_flow_start = None;
        self.state
            .router
            .screen_stack
            .retain(|s| !matches!(s, Screen::AgentProvisioning));
        self.group_profiles.clear();

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

    pub(super) fn start_notifications_loop(&mut self, initial_seen_welcomes: HashSet<EventId>) {
        let Some(sess) = self.session.as_ref() else {
            return;
        };
        let mut rx = sess.client.notifications();
        let client = sess.client.clone();
        let tx = self.core_sender.clone();
        self.runtime.spawn(async move {
            let mut seen = InboundRelaySeenCache::default();
            seen.extend(initial_seen_welcomes);

            loop {
                match rx.recv().await {
                    Ok(RelayPoolNotification::Event { event, .. }) => {
                        let ev: Event = (*event).clone();
                        match classify_app_notification_event(&client, &mut seen, ev).await {
                            Ok(Some(internal)) => {
                                let _ = tx.send(CoreMsg::Internal(Box::new(internal)));
                            }
                            Ok(None) => {}
                            Err(e) => {
                                tracing::warn!("notification ingress failed: {e:#}");
                            }
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
        let relay_roles = self.relay_role_plan(Vec::new());
        let publish_relays = relay_roles.key_package_operation_relays;
        if publish_relays.is_empty() {
            return;
        }
        let relays_for_tags = if relay_roles.temporary_key_package_relays.is_empty() {
            publish_relays.clone()
        } else {
            relay_roles.temporary_key_package_relays
        };
        let Some(sess) = self.session.as_mut() else {
            return;
        };
        self.key_package_publish_token = self.key_package_publish_token.wrapping_add(1);
        let token = self.key_package_publish_token;
        self.local_key_package_published = false;
        let (content, tags, _hash_ref) = match sess
            .mdk
            .create_key_package_for_event(&sess.pubkey, relays_for_tags)
        {
            Ok(v) => v,
            Err(e) => {
                self.fail_direct_chat_creation(format!("Key package create failed: {e}"));
                return;
            }
        };

        // Disable NIP-70 for now: strip the protected marker before publish.
        let tags: Tags = tags
            .into_iter()
            .filter(|t: &Tag| !matches!(t.kind(), TagKind::Protected))
            .collect();
        let builder = EventBuilder::new(Kind::MlsKeyPackage, content).tags(tags);

        let session_client = sess.client.clone();
        let tx = self.core_sender.clone();
        self.runtime.spawn(async move {
            let client =
                match temporary_client_from_session_signer(&session_client, "key package publish")
                    .await
                {
                    Ok(client) => client,
                    Err(e) => {
                        let _ = tx.send(CoreMsg::Internal(Box::new(
                            InternalEvent::KeyPackagePublished {
                                token,
                                ok: false,
                                error: Some(format!("key package publish client failed: {e:#}")),
                            },
                        )));
                        return;
                    }
                };
            let event = match client.sign_event_builder(builder).await {
                Ok(e) => e,
                Err(e) => {
                    client.shutdown().await;
                    let _ = tx.send(CoreMsg::Internal(Box::new(
                        InternalEvent::KeyPackagePublished {
                            token,
                            ok: false,
                            error: Some(format!("key package sign failed: {e}")),
                        },
                    )));
                    return;
                }
            };

            for r in publish_relays.iter().cloned() {
                let _ = client.add_relay(r).await;
            }

            let outcome = super::relay_publish::publish_event_with_retry(
                &client,
                &publish_relays,
                &event,
                6,
                "key package publish",
                true,
            )
            .await;
            client.shutdown().await;
            match outcome {
                super::relay_publish::PublishOutcome::Ok => {
                    let _ = tx.send(CoreMsg::Internal(Box::new(
                        InternalEvent::KeyPackagePublished {
                            token,
                            ok: true,
                            error: None,
                        },
                    )));
                }
                super::relay_publish::PublishOutcome::Err(err) => {
                    let _ = tx.send(CoreMsg::Internal(Box::new(
                        InternalEvent::KeyPackagePublished {
                            token,
                            ok: false,
                            error: Some(err),
                        },
                    )));
                }
            }
        });
    }

    pub(super) fn publish_key_package_relays_best_effort(&mut self) {
        let general_relays = self.long_lived_session_relays();
        let kp_relays = self.temporary_key_package_relays();
        let Some(sess) = self.session.as_ref() else {
            return;
        };

        if general_relays.is_empty() || kp_relays.is_empty() {
            return;
        }

        let tags: Vec<Tag> = kp_relays.iter().cloned().map(Tag::relay).collect();

        let client = sess.client.clone();
        self.runtime.spawn(async move {
            let builder = EventBuilder::new(Kind::MlsKeyPackageRelays, "").tags(tags);
            let event = match client.sign_event_builder(builder).await {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(%e, "key package relays sign failed");
                    return;
                }
            };

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
        let Some(sess) = self.session.as_ref() else {
            return;
        };
        let sync_plan = match plan_app_session_sync(self, sess) {
            Ok(plan) => plan,
            Err(err) => {
                tracing::warn!(%err, "failed to plan app session sync");
                return;
            }
        };
        let needed_relays = sync_plan.relay_roles.session_connect_relays;
        let Some(sess) = self.session.as_mut() else {
            return;
        };

        self.subs_recompute_in_flight = true;
        self.subs_recompute_dirty = false;
        let force_reconnect = std::mem::take(&mut self.subs_force_reconnect);
        self.subs_recompute_token = self.subs_recompute_token.wrapping_add(1);
        let token = self.subs_recompute_token;

        let client = sess.client.clone();
        let tx = self.core_sender.clone();
        let pubkey = sess.pubkey;
        let prev_giftwrap_sub = sess.giftwrap_sub.clone();
        let prev_group_sub = sess.group_sub.clone();
        let h_values = sync_plan.group_subscriptions.current.target_group_ids;
        let welcome_inbox = sync_plan.welcome_inbox;
        let alive = sess.alive.clone();

        self.runtime.spawn(async move {
            // Session lifecycle guard: if the user logs out while this task is in-flight, avoid
            // side effects like reconnecting or re-subscribing for a dead session.
            if !alive.load(Ordering::SeqCst) {
                return;
            }
            if !alive.load(Ordering::SeqCst) {
                return;
            }
            connect_runtime_relays(
                &client,
                &needed_relays,
                force_reconnect,
                Some(Duration::from_secs(4)),
            )
            .await;
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
            let giftwrap_sub = subscribe_welcome_inbox(
                &client,
                pubkey,
                welcome_inbox.lookback,
                welcome_inbox.limit,
            )
            .await
            .ok();

            // Group subscription: kind 445 filtered by #h for all joined groups.
            if !alive.load(Ordering::SeqCst) {
                return;
            }
            let group_sub = subscribe_group_messages_combined(&client, &h_values)
                .await
                .ok()
                .flatten();

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

    pub(super) fn publish_welcomes_to_peers(
        &mut self,
        peer_pubkeys: Vec<PublicKey>,
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
            let signer = match client.signer().await {
                Ok(signer) => signer,
                Err(err) => {
                    tracing::error!("welcome delivery signer unavailable: {err}");
                    return;
                }
            };
            Self::publish_welcome_rumors_best_effort(
                &client,
                &signer,
                &peer_pubkeys,
                &welcome_rumors,
                relays,
                4,
                "welcome publish",
            )
            .await;
        });
    }

    async fn publish_welcome_rumors_best_effort<T>(
        client: &Client,
        signer: &T,
        recipients: &[PublicKey],
        welcome_rumors: &[UnsignedEvent],
        relays: Vec<RelayUrl>,
        max_attempts: u8,
        context: &'static str,
    ) where
        T: NostrSigner,
    {
        Self::publish_welcome_rumors_best_effort_with_publisher(
            signer,
            recipients,
            welcome_rumors,
            max_attempts,
            context,
            |_, giftwrap| {
                let client = client.clone();
                let relays = relays.clone();
                async move {
                    super::relay_publish::publish_event_with_retry(
                        &client,
                        &relays,
                        &giftwrap,
                        max_attempts,
                        context,
                        true,
                    )
                    .await
                }
            },
        )
        .await;
    }

    async fn publish_welcome_rumors_best_effort_with_publisher<T, F, Fut>(
        signer: &T,
        recipients: &[PublicKey],
        welcome_rumors: &[UnsignedEvent],
        max_attempts: u8,
        context: &'static str,
        mut publish_giftwrap: F,
    ) where
        T: NostrSigner,
        F: FnMut(PublicKey, Event) -> Fut,
        Fut: Future<Output = super::relay_publish::PublishOutcome>,
    {
        let expires = Timestamp::from_secs(Timestamp::now().as_secs() + 30 * 24 * 60 * 60);
        let tags = vec![Tag::expiration(expires)];
        let result = publish_welcome_rumors(
            signer,
            welcome_rumors,
            recipients,
            tags,
            |receiver, giftwrap| {
                let publish = publish_giftwrap(receiver, giftwrap);
                async move {
                    match publish.await {
                        super::relay_publish::PublishOutcome::Ok => Ok(()),
                        super::relay_publish::PublishOutcome::Err(err) => {
                            tracing::error!(
                                "{context} failed after {max_attempts} attempts: {err}"
                            );
                            Ok(())
                        }
                    }
                }
            },
        )
        .await;
        if let Err(err) = result {
            tracing::error!("welcome delivery setup failed: {err}");
        }
    }

    /// Load cached follow pubkeys from the profile DB and build an initial
    /// follow list so the UI can display follows instantly before the network
    /// fetch completes.
    pub(super) fn hydrate_follow_list_from_cache(&mut self) {
        let Some(conn) = self.profile_db.as_ref() else {
            return;
        };
        let cached_follows = profile_db::load_follows(conn);
        if cached_follows.is_empty() {
            return;
        }
        let mut follow_list: Vec<crate::state::FollowListEntry> = cached_follows
            .into_iter()
            .map(|hex_pubkey| {
                let npub = PublicKey::from_hex(&hex_pubkey)
                    .ok()
                    .and_then(|pk| pk.to_bech32().ok())
                    .unwrap_or_else(|| hex_pubkey.clone());
                let cached = self.profiles.get(&hex_pubkey);
                let name = cached.and_then(|p| p.name.clone());
                let username = cached.and_then(|p| p.username.clone());
                let picture_url =
                    cached.and_then(|p| p.display_picture_url(&self.data_dir, &hex_pubkey));
                crate::state::FollowListEntry {
                    pubkey: hex_pubkey,
                    npub,
                    name,
                    username,
                    picture_url,
                }
            })
            .collect();
        follow_list.sort_by(|a, b| match (&a.name, &b.name) {
            (Some(na), Some(nb)) => na.to_lowercase().cmp(&nb.to_lowercase()),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.npub.cmp(&b.npub),
        });
        self.state.follow_list = follow_list;
        self.emit_state();
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

        let my_pubkey = sess.pubkey;
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

            let newest = contact_events
                .into_iter()
                .filter(|e| e.verify().is_ok())
                .max_by_key(|e| e.created_at);
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
                    for ev in events.into_iter().filter(|e| e.verify().is_ok()) {
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
                .and_then(|evs| {
                    evs.into_iter()
                        .filter(|e| e.verify().is_ok())
                        .max_by_key(|e| e.created_at)
                });

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
        if let Some(conn) = self.profile_db.as_ref() {
            profile_db::add_follow(conn, pubkey_hex);
        }
        self.modify_contact_list(pubkey_hex, true);
    }

    pub(super) fn unfollow_user(&mut self, pubkey_hex: &str) {
        if let Some(conn) = self.profile_db.as_ref() {
            profile_db::remove_follow(conn, pubkey_hex);
        }
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
        let (my_pubkey, client) = {
            let Some(sess) = self.session.as_ref() else {
                return;
            };
            (sess.pubkey, sess.client.clone())
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
                Ok(evs) => evs
                    .into_iter()
                    .filter(|e| e.verify().is_ok())
                    .max_by_key(|e| e.created_at),
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

            let event = match client
                .sign_event_builder(EventBuilder::new(Kind::ContactList, &content).tags(tags))
                .await
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
        let relays = self.default_relays();
        self.runtime.spawn(async move {
            let req = EventDeletionRequest::new()
                .id(id)
                .reason("rotated key package");
            if let Ok(ev) = client.sign_event_builder(EventBuilder::delete(req)).await {
                let _ = client.send_event_to(&relays, &ev).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::make_core_with_config_for_tests as make_core_with_config;

    fn open_test_mdk(dir: &tempfile::TempDir, keys: &Keys) -> PikaMdk {
        crate::mdk_support::open_mdk(
            dir.path().to_str().expect("tempdir path"),
            &keys.public_key(),
            "test.keychain.group",
        )
        .expect("open test mdk")
    }

    fn make_key_package_event(mdk: &PikaMdk, keys: &Keys) -> Event {
        let relay = RelayUrl::parse("wss://test.relay").expect("relay url");
        let (content, tags, _hash_ref) = mdk
            .create_key_package_for_event(&keys.public_key(), vec![relay])
            .expect("create key package");
        EventBuilder::new(Kind::MlsKeyPackage, content)
            .tags(tags)
            .sign_with_keys(keys)
            .expect("sign key package")
    }

    #[test]
    fn app_runtime_bootstrap_uses_shared_session_service() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir, &inviter_keys);
        let invitee_mdk = open_test_mdk(&invitee_dir, &invitee_keys);

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData {
            name: "App runtime bootstrap".to_string(),
            description: String::new(),
            image_hash: None,
            image_key: None,
            image_nonce: None,
            relays: vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            admins: vec![inviter_keys.public_key()],
        };
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");

        let bootstrapped = bootstrap_runtime_for_app(
            inviter_dir.path().to_str().expect("tempdir path"),
            "test.keychain.group",
            inviter_keys.public_key(),
            Arc::new(inviter_keys.clone()),
            vec![RelayUrl::parse("wss://message-1.example").expect("message relay")],
            vec![RelayUrl::parse("wss://kp-1.example").expect("kp relay")],
        )
        .expect("bootstrap app runtime");

        assert_eq!(bootstrapped.session.pubkey, inviter_keys.public_key());
        assert_eq!(bootstrapped.open.pubkey, inviter_keys.public_key());
        assert_eq!(
            bootstrapped.open.joined_group_snapshots.len(),
            1,
            "app bootstrap should surface joined groups through shared open state"
        );
        assert_eq!(
            bootstrapped
                .open
                .sync_plan
                .relay_roles
                .session_connect_relays,
            vec![
                RelayUrl::parse("wss://message-1.example").expect("message relay"),
                RelayUrl::parse("wss://test.relay").expect("relay url"),
            ]
        );
        let seeded_groups = seed_app_groups_from_open_state(
            &inviter_keys.public_key(),
            &bootstrapped.open.joined_group_snapshots,
        );
        assert_eq!(seeded_groups.len(), 1);
        assert!(
            seeded_groups
                .get(&hex::encode(created.group.nostr_group_id))
                .expect("seeded group")
                .self_is_admin
        );
        assert_eq!(
            bootstrapped.startup.group_subscriptions.target_group_ids,
            vec![hex::encode(created.group.nostr_group_id)]
        );
    }

    #[test]
    fn app_subscription_planning_uses_shared_runtime_targets() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir, &inviter_keys);
        let invitee_mdk = open_test_mdk(&invitee_dir, &invitee_keys);

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let created = inviter_mdk
            .create_group(
                &inviter_keys.public_key(),
                vec![invitee_kp],
                NostrGroupConfigData {
                    name: "App subscription planning".to_string(),
                    description: String::new(),
                    image_hash: None,
                    image_key: None,
                    image_nonce: None,
                    relays: vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
                    admins: vec![inviter_keys.public_key()],
                },
            )
            .expect("create group");
        let expected_group_id = hex::encode(created.group.nostr_group_id);
        let mut groups = HashMap::new();
        groups.insert(
            "stale-group".to_string(),
            GroupIndexEntry {
                mls_group_id: GroupId::from_slice(&[1, 2, 3]),
                is_group: true,
                group_name: Some("Stale Group".into()),
                self_is_admin: false,
                members: vec![],
            },
        );
        let session = Session {
            pubkey: inviter_keys.public_key(),
            local_keys: Some(inviter_keys.clone()),
            mdk: inviter_mdk,
            client: Client::default(),
            alive: Arc::new(AtomicBool::new(true)),
            giftwrap_sub: None,
            group_sub: None,
            groups,
        };

        let plan = plan_app_group_subscriptions(&session).expect("plan app group subscriptions");

        assert_eq!(
            plan.current.target_group_ids,
            vec![expected_group_id.clone()]
        );
        assert_eq!(
            plan.current.relay_urls,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")]
        );
        assert_eq!(plan.added_group_ids, vec![expected_group_id]);
        assert_eq!(plan.removed_group_ids, vec!["stale-group".to_string()]);
    }

    #[test]
    fn app_session_sync_planning_uses_shared_runtime_sync_plan() {
        let (core, inviter_dir) = make_core_with_config(crate::core::config::AppConfig {
            relay_urls: Some(vec!["wss://message-1.example".to_string()]),
            key_package_relay_urls: Some(vec!["wss://kp-1.example".to_string()]),
            ..crate::core::config::AppConfig::default()
        });
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir, &inviter_keys);
        let invitee_mdk = open_test_mdk(&invitee_dir, &invitee_keys);
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let created = inviter_mdk
            .create_group(
                &inviter_keys.public_key(),
                vec![invitee_kp],
                NostrGroupConfigData {
                    name: "App session sync planning".to_string(),
                    description: String::new(),
                    image_hash: None,
                    image_key: None,
                    image_nonce: None,
                    relays: vec![RelayUrl::parse("wss://group-1.example").expect("group relay")],
                    admins: vec![inviter_keys.public_key()],
                },
            )
            .expect("create group");
        let expected_group_id = hex::encode(created.group.nostr_group_id);
        let mut groups = HashMap::new();
        groups.insert(
            "stale-group".to_string(),
            GroupIndexEntry {
                mls_group_id: GroupId::from_slice(&[1, 2, 3]),
                is_group: true,
                group_name: Some("Stale Group".into()),
                self_is_admin: false,
                members: vec![],
            },
        );
        let session = Session {
            pubkey: inviter_keys.public_key(),
            local_keys: Some(inviter_keys.clone()),
            mdk: inviter_mdk,
            client: Client::default(),
            alive: Arc::new(AtomicBool::new(true)),
            giftwrap_sub: None,
            group_sub: None,
            groups,
        };

        let sync_plan = plan_app_session_sync(&core, &session).expect("plan app session sync");

        assert_eq!(
            sync_plan.group_subscriptions.current.target_group_ids,
            vec![expected_group_id.clone()]
        );
        assert_eq!(
            sync_plan.group_subscriptions.added_group_ids,
            vec![expected_group_id]
        );
        assert_eq!(
            sync_plan.group_subscriptions.removed_group_ids,
            vec!["stale-group".to_string()]
        );
        assert_eq!(
            sync_plan.relay_roles.session_connect_relays,
            vec![
                RelayUrl::parse("wss://group-1.example").expect("group relay"),
                RelayUrl::parse("wss://message-1.example").expect("message relay"),
            ]
        );
        assert_eq!(sync_plan.welcome_inbox, app_welcome_inbox_intent());
    }

    #[test]
    fn app_relay_role_planning_keeps_key_package_relays_out_of_session_connects() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir, &inviter_keys);
        let invitee_mdk = open_test_mdk(&invitee_dir, &invitee_keys);
        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let group_relay = RelayUrl::parse("wss://group-1.example").expect("group relay");
        inviter_mdk
            .create_group(
                &inviter_keys.public_key(),
                vec![invitee_kp],
                NostrGroupConfigData {
                    name: "Relay role planning".to_string(),
                    description: String::new(),
                    image_hash: None,
                    image_key: None,
                    image_nonce: None,
                    relays: vec![group_relay.clone()],
                    admins: vec![inviter_keys.public_key()],
                },
            )
            .expect("create group");
        let session = Session {
            pubkey: inviter_keys.public_key(),
            local_keys: Some(inviter_keys.clone()),
            mdk: inviter_mdk,
            client: Client::default(),
            alive: Arc::new(AtomicBool::new(true)),
            giftwrap_sub: None,
            group_sub: None,
            groups: HashMap::new(),
        };
        let plan = plan_app_group_subscriptions(&session).expect("plan app group subscriptions");
        let relay_roles = pika_marmot_runtime::runtime::plan_runtime_relay_roles(
            vec![RelayUrl::parse("wss://message-1.example").expect("message relay")],
            plan.current.relay_urls.clone(),
            vec![RelayUrl::parse("wss://kp-1.example").expect("kp relay")],
        );
        let session_relays: BTreeSet<RelayUrl> =
            relay_roles.session_connect_relays.into_iter().collect();

        assert_eq!(
            session_relays,
            BTreeSet::from([
                RelayUrl::parse("wss://group-1.example").expect("group relay"),
                RelayUrl::parse("wss://message-1.example").expect("message relay"),
            ])
        );
        assert!(!session_relays.contains(&RelayUrl::parse("wss://kp-1.example").expect("kp relay")));
    }

    #[tokio::test]
    async fn app_background_publish_uses_shared_welcome_pairing() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let bob_dir = tempfile::tempdir().expect("bob tempdir");
        let charlie_dir = tempfile::tempdir().expect("charlie tempdir");
        let inviter_keys = Keys::generate();
        let bob_keys = Keys::generate();
        let charlie_keys = Keys::generate();
        let inviter_mdk = open_test_mdk(&inviter_dir, &inviter_keys);
        let bob_mdk = open_test_mdk(&bob_dir, &bob_keys);
        let charlie_mdk = open_test_mdk(&charlie_dir, &charlie_keys);

        let bob_kp = make_key_package_event(&bob_mdk, &bob_keys);
        let charlie_kp = make_key_package_event(&charlie_mdk, &charlie_keys);
        let config = NostrGroupConfigData {
            name: "App publish test".to_string(),
            description: String::new(),
            image_hash: None,
            image_key: None,
            image_nonce: None,
            relays: vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            admins: vec![inviter_keys.public_key()],
        };
        let group_result = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![bob_kp, charlie_kp], config)
            .expect("create group");

        let published =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::<(PublicKey, Event)>::new()));
        let published_capture = std::sync::Arc::clone(&published);
        AppCore::publish_welcome_rumors_best_effort_with_publisher(
            &inviter_keys,
            &[bob_keys.public_key(), charlie_keys.public_key()],
            &group_result.welcome_rumors,
            4,
            "welcome publish",
            move |receiver, giftwrap| {
                let published_capture = std::sync::Arc::clone(&published_capture);
                async move {
                    published_capture
                        .lock()
                        .expect("published lock")
                        .push((receiver, giftwrap));
                    super::relay_publish::PublishOutcome::Ok
                }
            },
        )
        .await;

        let published = published.lock().expect("published lock").clone();
        assert_eq!(published.len(), 2);

        for (receiver, wrapper) in published {
            match receiver {
                receiver if receiver == bob_keys.public_key() => {
                    let ingested = pika_marmot_runtime::welcome::ingest_welcome_from_giftwrap(
                        &bob_mdk,
                        &bob_keys,
                        &wrapper,
                        |_| true,
                    )
                    .await
                    .expect("ingest bob welcome");
                    assert!(ingested.is_some(), "bob should ingest exactly one welcome");
                }
                receiver if receiver == charlie_keys.public_key() => {
                    let ingested = pika_marmot_runtime::welcome::ingest_welcome_from_giftwrap(
                        &charlie_mdk,
                        &charlie_keys,
                        &wrapper,
                        |_| true,
                    )
                    .await
                    .expect("ingest charlie welcome");
                    assert!(
                        ingested.is_some(),
                        "charlie should ingest exactly one welcome"
                    );
                }
                other => panic!("unexpected receiver {}", other.to_hex()),
            }
        }
    }

    #[tokio::test]
    async fn app_notification_ingress_uses_shared_runtime_classifier_for_welcome() {
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let client = Client::builder().signer(invitee_keys.clone()).build();
        let rumor = UnsignedEvent::new(
            inviter_keys.public_key(),
            Timestamp::from(1_u64),
            Kind::MlsWelcome,
            Tags::new(),
            "{}".to_string(),
        );
        let wrapper = EventBuilder::gift_wrap(
            &inviter_keys,
            &invitee_keys.public_key(),
            rumor.clone(),
            Vec::<Tag>::new(),
        )
        .await
        .expect("gift wrap");
        let mut seen = InboundRelaySeenCache::default();

        let first = classify_app_notification_event(&client, &mut seen, wrapper.clone())
            .await
            .expect("classify app inbound event");
        let duplicate = classify_app_notification_event(&client, &mut seen, wrapper.clone())
            .await
            .expect("classify duplicate app inbound event");

        match first {
            Some(InternalEvent::GiftWrapReceived {
                wrapper: first_wrapper,
                rumor: first_rumor,
            }) => {
                assert_eq!(first_wrapper.id, wrapper.id);
                assert_eq!(first_rumor.pubkey, rumor.pubkey);
                assert_eq!(first_rumor.kind, rumor.kind);
                assert_eq!(first_rumor.content, rumor.content);
            }
            other => panic!("expected giftwrap internal event, got {other:?}"),
        }
        assert!(
            duplicate.is_none(),
            "shared ingress classifier should suppress duplicate relay events for the app"
        );
    }
}
