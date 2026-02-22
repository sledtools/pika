use std::time::Duration;

use base64::Engine;
use nostr_blossom::client::BlossomClient;

use crate::state::MyProfileState;

use super::*;

const MAX_PROFILE_IMAGE_BYTES: usize = 8 * 1024 * 1024;

impl AppCore {
    pub(super) fn refresh_my_profile(&mut self, toast_on_error: bool) {
        if !self.is_logged_in() {
            return;
        }
        if !self.network_enabled() {
            if toast_on_error {
                self.toast("Network disabled");
            }
            return;
        }

        let (client, pubkey, tx) = {
            let Some(sess) = self.session.as_ref() else {
                return;
            };
            (sess.client.clone(), sess.pubkey, self.core_sender.clone())
        };

        self.runtime.spawn(async move {
            client.connect().await;
            client.wait_for_connection(Duration::from_secs(4)).await;
            match client.fetch_metadata(pubkey, Duration::from_secs(8)).await {
                Ok(metadata) => {
                    let _ = tx.send(CoreMsg::Internal(Box::new(
                        InternalEvent::MyProfileFetched { metadata },
                    )));
                }
                Err(e) => {
                    let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::MyProfileError {
                        message: format!("Profile fetch failed: {e}"),
                        toast: toast_on_error,
                    })));
                }
            }
        });
    }

    pub(super) fn save_my_profile(&mut self, name: String, about: String) {
        if !self.is_logged_in() {
            self.toast("Please log in first");
            return;
        }
        if !self.network_enabled() {
            self.toast("Network disabled");
            return;
        }

        let metadata = self.metadata_for_profile_edit(name, about);
        let (client, tx) = {
            let Some(sess) = self.session.as_ref() else {
                return;
            };
            (sess.client.clone(), self.core_sender.clone())
        };

        self.runtime.spawn(async move {
            client.connect().await;
            client.wait_for_connection(Duration::from_secs(4)).await;
            match client.set_metadata(&metadata).await {
                Ok(output) if !output.success.is_empty() => {
                    let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::MyProfileSaved {
                        metadata,
                    })));
                }
                Ok(output) => {
                    let message = output
                        .failed
                        .values()
                        .next()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "no relay accepted profile update".to_string());
                    let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::MyProfileError {
                        message: format!("Profile update failed: {message}"),
                        toast: true,
                    })));
                }
                Err(e) => {
                    let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::MyProfileError {
                        message: format!("Profile update failed: {e}"),
                        toast: true,
                    })));
                }
            }
        });
    }

    pub(super) fn upload_my_profile_image(&mut self, image_base64: String, mime_type: String) {
        if !self.is_logged_in() {
            self.toast("Please log in first");
            return;
        }
        if !self.network_enabled() {
            self.toast("Network disabled");
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

        let mut metadata = self.metadata_for_profile_edit(
            self.state.my_profile.name.clone(),
            self.state.my_profile.about.clone(),
        );
        let mime_type = Self::normalized_profile_field(mime_type);

        let blossom_servers = self.blossom_servers();
        let (client, local_keys, tx) = {
            let Some(sess) = self.session.as_ref() else {
                return;
            };
            let Some(local_keys) = sess.local_keys.clone() else {
                self.toast("Profile image upload requires local key signer");
                return;
            };
            (sess.client.clone(), local_keys, self.core_sender.clone())
        };

        self.runtime.spawn(async move {
            if blossom_servers.is_empty() {
                let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::MyProfileError {
                    message: "No valid Blossom servers configured".to_string(),
                    toast: true,
                })));
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
                        mime_type.clone(),
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

                client.connect().await;
                client.wait_for_connection(Duration::from_secs(4)).await;
                match client.set_metadata(&metadata).await {
                    Ok(output) if !output.success.is_empty() => {
                        let _ =
                            tx.send(CoreMsg::Internal(Box::new(InternalEvent::MyProfileSaved {
                                metadata,
                            })));
                        return;
                    }
                    Ok(output) => {
                        let message = output
                            .failed
                            .values()
                            .next()
                            .map(ToString::to_string)
                            .unwrap_or_else(|| "no relay accepted profile update".to_string());
                        let _ =
                            tx.send(CoreMsg::Internal(Box::new(InternalEvent::MyProfileError {
                                message: format!("Profile picture update failed: {message}"),
                                toast: true,
                            })));
                        return;
                    }
                    Err(e) => {
                        let _ =
                            tx.send(CoreMsg::Internal(Box::new(InternalEvent::MyProfileError {
                                message: format!("Profile picture update failed: {e}"),
                                toast: true,
                            })));
                        return;
                    }
                }
            }

            let message = last_error
                .map(|e| format!("Blossom upload failed: {e}"))
                .unwrap_or_else(|| "Blossom upload failed".to_string());
            let _ = tx.send(CoreMsg::Internal(Box::new(InternalEvent::MyProfileError {
                message,
                toast: true,
            })));
        });
    }

    pub(super) fn apply_my_profile_metadata(&mut self, metadata: Option<Metadata>) {
        // Serialize to JSON and upsert into the shared profile cache —
        // same storage and picture-caching path as every other profile.
        if let Some(pk) = self.session.as_ref().map(|s| s.pubkey.to_hex()) {
            let metadata_json = metadata.and_then(|m| serde_json::to_string(&m).ok());
            self.upsert_profile(
                pk,
                ProfileCache::from_metadata_json(
                    metadata_json,
                    crate::state::now_seconds(),
                    crate::state::now_seconds(),
                ),
            );
        }

        let next = self.my_profile_state();
        if next != self.state.my_profile {
            self.state.my_profile = next;
            self.emit_state();
        }
    }

    fn metadata_for_profile_edit(&self, name: String, about: String) -> Metadata {
        // Reconstruct Metadata from the stored JSON, preserving fields we don't edit.
        let pk = self.session.as_ref().map(|s| s.pubkey.to_hex());
        let mut metadata: Metadata = pk
            .and_then(|pk| {
                // Try in-memory first (set by apply_my_profile_metadata), then fall back to DB.
                let in_mem = self.profiles.get(&pk).and_then(|p| p.metadata_json.clone());
                in_mem.or_else(|| {
                    self.profile_db
                        .as_ref()
                        .and_then(|conn| super::profile_db::load_metadata_json(conn, &pk))
                })
            })
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        let name = Self::normalized_profile_field(name);
        metadata.name = name.clone();
        metadata.display_name = name;
        metadata.about = Self::normalized_profile_field(about);
        metadata
    }

    /// Build current MyProfileState from the shared profile cache.
    pub(super) fn my_profile_state(&self) -> MyProfileState {
        let pk = self.session.as_ref().map(|s| s.pubkey.to_hex());
        let cached = pk.as_ref().and_then(|pk| self.profiles.get(pk));

        MyProfileState {
            name: cached.and_then(|p| p.name.clone()).unwrap_or_default(),
            about: cached.and_then(|p| p.about.clone()).unwrap_or_default(),
            picture_url: match (cached, pk.as_ref()) {
                (Some(p), Some(pk)) => p.display_picture_url(&self.data_dir, pk),
                _ => None,
            },
        }
    }

    fn normalized_profile_field(value: String) -> Option<String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
}
