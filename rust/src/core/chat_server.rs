use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use nostr_sdk::prelude::{Client, Event, EventBuilder, Kind, Tag, TagKind, ToBech32};
use pika_chat_server::protocol::{
    ClaimKeyPackageRequest, ClaimKeyPackageResponse, KeyPackageRecord, RegisterDeviceRequest,
    RegisterDeviceResponse, UploadKeyPackageRequest, UploadKeyPackageResponse,
};
use pika_chat_server::SessionTokenResponse;
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use url::Url;

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

pub async fn register_device(
    http_client: &reqwest::Client,
    base_url: &Url,
    access_token: &str,
    platform: Option<&str>,
    push_token: Option<&str>,
) -> Result<String> {
    let url = endpoint(base_url, "/v1/devices/register")?;
    let response: RegisterDeviceResponse = read_json(
        http_client
            .post(url)
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .json(&RegisterDeviceRequest {
                platform: platform
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string),
                push_token: push_token
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string),
            })
            .send()
            .await
            .context("send chat-server register-device request")?,
        "chat-server register device",
    )
    .await?;
    Ok(response.device.device_id)
}

pub async fn upload_key_package_event(
    http_client: &reqwest::Client,
    signer_client: &Client,
    base_url: &Url,
    platform: Option<&str>,
    push_token: Option<&str>,
    key_package_event: &Event,
) -> Result<KeyPackageRecord> {
    let access_token = login(http_client, signer_client, base_url).await?;
    let device_id =
        register_device(http_client, base_url, &access_token, platform, push_token).await?;
    let url = endpoint(base_url, "/v1/key-packages")?;
    read_json(
        http_client
            .post(url)
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .json(&UploadKeyPackageRequest {
                device_id,
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

pub fn platform_label() -> &'static str {
    std::env::consts::OS
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

        let uploaded = upload_key_package_event(
            &http_client,
            &alice_client,
            &base_url,
            Some("ios"),
            Some("push-token"),
            &key_package_event,
        )
        .await
        .expect("upload key package");
        assert_eq!(
            uploaded.owner_npub,
            alice_keys.public_key().to_bech32().unwrap().to_lowercase()
        );

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
