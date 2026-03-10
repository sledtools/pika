use anyhow::{Context, Result};
use mdk_core::prelude::NostrGroupConfigData;
use nostr_sdk::prelude::{Event, Keys, PublicKey, Tag, UnsignedEvent};

use crate::PikaMdk;
use crate::welcome::{PublishedWelcome, publish_welcome_rumors};

#[derive(Debug, Clone)]
pub struct GroupWelcomeDeliveryPlan {
    pub recipients: Vec<PublicKey>,
    pub welcome_rumors: Vec<UnsignedEvent>,
}

#[derive(Debug, Clone)]
pub struct PlannedGroupCreation {
    pub group: mdk_storage_traits::groups::types::Group,
    pub welcome_delivery: Option<GroupWelcomeDeliveryPlan>,
}

#[derive(Debug, Clone)]
pub struct CreatedGroup {
    pub group: mdk_storage_traits::groups::types::Group,
    pub published_welcomes: Vec<PublishedWelcome>,
}

pub fn create_group_and_plan_welcome_delivery(
    creator_pubkey: &PublicKey,
    mdk: &PikaMdk,
    peer_key_packages: Vec<Event>,
    config: NostrGroupConfigData,
    recipients: &[PublicKey],
) -> Result<PlannedGroupCreation> {
    if recipients.len() != peer_key_packages.len() {
        anyhow::bail!(
            "recipient/keypackage mismatch: {} recipients for {} key packages",
            recipients.len(),
            peer_key_packages.len()
        );
    }

    let result = mdk
        .create_group(creator_pubkey, peer_key_packages, config)
        .context("create group")?;

    let welcome_delivery = if result.welcome_rumors.is_empty() {
        None
    } else {
        Some(GroupWelcomeDeliveryPlan {
            recipients: recipients.to_vec(),
            welcome_rumors: result.welcome_rumors,
        })
    };

    Ok(PlannedGroupCreation {
        group: result.group,
        welcome_delivery,
    })
}

pub async fn publish_group_welcome_delivery<F, Fut>(
    keys: &Keys,
    plan: &GroupWelcomeDeliveryPlan,
    welcome_tags: Vec<Tag>,
    publish_giftwrap: F,
) -> Result<Vec<PublishedWelcome>>
where
    F: FnMut(PublicKey, Event) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    publish_welcome_rumors(
        keys,
        &plan.welcome_rumors,
        &plan.recipients,
        welcome_tags,
        publish_giftwrap,
    )
    .await
}

pub async fn create_group_and_publish_welcomes<F, Fut>(
    keys: &Keys,
    mdk: &PikaMdk,
    peer_key_packages: Vec<Event>,
    config: NostrGroupConfigData,
    recipients: &[PublicKey],
    welcome_tags: Vec<Tag>,
    publish_giftwrap: F,
) -> Result<CreatedGroup>
where
    F: FnMut(PublicKey, Event) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let creator_pubkey = keys.public_key();
    let planned = create_group_and_plan_welcome_delivery(
        &creator_pubkey,
        mdk,
        peer_key_packages,
        config,
        recipients,
    )?;
    let published_welcomes = match planned.welcome_delivery.as_ref() {
        Some(plan) => {
            publish_group_welcome_delivery(keys, plan, welcome_tags, publish_giftwrap).await?
        }
        None => Vec::new(),
    };

    Ok(CreatedGroup {
        group: planned.group,
        published_welcomes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::open_mdk;
    use mdk_core::prelude::NostrGroupConfigData;
    use nostr_sdk::prelude::{EventBuilder, Kind, RelayUrl};

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
    fn create_group_and_plan_welcome_delivery_returns_group_and_welcome_plan() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime create plan test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );

        let planned = create_group_and_plan_welcome_delivery(
            &inviter_keys.public_key(),
            &inviter_mdk,
            vec![invitee_kp],
            config,
            &[invitee_keys.public_key()],
        )
        .expect("create group and plan welcomes");

        assert_eq!(planned.group.name, "Runtime create plan test");
        let welcome_delivery = planned.welcome_delivery.expect("welcome delivery plan");
        assert_eq!(welcome_delivery.recipients, vec![invitee_keys.public_key()]);
        assert_eq!(welcome_delivery.welcome_rumors.len(), 1);
    }

    #[test]
    fn create_group_and_plan_welcome_delivery_returns_no_plan_without_recipients() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let inviter_keys = Keys::generate();
        let inviter_mdk = open_mdk(inviter_dir.path()).expect("open inviter mdk");

        let config = NostrGroupConfigData::new(
            "Runtime local create plan test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key()],
        );

        let planned = create_group_and_plan_welcome_delivery(
            &inviter_keys.public_key(),
            &inviter_mdk,
            vec![],
            config,
            &[],
        )
        .expect("create local-only group");

        assert_eq!(planned.group.name, "Runtime local create plan test");
        assert!(
            planned.welcome_delivery.is_none(),
            "local-only create should not enqueue welcome delivery work"
        );
    }

    #[tokio::test]
    async fn create_group_and_publish_welcomes_returns_group_and_published_metadata() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime create test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );

        let published = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Event>::new()));
        let published_capture = std::sync::Arc::clone(&published);
        let mut created = create_group_and_publish_welcomes(
            &inviter_keys,
            &inviter_mdk,
            vec![invitee_kp],
            config,
            &[invitee_keys.public_key()],
            vec![],
            move |_receiver, giftwrap| {
                let published_capture = std::sync::Arc::clone(&published_capture);
                async move {
                    published_capture
                        .lock()
                        .expect("published lock")
                        .push(giftwrap);
                    Ok(())
                }
            },
        )
        .await
        .expect("create group and publish welcomes");

        assert_eq!(created.group.name, "Runtime create test");
        assert_eq!(created.published_welcomes.len(), 1);
        assert_eq!(
            created.published_welcomes[0].receiver,
            invitee_keys.public_key()
        );
        let published_welcome = &mut created.published_welcomes[0];
        let rumor_id = published_welcome.rumor.id();
        assert_eq!(published_welcome.welcome_event_id, rumor_id);

        let published = published.lock().expect("published lock");
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].kind, Kind::GiftWrap);
        assert_eq!(
            created.published_welcomes[0].wrapper_event_id,
            published[0].id
        );
    }

    #[test]
    fn create_group_and_plan_welcome_delivery_rejects_mismatch_before_create() {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Runtime create mismatch test".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );

        let err = create_group_and_plan_welcome_delivery(
            &inviter_keys.public_key(),
            &inviter_mdk,
            vec![invitee_kp],
            config,
            &[],
        )
        .expect_err("recipient/keypackage mismatch should fail");

        assert!(err.to_string().contains("recipient/keypackage mismatch"));
        assert!(
            inviter_mdk.get_groups().expect("get groups").is_empty(),
            "helper should fail before creating a local group"
        );
    }
}
