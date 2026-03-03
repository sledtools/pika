use base64::Engine;

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

        self.publish_group_kind0(&chat_id, &group.mls_group_id, &metadata_json, None, vec![]);

        // Update local cache immediately.
        let cache =
            ProfileCache::from_metadata_json(Some(metadata_json), now_seconds(), now_seconds());
        self.upsert_group_profile(&chat_id, my_hex, cache, None);
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

        let Some(sess) = self.session.as_mut() else {
            return;
        };
        let Some(group) = sess.groups.get(&chat_id).cloned() else {
            self.toast("Chat not found");
            return;
        };
        let Some(local_keys) = sess.local_keys.clone() else {
            self.toast("Profile image upload requires local key signer");
            return;
        };

        let my_hex = sess.pubkey.to_hex();

        // Encrypt the image using MLS group media encryption.
        let manager = sess.mdk.media_manager(group.mls_group_id.clone());
        let mut upload = match manager.encrypt_for_upload_with_options(
            &image_bytes,
            &mime_type,
            "profile.jpg",
            &MediaProcessingOptions::default(),
        ) {
            Ok(u) => u,
            Err(e) => {
                self.toast(format!("Profile image encryption failed: {e}"));
                return;
            }
        };

        let encrypted_data = std::mem::take(&mut upload.encrypted_data);
        let expected_hash_hex = hex::encode(upload.encrypted_hash);

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

        let blossom_servers = self.blossom_servers();
        let tx = self.core_sender.clone();

        // Upload encrypted data to Blossom async.
        self.runtime.spawn(async move {
            let result = chat_media::upload_to_blossom(
                &blossom_servers,
                encrypted_data,
                "application/octet-stream",
                &expected_hash_hex,
                &local_keys,
            )
            .await;

            match result {
                Ok((uploaded_url, _)) => {
                    metadata.picture = Some(uploaded_url.clone());
                    let metadata_json = serde_json::to_string(&metadata).unwrap_or_default();
                    let _ = tx.send(CoreMsg::Internal(Box::new(
                        InternalEvent::GroupProfileImageUploaded {
                            chat_id,
                            metadata_json,
                            image_bytes,
                            upload,
                            uploaded_url,
                        },
                    )));
                }
                Err(e) => {
                    let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::Toast(e))));
                }
            }
        });
    }

    /// Handle result of async Blossom upload for encrypted group profile image.
    pub(super) fn handle_group_profile_image_uploaded(
        &mut self,
        chat_id: String,
        metadata_json: String,
        image_bytes: Vec<u8>,
        upload: EncryptedMediaUpload,
        uploaded_url: String,
    ) {
        let Some(sess) = self.session.as_mut() else {
            return;
        };
        let my_hex = sess.pubkey.to_hex();
        let Some(group) = sess.groups.get(&chat_id).cloned() else {
            return;
        };

        // Create imeta tag from the upload + URL.
        let manager = sess.mdk.media_manager(group.mls_group_id.clone());
        let imeta_tag = manager.create_imeta_tag(&upload, &uploaded_url);

        // Publish kind-0 with picture URL and imeta tag.
        self.publish_group_kind0(
            &chat_id,
            &group.mls_group_id,
            &metadata_json,
            None,
            vec![imeta_tag],
        );

        // Cache the plaintext image locally.
        profile_pics::ensure_group_dir(&self.data_dir, &chat_id);
        let _ =
            profile_pics::save_group_image_bytes(&self.data_dir, &chat_id, &my_hex, &image_bytes);

        // Update local cache. The encrypted pic metadata is not stored in
        // ProfileCache — we already have the plaintext cached on disk, so we
        // pass None for encrypted_pic_info (no download needed).
        let cache =
            ProfileCache::from_metadata_json(Some(metadata_json), now_seconds(), now_seconds());
        self.upsert_group_profile(&chat_id, my_hex, cache, None);
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
        extra_tags: Vec<Tag>,
    ) {
        let Some(sess) = self.session.as_mut() else {
            return;
        };

        let mut tags = extra_tags;
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

    /// Rebroadcast all stored group profiles for a chat (excluding our own,
    /// which is handled by `maybe_rebroadcast_my_group_profile` on commit).
    /// Called by the admin after adding new members.
    pub(super) fn rebroadcast_group_profiles(&mut self, chat_id: &str, mls_group_id: &GroupId) {
        let my_hex = self.session.as_ref().map(|s| s.pubkey.to_hex());
        let profiles_to_broadcast =
            profiles_to_rebroadcast(self.group_profiles.get(chat_id), my_hex.as_deref());

        if profiles_to_broadcast.is_empty() {
            return;
        }

        for (pubkey_hex, metadata_json) in profiles_to_broadcast {
            self.publish_group_kind0(
                chat_id,
                mls_group_id,
                &metadata_json,
                Some(&pubkey_hex),
                vec![],
            );
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
        self.publish_group_kind0(chat_id, mls_group_id, &json, None, vec![]);
    }
}

/// Extract the list of (pubkey_hex, metadata_json) pairs that should be
/// rebroadcast for a group. Excludes `my_hex` (own profile) and entries
/// without `metadata_json`.
fn profiles_to_rebroadcast(
    group_map: Option<&HashMap<String, ProfileCache>>,
    my_hex: Option<&str>,
) -> Vec<(String, String)> {
    group_map
        .map(|m| {
            m.iter()
                .filter(|(pk, _)| my_hex != Some(pk.as_str()))
                .filter_map(|(pk, cache)| {
                    cache
                        .metadata_json
                        .as_ref()
                        .map(|json| (pk.clone(), json.clone()))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- profiles_to_rebroadcast filtering tests ---

    fn make_cache(metadata_json: Option<&str>, name: Option<&str>) -> ProfileCache {
        ProfileCache {
            metadata_json: metadata_json.map(String::from),
            name: name.map(String::from),
            username: None,
            about: None,
            picture_url: None,
            event_created_at: 1,
            last_checked_at: 1,
        }
    }

    #[test]
    fn rebroadcast_filter_excludes_own_profile() {
        let mut map = HashMap::new();
        map.insert(
            "my_pk".to_string(),
            make_cache(Some(r#"{"name":"Me"}"#), Some("Me")),
        );
        map.insert(
            "other_pk".to_string(),
            make_cache(Some(r#"{"name":"Other"}"#), Some("Other")),
        );

        let result = profiles_to_rebroadcast(Some(&map), Some("my_pk"));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "other_pk");
    }

    #[test]
    fn rebroadcast_filter_skips_no_metadata() {
        let mut map = HashMap::new();
        map.insert("pk1".to_string(), make_cache(None, Some("NoMeta")));
        map.insert(
            "pk2".to_string(),
            make_cache(Some(r#"{"name":"HasMeta"}"#), Some("HasMeta")),
        );

        let result = profiles_to_rebroadcast(Some(&map), None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "pk2");
    }

    #[test]
    fn rebroadcast_filter_empty_map_returns_empty() {
        let result = profiles_to_rebroadcast(None, Some("my_pk"));
        assert!(result.is_empty());
    }

    #[test]
    fn rebroadcast_filter_all_own_returns_empty() {
        let mut map = HashMap::new();
        map.insert(
            "my_pk".to_string(),
            make_cache(Some(r#"{"name":"Me"}"#), Some("Me")),
        );

        let result = profiles_to_rebroadcast(Some(&map), Some("my_pk"));
        assert!(result.is_empty());
    }

    // --- metadata construction tests ---

    #[test]
    fn metadata_preserves_picture_when_name_changes() {
        let json = r#"{"name":"Alice","picture":"https://example.com/pic.jpg"}"#;
        let cache = ProfileCache::from_metadata_json(Some(json.to_string()), 1, 1);
        assert_eq!(cache.name.as_deref(), Some("Alice"));
        assert_eq!(
            cache.picture_url.as_deref(),
            Some("https://example.com/pic.jpg")
        );

        // Rebuild with different name but same picture via Metadata struct.
        let mut metadata = Metadata::new();
        metadata.name = Some("Bob".to_string());
        metadata.display_name = Some("Bob".to_string());
        metadata.picture = cache.picture_url.clone();
        let new_json = serde_json::to_string(&metadata).unwrap();
        let new_cache = ProfileCache::from_metadata_json(Some(new_json), 2, 2);
        assert_eq!(new_cache.name.as_deref(), Some("Bob"));
        assert_eq!(
            new_cache.picture_url.as_deref(),
            Some("https://example.com/pic.jpg")
        );
    }

    #[test]
    fn metadata_trims_name_and_about() {
        let mut metadata = Metadata::new();
        let name = "  Alice  ".trim();
        if !name.is_empty() {
            metadata.name = Some(name.to_string());
        }
        let about = "  bio  ".trim();
        if !about.is_empty() {
            metadata.about = Some(about.to_string());
        }
        let json = serde_json::to_string(&metadata).unwrap();
        let cache = ProfileCache::from_metadata_json(Some(json), 1, 1);
        assert_eq!(cache.name.as_deref(), Some("Alice"));
        assert_eq!(cache.about.as_deref(), Some("bio"));
    }

    #[test]
    fn metadata_empty_name_becomes_none() {
        let cache =
            ProfileCache::from_metadata_json(Some(r#"{"about":"just about"}"#.to_string()), 1, 1);
        assert!(cache.name.is_none());
        assert_eq!(cache.about.as_deref(), Some("just about"));
    }
}
