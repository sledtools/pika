use anyhow::{Context, Result};
use nostr::{Event, PublicKey, RelayUrl, Tag};

use crate::PikaMls;

pub fn create_key_package_for_event<I>(
    mls: &PikaMls,
    public_key: &PublicKey,
    relays: I,
) -> Result<(String, Vec<Tag>, Vec<u8>)>
where
    I: IntoIterator<Item = RelayUrl>,
{
    mls.inner
        .create_key_package_for_event(public_key, relays)
        .context("create key package")
}

pub fn parse_key_package(mls: &PikaMls, event: &Event) -> Result<()> {
    mls.inner
        .parse_key_package(event)
        .map(|_| ())
        .context("parse key package")
}
