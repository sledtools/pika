use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use nostr_sdk::prelude::{Client, Event, EventBuilder, Kind, Tag, TagKind, ToBech32};
use pika_chat_server::protocol::{
    AppendRoomEventRequest, AppendRoomEventResponse, ClaimKeyPackageRequest,
    ClaimKeyPackageResponse, ClaimWelcomesResponse, CreateRoomRequest, CreateRoomResponse,
    KeyPackageRecord, RoomEvent, RoomEventType, RoomSummary, SubmitMembershipCommitRequest,
    SubmitMembershipCommitResponse, SyncRoomEventsResponse, UploadKeyPackageRequest,
    UploadKeyPackageResponse, UploadWelcomeRequest, UploadWelcomeResponse, WelcomeEnvelope,
};
use pika_chat_server::SessionTokenResponse;
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WelcomeRoomBinding {
    pub server_url: String,
    pub room_id: String,
}

#[derive(Debug, Clone)]
pub struct ClaimedWelcomeEvent {
    pub wrapper: Event,
    pub rumor: nostr_sdk::prelude::UnsignedEvent,
    pub room_binding: Option<WelcomeRoomBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitMembershipCommitError {
    StaleEpochConflict(String),
    RequestFailed(String),
}

impl SubmitMembershipCommitError {
    pub fn is_stale_epoch_conflict(&self) -> bool {
        matches!(self, Self::StaleEpochConflict(_))
    }
}

impl std::fmt::Display for SubmitMembershipCommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleEpochConflict(message) | Self::RequestFailed(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for SubmitMembershipCommitError {}

fn endpoint(base_url: &Url, path: &str) -> Result<Url> {
    base_url
        .join(path.trim_start_matches('/'))
        .with_context(|| format!("join chat server URL path `{path}`"))
}

async fn build_nip98_authorization_header(
    signer_client: &Client,
    method: &Method,
    url: &str,
) -> Result<String> {
    let event = signer_client
        .sign_event_builder(EventBuilder::new(Kind::Custom(27235), "").tags([
            Tag::custom(TagKind::custom("u"), [url]),
            Tag::custom(
                TagKind::custom("method"),
                [method.as_str().to_ascii_uppercase()],
            ),
        ]))
        .await
        .context("sign NIP-98 event")?;
    let payload = serde_json::to_vec(&event).context("serialize NIP-98 event")?;
    Ok(format!("Nostr {}", STANDARD.encode(payload)))
}

async fn read_json<T>(response: reqwest::Response, context: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("{context}: read response body"))?;
    if !status.is_success() {
        let detail = String::from_utf8_lossy(&bytes).trim().to_string();
        if detail.is_empty() {
            bail!("{context}: request failed with status {status}");
        }
        bail!("{context}: request failed with status {status}: {detail}");
    }
    serde_json::from_slice(&bytes).with_context(|| format!("{context}: decode JSON body"))
}

pub async fn login(
    http_client: &reqwest::Client,
    signer_client: &Client,
    base_url: &Url,
) -> Result<String> {
    let url = endpoint(base_url, "/v1/session/login")?;
    let method = Method::POST;
    let authorization = build_nip98_authorization_header(signer_client, &method, url.as_str())
        .await
        .context("build chat-server login authorization")?;
    let response: SessionTokenResponse = read_json(
        http_client
            .request(method, url)
            .header("Authorization", authorization)
            .header("Accept", "application/json")
            .send()
            .await
            .context("send chat-server login request")?,
        "chat-server login",
    )
    .await?;
    Ok(response.access_token)
}

pub async fn create_room(
    http_client: &reqwest::Client,
    signer_client: &Client,
    base_url: &Url,
    member_npubs: Vec<String>,
    epoch: u64,
) -> Result<RoomSummary> {
    let access_token = login(http_client, signer_client, base_url).await?;
    let url = endpoint(base_url, "/v1/rooms")?;
    read_json(
        http_client
            .post(url)
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .json(&CreateRoomRequest {
                member_npubs,
                epoch: Some(epoch),
            })
            .send()
            .await
            .context("send chat-server create-room request")?,
        "chat-server create room",
    )
    .await
    .map(|response: CreateRoomResponse| response.room)
}

pub async fn append_room_event(
    http_client: &reqwest::Client,
    signer_client: &Client,
    base_url: &Url,
    room_id: &str,
    request: AppendRoomEventRequest,
) -> Result<RoomEvent> {
    let access_token = login(http_client, signer_client, base_url).await?;
    let url = endpoint(base_url, &format!("/v1/rooms/{room_id}/events"))?;
    read_json(
        http_client
            .post(url)
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .json(&request)
            .send()
            .await
            .context("send chat-server append-room-event request")?,
        "chat-server append room event",
    )
    .await
    .map(|response: AppendRoomEventResponse| response.event)
}

pub async fn append_wrapped_room_event(
    http_client: &reqwest::Client,
    signer_client: &Client,
    base_url: &Url,
    room_id: &str,
    event_type: RoomEventType,
    wrapper: &Event,
) -> Result<RoomEvent> {
    append_room_event(
        http_client,
        signer_client,
        base_url,
        room_id,
        AppendRoomEventRequest {
            event_type,
            epoch: 0,
            sender_device_id: None,
            content: serde_json::to_string(wrapper).context("serialize wrapped room event")?,
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn submit_membership_commit(
    http_client: &reqwest::Client,
    signer_client: &Client,
    base_url: &Url,
    room_id: &str,
    expected_epoch: u64,
    member_npubs: Vec<String>,
    wrapper: &Event,
    welcomes: Vec<WelcomeEnvelope>,
) -> std::result::Result<(RoomSummary, RoomEvent), SubmitMembershipCommitError> {
    let access_token = login(http_client, signer_client, base_url)
        .await
        .map_err(|err| {
            SubmitMembershipCommitError::RequestFailed(format!(
                "chat-server submit membership commit: login failed: {err}"
            ))
        })?;
    let url =
        endpoint(base_url, &format!("/v1/rooms/{room_id}/membership-commits")).map_err(|err| {
            SubmitMembershipCommitError::RequestFailed(format!(
                "chat-server submit membership commit: build endpoint failed: {err}"
            ))
        })?;
    let response = http_client
        .post(url)
        .bearer_auth(access_token)
        .header("Accept", "application/json")
        .json(&SubmitMembershipCommitRequest {
            expected_epoch,
            sender_device_id: None,
            member_npubs,
            wrapper_event_json: serde_json::to_string(wrapper)
                .map_err(|err| {
                    SubmitMembershipCommitError::RequestFailed(format!(
                        "chat-server submit membership commit: serialize membership commit wrapper: {err}"
                    ))
                })?,
            welcomes,
        })
        .send()
        .await
        .map_err(|err| {
            SubmitMembershipCommitError::RequestFailed(format!(
                "chat-server submit membership commit: send request failed: {err}"
            ))
        })?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|err| {
        SubmitMembershipCommitError::RequestFailed(format!(
            "chat-server submit membership commit: read response body failed: {err}"
        ))
    })?;
    if !status.is_success() {
        let detail = String::from_utf8_lossy(&bytes).trim().to_string();
        let message = if detail.is_empty() {
            format!("chat-server submit membership commit: request failed with status {status}")
        } else {
            format!(
                "chat-server submit membership commit: request failed with status {status}: {detail}"
            )
        };
        return Err(if status == StatusCode::CONFLICT {
            SubmitMembershipCommitError::StaleEpochConflict(message)
        } else {
            SubmitMembershipCommitError::RequestFailed(message)
        });
    }
    serde_json::from_slice::<SubmitMembershipCommitResponse>(&bytes)
        .map(|response| (response.room, response.event))
        .map_err(|err| {
            SubmitMembershipCommitError::RequestFailed(format!(
                "chat-server submit membership commit: decode JSON body failed: {err}"
            ))
        })
}

pub async fn sync_room_events(
    http_client: &reqwest::Client,
    signer_client: &Client,
    base_url: &Url,
    room_id: &str,
    after_seq: u64,
    limit: usize,
) -> Result<SyncRoomEventsResponse> {
    let access_token = login(http_client, signer_client, base_url).await?;
    let mut url = endpoint(base_url, &format!("/v1/rooms/{room_id}/events"))?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("after_seq", &after_seq.to_string());
        query.append_pair("limit", &limit.to_string());
    }
    read_json(
        http_client
            .get(url)
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .send()
            .await
            .context("send chat-server sync-room-events request")?,
        "chat-server sync room events",
    )
    .await
}

pub async fn upload_welcome_event(
    http_client: &reqwest::Client,
    signer_client: &Client,
    base_url: &Url,
    recipient_npub: &str,
    wrapper: &Event,
    room_binding: Option<&WelcomeRoomBinding>,
) -> Result<()> {
    let access_token = login(http_client, signer_client, base_url).await?;
    let url = endpoint(base_url, "/v1/welcomes")?;
    let _: UploadWelcomeResponse = read_json(
        http_client
            .post(url)
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .json(&UploadWelcomeRequest {
                recipient_npub: recipient_npub.trim().to_ascii_lowercase(),
                wrapper_event_json: serde_json::to_string(wrapper)
                    .context("serialize welcome wrapper")?,
                server_url: room_binding.map(|binding| binding.server_url.clone()),
                room_id: room_binding.map(|binding| binding.room_id.clone()),
            })
            .send()
            .await
            .context("send chat-server upload-welcome request")?,
        "chat-server upload welcome",
    )
    .await?;
    Ok(())
}

pub async fn claim_welcome_events(
    http_client: &reqwest::Client,
    signer_client: &Client,
    base_url: &Url,
) -> Result<Vec<ClaimedWelcomeEvent>> {
    let access_token = login(http_client, signer_client, base_url).await?;
    let url = endpoint(base_url, "/v1/welcomes/claim")?;
    let claimed: ClaimWelcomesResponse = read_json(
        http_client
            .post(url)
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .send()
            .await
            .context("send chat-server claim-welcomes request")?,
        "chat-server claim welcomes",
    )
    .await?;

    let mut welcomes = Vec::with_capacity(claimed.welcomes.len());
    for record in claimed.welcomes {
        let wrapper: Event = serde_json::from_str(&record.wrapper_event_json)
            .context("decode claimed welcome wrapper")?;
        let unwrapped = signer_client
            .unwrap_gift_wrap(&wrapper)
            .await
            .context("unwrap claimed welcome giftwrap")?;
        let room_binding = match (record.server_url, record.room_id) {
            (Some(server_url), Some(room_id)) => Some(WelcomeRoomBinding {
                server_url,
                room_id,
            }),
            _ => None,
        };
        welcomes.push(ClaimedWelcomeEvent {
            wrapper,
            rumor: unwrapped.rumor,
            room_binding,
        });
    }
    Ok(welcomes)
}

pub async fn upload_key_package_event(
    http_client: &reqwest::Client,
    signer_client: &Client,
    base_url: &Url,
    key_package_event: &Event,
) -> Result<KeyPackageRecord> {
    let access_token = login(http_client, signer_client, base_url).await?;
    let url = endpoint(base_url, "/v1/key-packages")?;
    read_json(
        http_client
            .post(url)
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .json(&UploadKeyPackageRequest {
                device_id: None,
                ciphersuite: None,
                payload: serde_json::to_string(key_package_event)
                    .context("serialize key package event payload")?,
            })
            .send()
            .await
            .context("send chat-server upload-key-package request")?,
        "chat-server upload key package",
    )
    .await
    .map(|response: UploadKeyPackageResponse| response.key_package)
}

pub async fn claim_key_package_event(
    http_client: &reqwest::Client,
    signer_client: &Client,
    base_url: &Url,
    owner_npub: &str,
    room_id: Option<&str>,
) -> Result<Option<Event>> {
    let access_token = login(http_client, signer_client, base_url).await?;
    let url = endpoint(base_url, "/v1/key-packages/claim")?;
    let response = http_client
        .post(url)
        .bearer_auth(access_token)
        .header("Accept", "application/json")
        .json(&ClaimKeyPackageRequest {
            owner_npub: owner_npub.trim().to_ascii_lowercase(),
            room_id: room_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string),
        })
        .send()
        .await
        .context("send chat-server claim-key-package request")?;

    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }

    let claimed: ClaimKeyPackageResponse =
        read_json(response, "chat-server claim key package").await?;
    let event: Event = serde_json::from_str(&claimed.key_package.payload)
        .context("decode claimed key package event payload")?;
    Ok(Some(event))
}

pub fn peer_npub(peer_pubkey: &nostr_sdk::prelude::PublicKey) -> Result<String> {
    peer_pubkey
        .to_bech32()
        .map(|npub| npub.to_lowercase())
        .map_err(|err| anyhow!("encode peer npub: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::SocketAddr;

    use nostr_sdk::prelude::{Keys, ToBech32};
    use pika_chat_server::store::StoreHandle;
    use pika_chat_server::{router, AppState, SessionManager};

    async fn spawn_test_server() -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let state = AppState {
            sessions: SessionManager::new([3u8; 32], 600),
            trust_forwarded_host: false,
            store: StoreHandle::in_memory(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("listener addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, router(state))
                .await
                .expect("serve chat server test app");
        });
        (addr, handle)
    }

    #[tokio::test]
    async fn upload_and_claim_key_package_round_trip() {
        let (addr, handle) = spawn_test_server().await;
        let base_url = Url::parse(&format!("http://{addr}/")).expect("base url");
        let http_client = reqwest::Client::new();

        let alice_keys = Keys::generate();
        let alice_client = Client::builder().signer(alice_keys.clone()).build();
        let bob_keys = Keys::generate();
        let bob_client = Client::builder().signer(bob_keys).build();

        let key_package_event = EventBuilder::new(Kind::MlsKeyPackage, "opaque-key-package")
            .sign_with_keys(&alice_keys)
            .expect("sign key package event");

        let uploaded =
            upload_key_package_event(&http_client, &alice_client, &base_url, &key_package_event)
                .await
                .expect("upload key package");
        assert_eq!(
            uploaded.owner_npub,
            alice_keys.public_key().to_bech32().unwrap().to_lowercase()
        );
        assert_eq!(uploaded.device_id, None);

        let claimed = claim_key_package_event(
            &http_client,
            &bob_client,
            &base_url,
            &alice_keys.public_key().to_bech32().unwrap(),
            None,
        )
        .await
        .expect("claim key package")
        .expect("claimed key package");
        assert_eq!(claimed.id, key_package_event.id);

        handle.abort();
    }

    #[tokio::test]
    async fn create_room_round_trip() {
        let (addr, handle) = spawn_test_server().await;
        let base_url = Url::parse(&format!("http://{addr}/")).expect("base url");
        let http_client = reqwest::Client::new();

        let alice_keys = Keys::generate();
        let alice_npub = alice_keys.public_key().to_bech32().unwrap().to_lowercase();
        let bob_keys = Keys::generate();
        let bob_npub = bob_keys.public_key().to_bech32().unwrap().to_lowercase();
        let alice_client = Client::builder().signer(alice_keys).build();
        let room = create_room(
            &http_client,
            &alice_client,
            &base_url,
            vec![bob_npub.clone()],
            0,
        )
        .await
        .expect("create room");

        assert_eq!(room.members.len(), 2);
        assert_eq!(room.last_seq, 0);
        assert!(room.members.contains(&alice_npub));
        assert!(room.members.contains(&bob_npub));

        handle.abort();
    }

    #[tokio::test]
    async fn append_and_sync_wrapped_room_event_round_trip() {
        let (addr, handle) = spawn_test_server().await;
        let base_url = Url::parse(&format!("http://{addr}/")).expect("base url");
        let http_client = reqwest::Client::new();

        let alice_keys = Keys::generate();
        let alice_client = Client::builder().signer(alice_keys.clone()).build();
        let bob_keys = Keys::generate();
        let bob_npub = bob_keys.public_key().to_bech32().unwrap().to_lowercase();
        let bob_client = Client::builder().signer(bob_keys).build();

        let room = create_room(&http_client, &alice_client, &base_url, vec![bob_npub], 0)
            .await
            .expect("create room");
        let wrapper = EventBuilder::new(Kind::MlsGroupMessage, "opaque-wrapper")
            .sign_with_keys(&alice_keys)
            .expect("sign wrapper");

        let appended = append_wrapped_room_event(
            &http_client,
            &alice_client,
            &base_url,
            &room.room_id,
            RoomEventType::ApplicationMessage,
            &wrapper,
        )
        .await
        .expect("append room event");
        assert_eq!(appended.seq, 1);
        assert_eq!(appended.event_type, RoomEventType::ApplicationMessage);

        let synced = sync_room_events(&http_client, &bob_client, &base_url, &room.room_id, 0, 10)
            .await
            .expect("sync room events");
        assert_eq!(synced.room.last_seq, 1);
        assert_eq!(synced.events.len(), 1);

        let decoded: Event =
            serde_json::from_str(&synced.events[0].content).expect("decode synced wrapper");
        assert_eq!(decoded.id, wrapper.id);

        handle.abort();
    }

    #[tokio::test]
    async fn submit_membership_commit_round_trip() {
        let (addr, handle) = spawn_test_server().await;
        let base_url = Url::parse(&format!("http://{addr}/")).expect("base url");
        let http_client = reqwest::Client::new();

        let alice_keys = Keys::generate();
        let alice_npub = alice_keys.public_key().to_bech32().unwrap().to_lowercase();
        let alice_client = Client::builder().signer(alice_keys.clone()).build();
        let bob_keys = Keys::generate();
        let bob_npub = bob_keys.public_key().to_bech32().unwrap().to_lowercase();
        let carol_keys = Keys::generate();
        let carol_npub = carol_keys.public_key().to_bech32().unwrap().to_lowercase();
        let carol_client = Client::builder().signer(carol_keys).build();

        let room = create_room(
            &http_client,
            &alice_client,
            &base_url,
            vec![bob_npub.clone()],
            0,
        )
        .await
        .expect("create room");
        let wrapper = EventBuilder::new(Kind::MlsGroupMessage, "opaque-membership-commit")
            .sign_with_keys(&alice_keys)
            .expect("sign membership commit wrapper");

        let (updated_room, appended) = submit_membership_commit(
            &http_client,
            &alice_client,
            &base_url,
            &room.room_id,
            0,
            vec![alice_npub, bob_npub, carol_npub.clone()],
            &wrapper,
            vec![WelcomeEnvelope {
                recipient_npub: carol_npub.clone(),
                wrapper_event_json: "{\"kind\":1059,\"content\":\"welcome\"}".to_string(),
                server_url: Some("https://chat.example".to_string()),
                room_id: Some(room.room_id.clone()),
            }],
        )
        .await
        .expect("submit membership commit");

        assert_eq!(updated_room.epoch, 1);
        assert_eq!(updated_room.last_seq, 1);
        assert!(updated_room.members.contains(&carol_npub));
        assert_eq!(appended.event_type, RoomEventType::Commit);
        assert_eq!(appended.seq, 1);
        assert_eq!(appended.epoch, 1);

        let synced = sync_room_events(&http_client, &carol_client, &base_url, &room.room_id, 0, 10)
            .await
            .expect("sync room events");
        assert_eq!(synced.room.epoch, 1);
        assert_eq!(synced.events.len(), 1);

        handle.abort();
    }

    #[tokio::test]
    async fn submit_membership_commit_reports_stale_epoch_conflict() {
        let (addr, handle) = spawn_test_server().await;
        let base_url = Url::parse(&format!("http://{addr}/")).expect("base url");
        let http_client = reqwest::Client::new();

        let alice_keys = Keys::generate();
        let alice_client = Client::builder().signer(alice_keys.clone()).build();
        let bob_keys = Keys::generate();
        let bob_npub = bob_keys.public_key().to_bech32().unwrap().to_lowercase();

        let room = create_room(
            &http_client,
            &alice_client,
            &base_url,
            vec![bob_npub.clone()],
            0,
        )
        .await
        .expect("create room");
        let wrapper = EventBuilder::new(Kind::MlsGroupMessage, "stale-membership-commit")
            .sign_with_keys(&alice_keys)
            .expect("sign membership commit wrapper");

        let err = submit_membership_commit(
            &http_client,
            &alice_client,
            &base_url,
            &room.room_id,
            1,
            vec![bob_npub],
            &wrapper,
            Vec::new(),
        )
        .await
        .expect_err("stale epoch should fail");
        assert!(err.is_stale_epoch_conflict());
        assert!(err.to_string().contains("expected 1, actual 0"));

        handle.abort();
    }

    #[tokio::test]
    async fn upload_and_claim_welcome_round_trip() {
        let (addr, handle) = spawn_test_server().await;
        let base_url = Url::parse(&format!("http://{addr}/")).expect("base url");
        let http_client = reqwest::Client::new();

        let sender_keys = Keys::generate();
        let sender_client = Client::builder().signer(sender_keys.clone()).build();
        let receiver_keys = Keys::generate();
        let receiver_npub = receiver_keys
            .public_key()
            .to_bech32()
            .unwrap()
            .to_lowercase();
        let receiver_client = Client::builder().signer(receiver_keys.clone()).build();
        let rumor = EventBuilder::new(Kind::MlsWelcome, "opaque-welcome-rumor")
            .sign_with_keys(&sender_keys)
            .expect("sign welcome rumor");
        let wrapper = EventBuilder::gift_wrap(
            &sender_keys,
            &receiver_keys.public_key(),
            rumor.clone().into(),
            vec![],
        )
        .await
        .expect("gift wrap welcome rumor");

        upload_welcome_event(
            &http_client,
            &sender_client,
            &base_url,
            &receiver_npub,
            &wrapper,
            Some(&WelcomeRoomBinding {
                server_url: "https://chat.example".to_string(),
                room_id: "room_123".to_string(),
            }),
        )
        .await
        .expect("upload welcome event");

        let mut claimed = claim_welcome_events(&http_client, &receiver_client, &base_url)
            .await
            .expect("claim welcome events");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].wrapper.id, wrapper.id);
        assert_eq!(claimed[0].rumor.id(), rumor.id);
        assert_eq!(
            claimed[0]
                .room_binding
                .as_ref()
                .map(|binding| binding.server_url.as_str()),
            Some("https://chat.example")
        );
        assert_eq!(
            claimed[0]
                .room_binding
                .as_ref()
                .map(|binding| binding.room_id.as_str()),
            Some("room_123")
        );

        let claimed_again = claim_welcome_events(&http_client, &receiver_client, &base_url)
            .await
            .expect("claim welcome events again");
        assert!(claimed_again.is_empty());

        handle.abort();
    }

    #[tokio::test]
    async fn claim_key_package_allows_room_scoped_member_claim() {
        let (addr, handle) = spawn_test_server().await;
        let base_url = Url::parse(&format!("http://{addr}/")).expect("base url");
        let http_client = reqwest::Client::new();

        let owner_keys = Keys::generate();
        let owner_npub = owner_keys.public_key().to_bech32().unwrap().to_lowercase();
        let owner_client = Client::builder().signer(owner_keys.clone()).build();
        let claimer_keys = Keys::generate();
        let claimer_client = Client::builder().signer(claimer_keys).build();

        let key_package_event = EventBuilder::new(Kind::MlsKeyPackage, "opaque-key-package")
            .sign_with_keys(&owner_keys)
            .expect("sign key package event");
        upload_key_package_event(&http_client, &owner_client, &base_url, &key_package_event)
            .await
            .expect("upload key package");

        let room = create_room(
            &http_client,
            &claimer_client,
            &base_url,
            vec![owner_npub.clone()],
            0,
        )
        .await
        .expect("create room");

        let claimed = claim_key_package_event(
            &http_client,
            &claimer_client,
            &base_url,
            &owner_npub,
            Some(&room.room_id),
        )
        .await
        .expect("claim room-scoped key package")
        .expect("key package should exist");
        assert_eq!(claimed.id, key_package_event.id);

        handle.abort();
    }

    #[tokio::test]
    async fn claim_returns_none_when_no_key_package_exists() {
        let (addr, handle) = spawn_test_server().await;
        let base_url = Url::parse(&format!("http://{addr}/")).expect("base url");
        let http_client = reqwest::Client::new();
        let claimer_keys = Keys::generate();
        let claimer_client = Client::builder().signer(claimer_keys).build();

        let missing_owner = Keys::generate()
            .public_key()
            .to_bech32()
            .expect("missing owner npub");

        let claimed = claim_key_package_event(
            &http_client,
            &claimer_client,
            &base_url,
            &missing_owner,
            None,
        )
        .await
        .expect("claim request should succeed");
        assert!(claimed.is_none());

        handle.abort();
    }
}
