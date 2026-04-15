use crate::relay::PublishOutcome;
pub use pika_mls::membership::{
    EvolutionPublishStatus, MembershipRuntime, MembershipUpdateResult, PreparedMembershipEvolution,
    WelcomeDeliveryPlan,
};

impl pika_mls::membership::IntoEvolutionPublishStatus for PublishOutcome {
    fn into_evolution_publish_status(self) -> EvolutionPublishStatus {
        match self {
            PublishOutcome::Ok => EvolutionPublishStatus::Published,
            PublishOutcome::Err(err) => EvolutionPublishStatus::PublishFailed(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{PikaMdk, open_mdk};
    use nostr_sdk::prelude::{Event, EventBuilder, Keys, Kind, RelayUrl};
    use pika_mls::prelude::NostrGroupConfigData;
    use pika_mls::storage_traits::GroupId;

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

    fn create_base_group() -> (tempfile::TempDir, tempfile::TempDir, PikaMdk, GroupId, Keys) {
        let inviter_dir = tempfile::tempdir().expect("inviter tempdir");
        let invitee_dir = tempfile::tempdir().expect("invitee tempdir");
        let inviter_keys = Keys::generate();
        let invitee_keys = Keys::generate();
        let inviter_mdk = open_mdk(inviter_dir.path()).expect("open inviter mdk");
        let invitee_mdk = open_mdk(invitee_dir.path()).expect("open invitee mdk");

        let invitee_kp = make_key_package_event(&invitee_mdk, &invitee_keys);
        let config = NostrGroupConfigData::new(
            "Membership runtime".to_string(),
            String::new(),
            None,
            None,
            None,
            vec![RelayUrl::parse("wss://test.relay").expect("relay url")],
            vec![inviter_keys.public_key(), invitee_keys.public_key()],
        );
        let created = inviter_mdk
            .create_group(&inviter_keys.public_key(), vec![invitee_kp], config)
            .expect("create group");
        inviter_mdk
            .merge_pending_commit(&created.group.mls_group_id)
            .expect("merge initial commit");

        (
            inviter_dir,
            invitee_dir,
            inviter_mdk,
            created.group.mls_group_id,
            inviter_keys,
        )
    }

    #[test]
    fn prepare_add_members_validates_and_returns_welcome_plan() {
        let (_inviter_dir, _invitee_dir, inviter_mdk, group_id, _keys) = create_base_group();
        let peer_dir = tempfile::tempdir().expect("peer tempdir");
        let peer_keys = Keys::generate();
        let peer_mdk = open_mdk(peer_dir.path()).expect("open peer mdk");
        let peer_kp = make_key_package_event(&peer_mdk, &peer_keys);

        let prepared = MembershipRuntime::new(&inviter_mdk)
            .prepare_add_members(&group_id, &[peer_kp])
            .expect("prepare add members");

        assert_eq!(prepared.added_pubkeys, vec![peer_keys.public_key()]);
        assert_eq!(prepared.welcome_rumors.len(), 1);
        assert_eq!(prepared.evolution_event.kind, Kind::MlsGroupMessage);
        assert_eq!(
            prepared.expected_epoch,
            pika_mls::conversation::ConversationQueries::new(&inviter_mdk)
                .get_group(&group_id)
                .expect("get group")
                .expect("group")
                .epoch
        );
    }

    #[test]
    fn finalize_published_evolution_merges_and_returns_welcome_delivery() {
        let (_inviter_dir, _invitee_dir, inviter_mdk, group_id, _keys) = create_base_group();
        let peer_dir = tempfile::tempdir().expect("peer tempdir");
        let peer_keys = Keys::generate();
        let peer_mdk = open_mdk(peer_dir.path()).expect("open peer mdk");
        let peer_kp = make_key_package_event(&peer_mdk, &peer_keys);
        let runtime = MembershipRuntime::new(&inviter_mdk);

        let prepared = runtime
            .prepare_add_members(&group_id, &[peer_kp])
            .expect("prepare add members");

        let before_merge = pika_mls::conversation::ConversationQueries::new(&inviter_mdk)
            .get_members(&group_id)
            .expect("members before merge")
            .len();

        let finalized = runtime.finalize_published_evolution(prepared);

        let after_merge = pika_mls::conversation::ConversationQueries::new(&inviter_mdk)
            .get_members(&group_id)
            .expect("members after merge")
            .len();
        assert_eq!(before_merge + 1, after_merge);
        assert!(finalized.merge_error.is_none());
        assert_eq!(
            finalized
                .welcome_delivery
                .as_ref()
                .expect("welcome delivery")
                .recipients,
            vec![peer_keys.public_key()]
        );
    }

    #[test]
    fn prepare_remove_members_returns_publishable_evolution_without_welcomes() {
        let (_inviter_dir, _invitee_dir, inviter_mdk, group_id, inviter_keys) = create_base_group();
        let members = pika_mls::conversation::ConversationQueries::new(&inviter_mdk)
            .get_members(&group_id)
            .expect("get members before removal");
        let peer_pubkey = members
            .into_iter()
            .find(|pubkey| *pubkey != inviter_keys.public_key())
            .expect("invitee pubkey");

        let prepared = MembershipRuntime::new(&inviter_mdk)
            .prepare_remove_members(&group_id, &[peer_pubkey])
            .expect("prepare remove members");

        assert!(prepared.added_pubkeys.is_empty());
        assert_eq!(prepared.removed_pubkeys, vec![peer_pubkey]);
        assert!(!prepared.self_removed);
        assert!(prepared.welcome_rumors.is_empty());
        assert_eq!(prepared.evolution_event.kind, Kind::MlsGroupMessage);
    }

    #[test]
    fn prepare_leave_group_returns_publishable_evolution_without_welcomes() {
        let (_inviter_dir, _invitee_dir, inviter_mdk, group_id, _keys) = create_base_group();

        let prepared = MembershipRuntime::new(&inviter_mdk)
            .prepare_leave_group(&group_id)
            .expect("prepare leave group");

        assert!(prepared.added_pubkeys.is_empty());
        assert!(prepared.removed_pubkeys.is_empty());
        assert!(prepared.self_removed);
        assert!(prepared.welcome_rumors.is_empty());
        assert_eq!(prepared.evolution_event.kind, Kind::MlsGroupMessage);
    }

    #[tokio::test]
    async fn prepared_evolution_publish_status_tracks_shared_publish_outcome() {
        let (_inviter_dir, _invitee_dir, inviter_mdk, group_id, _keys) = create_base_group();
        let peer_dir = tempfile::tempdir().expect("peer tempdir");
        let peer_keys = Keys::generate();
        let peer_mdk = open_mdk(peer_dir.path()).expect("open peer mdk");
        let peer_kp = make_key_package_event(&peer_mdk, &peer_keys);

        let prepared = MembershipRuntime::new(&inviter_mdk)
            .prepare_add_members(&group_id, &[peer_kp])
            .expect("prepare add members");

        let ok = prepared
            .publish_with(|_| async { PublishOutcome::Ok })
            .await;
        assert!(matches!(ok, EvolutionPublishStatus::Published));

        let failed = prepared
            .publish_with(|_| async { PublishOutcome::Err("relay down".to_string()) })
            .await;
        assert!(matches!(
            failed,
            EvolutionPublishStatus::PublishFailed(ref err) if err == "relay down"
        ));
    }
}
