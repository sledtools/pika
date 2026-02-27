pub use pika_marmot_runtime::relay::{check_relay_ready, subscribe_group_msgs};

pub async fn connect_client(
    keys: &nostr_sdk::prelude::Keys,
    relay: &str,
) -> anyhow::Result<nostr_sdk::prelude::Client> {
    pika_marmot_runtime::relay::connect_client(keys, &[relay.to_string()]).await
}
