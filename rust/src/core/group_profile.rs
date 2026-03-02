use base64::Engine;
use nostr_blossom::client::BlossomClient;

use super::*;

const MAX_PROFILE_IMAGE_BYTES: usize = 8 * 1024 * 1024;

impl AppCore {
    pub(super) fn save_group_profile(&mut self, chat_id: String, name: String, about: String) {
        if !self.is_logged_in() {
            self.toast("Please log in first");
            return;
        }

        let Some(sess) = self.session.as_ref() else {
            return;
        };
        let my_hex = sess.pubkey.to_hex();
        let Some(group) = sess.groups.get(&chat_id).cloned() else {
            self.toast("Chat not found");
            return;
        };

        // Build metadata, preserving existing group picture URL if any.
        let existing_picture = self
            .group_profiles
            .get(&chat_id)
            .and_then(|m| m.get(&my_hex))
            .and_then(|p| p.picture_url.clone());

        let mut metadata = Metadata::new();
        let name_trimmed = name.trim();
        if !name_trimmed.is_empty() {
            metadata.name = Some(name_trimmed.to_string());
            metadata.display_name = Some(name_trimmed.to_string());
        }
        let about_trimmed = about.trim();
        if !about_trimmed.is_empty() {
            metadata.about = Some(about_trimmed.to_string());
        }
        metadata.picture = existing_picture;

        let metadata_json = serde_json::to_string(&metadata).unwrap_or_default();
        self.publish_group_kind0(&chat_id, &group.mls_group_id, &metadata_json, None);

        // Update local cache immediately.
        let cache =
            ProfileCache::from_metadata_json(Some(metadata_json), now_seconds(), now_seconds());
        self.upsert_group_profile(&chat_id, my_hex, cache);
        self.refresh_chat_list_from_storage();
        self.refresh_current_chat_if_open(&chat_id);
    }

    pub(super) fn upload_group_profile_image(
        &mut self,
        chat_id: String,
        image_base64: String,
        mime_type: String,
    ) {
        if !self.is_logged_in() {
            self.toast("Please log in first");
            return;
        }

        let image_bytes = match base64::engine::general_purpose::STANDARD.decode(image_base64) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.toast(format!("Invalid image data: {e}"));
                return;
            }
        };
        if image_bytes.is_empty() {
            self.toast("Pick an image first");
            return;
        }
        if image_bytes.len() > MAX_PROFILE_IMAGE_BYTES {
            self.toast("Image too large (max 8 MB)");
            return;
        }

        let Some(sess) = self.session.as_ref() else {
            return;
        };
        if !sess.groups.contains_key(&chat_id) {
            self.toast("Chat not found");
            return;
        }
        let Some(local_keys) = sess.local_keys.clone() else {
            self.toast("Profile image upload requires local key signer");
            return;
        };

        let my_hex = sess.pubkey.to_hex();

        // Build metadata preserving existing name/about.
        let existing = self
            .group_profiles
            .get(&chat_id)
            .and_then(|m| m.get(&my_hex))
            .cloned();
        let mut metadata = Metadata::new();
        if let Some(ref ep) = existing {
            if let Some(ref n) = ep.name {
                metadata.name = Some(n.clone());
                metadata.display_name = Some(n.clone());
            }
            if let Some(ref a) = ep.about {
                metadata.about = Some(a.clone());
            }
        }

        let mime_type = mime_type.trim().to_string();
        let blossom_servers = self.blossom_servers();
        let tx = self.core_sender.clone();

        // Upload to Blossom async, then send result back to main thread for MLS publish.
        self.runtime.spawn(async move {
            if blossom_servers.is_empty() {
                let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::Toast(
                    "No valid Blossom servers configured".to_string(),
                ))));
                return;
            }

            let mut last_error: Option<String> = None;

            for server in &blossom_servers {
                let base_url = match Url::parse(server) {
                    Ok(url) => url,
                    Err(e) => {
                        last_error = Some(format!("{server}: {e}"));
                        continue;
                    }
                };

                let blossom = BlossomClient::new(base_url);
                let upload = blossom
                    .upload_blob(
                        image_bytes.clone(),
                        Some(mime_type.clone()),
                        None,
                        Some(&local_keys),
                    )
                    .await;

                let descriptor = match upload {
                    Ok(d) => d,
                    Err(e) => {
                        last_error = Some(format!("{server}: {e}"));
                        continue;
                    }
                };

                metadata.picture = Some(descriptor.url.to_string());
                let metadata_json = serde_json::to_string(&metadata).unwrap_or_default();

                // Send back to main thread for MLS publish + cache update.
                let _ = tx.send(CoreMsg::Internal(Box::new(
                    InternalEvent::GroupProfileImageUploaded {
                        chat_id,
                        metadata_json,
                        image_bytes,
                    },
                )));
                return;
            }

            let message = last_error
                .map(|e| format!("Blossom upload failed: {e}"))
                .unwrap_or_else(|| "Blossom upload failed".to_string());
            let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::Toast(message))));
        });
    }

    /// Handle result of async Blossom upload for group profile image.
    pub(super) fn handle_group_profile_image_uploaded(
        &mut self,
        chat_id: String,
        metadata_json: String,
        image_bytes: Vec<u8>,
    ) {
        let Some(sess) = self.session.as_ref() else {
            return;
        };
        let my_hex = sess.pubkey.to_hex();
        let Some(group) = sess.groups.get(&chat_id).cloned() else {
            return;
        };

        // Publish the kind-0 with picture URL via MLS.
        self.publish_group_kind0(&chat_id, &group.mls_group_id, &metadata_json, None);

        // Cache the image locally.
        profile_pics::ensure_group_dir(&self.data_dir, &chat_id);
        let _ =
            profile_pics::save_group_image_bytes(&self.data_dir, &chat_id, &my_hex, &image_bytes);

        // Update local cache.
        let cache =
            ProfileCache::from_metadata_json(Some(metadata_json), now_seconds(), now_seconds());
        self.upsert_group_profile(&chat_id, my_hex, cache);
        self.refresh_chat_list_from_storage();
        self.refresh_current_chat_if_open(&chat_id);
    }

    /// Publish a kind-0 rumor to a group via MLS encryption.
    /// Used for both self-set profiles and admin rebroadcasts.
    fn publish_group_kind0(
        &mut self,
        chat_id: &str,
        mls_group_id: &GroupId,
        metadata_json: &str,
        p_tag_pubkey: Option<&str>,
    ) {
        let Some(sess) = self.session.as_mut() else {
            return;
        };

        let mut tags: Vec<Tag> = Vec::new();
        if let Some(pk_hex) = p_tag_pubkey {
            if let Ok(pk) = PublicKey::from_hex(pk_hex) {
                tags.push(Tag::public_key(pk));
            }
        }

        let rumor = UnsignedEvent::new(
            sess.pubkey,
            Timestamp::now(),
            Kind::Metadata,
            tags,
            metadata_json.to_string(),
        );

        let wrapper = match sess.mdk.create_message(mls_group_id, rumor) {
            Ok(ev) => ev,
            Err(e) => {
                tracing::warn!(err = %e, %chat_id, "group profile create_message failed");
                return;
            }
        };

        let client = sess.client.clone();
        let relays: Vec<RelayUrl> = sess
            .mdk
            .get_relays(mls_group_id)
            .ok()
            .map(|s| s.into_iter().collect())
            .filter(|v: &Vec<RelayUrl>| !v.is_empty())
            .unwrap_or_else(|| self.default_relays());

        self.runtime.spawn(async move {
            let _ = client.send_event_to(relays, &wrapper).await;
        });
    }

    /// Rebroadcast all stored group profiles for a chat.
    /// Called by the admin after adding new members.
    pub(super) fn rebroadcast_group_profiles(&mut self, chat_id: &str, mls_group_id: &GroupId) {
        let profiles_to_broadcast: Vec<(String, String)> = self
            .group_profiles
            .get(chat_id)
            .map(|m| {
                m.iter()
                    .filter_map(|(pk, cache)| {
                        cache
                            .metadata_json
                            .as_ref()
                            .map(|json| (pk.clone(), json.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        if profiles_to_broadcast.is_empty() {
            return;
        }

        for (pubkey_hex, metadata_json) in profiles_to_broadcast {
            self.publish_group_kind0(chat_id, mls_group_id, &metadata_json, Some(&pubkey_hex));
        }
    }

    /// Re-publish our own group profile when we detect a commit (membership change).
    pub(super) fn maybe_rebroadcast_my_group_profile(
        &mut self,
        chat_id: &str,
        mls_group_id: &GroupId,
    ) {
        let my_hex = match self.session.as_ref() {
            Some(s) => s.pubkey.to_hex(),
            None => return,
        };

        let metadata_json = self
            .group_profiles
            .get(chat_id)
            .and_then(|m| m.get(&my_hex))
            .and_then(|p| p.metadata_json.clone());

        let Some(json) = metadata_json else {
            return;
        };

        // Self-set: no p tag needed.
        self.publish_group_kind0(chat_id, mls_group_id, &json, None);
    }
}
