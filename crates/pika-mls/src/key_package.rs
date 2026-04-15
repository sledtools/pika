use anyhow::{Context, Result};
use nostr::{Event, PublicKey, RelayUrl, Tag};

use crate::PikaMdk;

pub fn create_key_package_for_event<I>(
    mdk: &PikaMdk,
    public_key: &PublicKey,
    relays: I,
) -> Result<(String, Vec<Tag>, Vec<u8>)>
where
    I: IntoIterator<Item = RelayUrl>,
{
    mdk.inner
        .create_key_package_for_event(public_key, relays)
        .context("create key package")
}

pub fn parse_key_package(mdk: &PikaMdk, event: &Event) -> Result<()> {
    mdk.inner
        .parse_key_package(event)
        .map(|_| ())
        .context("parse key package")
}
