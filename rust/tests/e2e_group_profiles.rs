//! Focused relay-backed multi-app FFI profile semantics for groups and DMs.
//!
//! These tests stay below `pikahut` because they are the clearest owner for per-chat/profile MLS
//! behavior. `pikahut` now owns the main user-facing deterministic selectors in this family, but
//! not every narrow semantic/profile-state edge belongs there.

use std::time::Duration;

use pika_core::{AppAction, FfiApp};
use tempfile::tempdir;

mod support;
use support::{
    create_account_and_wait, create_or_open_dm_chat, get_logged_in_npub, wait_until, write_config,
};

/// Create a group chat where `creator` adds `peer_npub`, retrying until
/// key packages are available and the group appears in the chat list.
fn create_group_chat(
    creator: &FfiApp,
    peer_npub: &str,
    group_name: &str,
    timeout: Duration,
) -> String {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if let Some(chat) = creator
            .state()
            .chat_list
            .iter()
            .find(|c| c.group_name.as_deref() == Some(group_name))
        {
            return chat.chat_id.clone();
        }
        creator.dispatch(AppAction::CreateGroupChat {
            peer_npubs: vec![peer_npub.to_owned()],
            group_name: group_name.to_owned(),
        });
        std::thread::sleep(Duration::from_secs(2));
    }
    panic!("group '{group_name}' was not created within {timeout:?}");
}

#[test]
fn group_profile_visible_to_other_member() {
    let infra = support::TestInfra::start_relay();

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config(&dir_a.path().to_string_lossy(), &infra.relay_url);
    write_config(&dir_b.path().to_string_lossy(), &infra.relay_url);

    let alice = FfiApp::new(
        dir_a.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );
    let bob = FfiApp::new(
        dir_b.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );

    create_account_and_wait(&alice);
    create_account_and_wait(&bob);

    let bob_npub = get_logged_in_npub(&bob);

    // Alice creates a group with Bob.
    let chat_id = create_group_chat(&alice, &bob_npub, "ProfileTest", Duration::from_secs(60));

    // Wait for Bob to receive the group.
    wait_until("bob has group", Duration::from_secs(30), || {
        bob.state().chat_list.iter().any(|c| c.chat_id == chat_id)
    });

    // Alice sets a group profile.
    alice.dispatch(AppAction::SaveGroupProfile {
        chat_id: chat_id.clone(),
        name: "Alice in Wonderland".to_owned(),
        about: "curiouser and curiouser".to_owned(),
    });

    // Verify Alice sees her own group profile.
    alice.dispatch(AppAction::OpenChat {
        chat_id: chat_id.clone(),
    });
    wait_until(
        "alice sees own group profile",
        Duration::from_secs(10),
        || {
            alice
                .state()
                .current_chat
                .as_ref()
                .and_then(|c| c.my_group_profile.as_ref())
                .map(|p| p.name == "Alice in Wonderland")
                .unwrap_or(false)
        },
    );

    // Bob opens the chat and should see Alice's group-specific name.
    bob.dispatch(AppAction::OpenChat {
        chat_id: chat_id.clone(),
    });
    wait_until(
        "bob sees alice group profile name",
        Duration::from_secs(30),
        || {
            bob.state()
                .current_chat
                .as_ref()
                .map(|c| {
                    c.members
                        .iter()
                        .any(|m| m.name.as_deref() == Some("Alice in Wonderland"))
                })
                .unwrap_or(false)
        },
    );
}

#[test]
fn rebroadcasted_group_profiles_reach_late_joiner_member_state() {
    // `pikahut` now owns the readable visibility-after-refresh contract for this area. This test
    // stays as the narrower semantic owner for the relay-backed rebroadcast/member-state mechanics,
    // including reciprocal existing-member propagation and Charlie eventually seeing both names.
    let infra = support::TestInfra::start_relay();

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    let dir_c = tempdir().unwrap();
    write_config(&dir_a.path().to_string_lossy(), &infra.relay_url);
    write_config(&dir_b.path().to_string_lossy(), &infra.relay_url);
    write_config(&dir_c.path().to_string_lossy(), &infra.relay_url);

    let alice = FfiApp::new(
        dir_a.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );
    let bob = FfiApp::new(
        dir_b.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );
    let charlie = FfiApp::new(
        dir_c.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );

    create_account_and_wait(&alice);
    create_account_and_wait(&bob);
    create_account_and_wait(&charlie);

    let bob_npub = get_logged_in_npub(&bob);
    let charlie_npub = get_logged_in_npub(&charlie);

    // Alice creates a group with Bob.
    let chat_id = create_group_chat(
        &alice,
        &bob_npub,
        "RebroadcastTest",
        Duration::from_secs(60),
    );

    // Wait for Bob to receive the group.
    wait_until("bob has group", Duration::from_secs(30), || {
        bob.state().chat_list.iter().any(|c| c.chat_id == chat_id)
    });

    // Alice sets a group profile.
    alice.dispatch(AppAction::SaveGroupProfile {
        chat_id: chat_id.clone(),
        name: "Admin Alice".to_owned(),
        about: String::new(),
    });

    // Bob sets a group profile.
    bob.dispatch(AppAction::SaveGroupProfile {
        chat_id: chat_id.clone(),
        name: "Builder Bob".to_owned(),
        about: String::new(),
    });

    // Keep the reciprocal existing-member propagation semantics owned here even though `pikahut`
    // now owns a higher-level late-joiner visibility contract nearby.
    alice.dispatch(AppAction::OpenChat {
        chat_id: chat_id.clone(),
    });
    wait_until("alice sees bob group name", Duration::from_secs(30), || {
        alice
            .state()
            .current_chat
            .as_ref()
            .map(|c| {
                c.members
                    .iter()
                    .any(|m| m.name.as_deref() == Some("Builder Bob"))
            })
            .unwrap_or(false)
    });

    // Alice adds Charlie to the group.
    // Retry since Charlie's key package may not be published yet.
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        if charlie
            .state()
            .chat_list
            .iter()
            .any(|c| c.chat_id == chat_id)
        {
            break;
        }
        alice.dispatch(AppAction::AddGroupMembers {
            chat_id: chat_id.clone(),
            peer_npubs: vec![charlie_npub.clone()],
        });
        std::thread::sleep(Duration::from_secs(2));
    }

    // Wait for Charlie to receive the group.
    wait_until("charlie has group", Duration::from_secs(30), || {
        charlie
            .state()
            .chat_list
            .iter()
            .any(|c| c.chat_id == chat_id)
    });

    // Give Charlie time to set up group subscriptions.
    std::thread::sleep(Duration::from_secs(2));

    // Alice and Bob re-save their group profiles. In production this
    // happens automatically via rebroadcast on commit, but in the e2e
    // test the commit fires before Charlie subscribes to the group relay.
    alice.dispatch(AppAction::SaveGroupProfile {
        chat_id: chat_id.clone(),
        name: "Admin Alice".to_owned(),
        about: String::new(),
    });
    bob.dispatch(AppAction::SaveGroupProfile {
        chat_id: chat_id.clone(),
        name: "Builder Bob".to_owned(),
        about: String::new(),
    });

    // Charlie opens the chat and should see both profiles.
    charlie.dispatch(AppAction::OpenChat {
        chat_id: chat_id.clone(),
    });
    wait_until(
        "charlie sees alice group name",
        Duration::from_secs(30),
        || {
            charlie
                .state()
                .current_chat
                .as_ref()
                .map(|c| {
                    c.members
                        .iter()
                        .any(|m| m.name.as_deref() == Some("Admin Alice"))
                })
                .unwrap_or(false)
        },
    );
    wait_until(
        "charlie sees bob group name",
        Duration::from_secs(30),
        || {
            charlie
                .state()
                .current_chat
                .as_ref()
                .map(|c| {
                    c.members
                        .iter()
                        .any(|m| m.name.as_deref() == Some("Builder Bob"))
                })
                .unwrap_or(false)
        },
    );
}

#[test]
fn dm_profile_override_updates_owner_chat_state() {
    // `pikahut` now owns the broader DM-local visibility/scoping contract: Bob sees the override
    // inside the DM and it does not leak into a separate group chat. This test stays as the
    // narrower semantic owner for the owner-side per-chat profile state that backs that behavior.
    let infra = support::TestInfra::start_relay();

    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    write_config(&dir_a.path().to_string_lossy(), &infra.relay_url);
    write_config(&dir_b.path().to_string_lossy(), &infra.relay_url);

    let alice = FfiApp::new(
        dir_a.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );
    let bob = FfiApp::new(
        dir_b.path().to_string_lossy().to_string(),
        String::new(),
        String::new(),
    );

    create_account_and_wait(&alice);
    create_account_and_wait(&bob);

    let bob_npub = get_logged_in_npub(&bob);

    // Alice creates a DM with Bob (1:1 chat, not a named group).
    let chat_id = create_or_open_dm_chat(&alice, &bob_npub, Duration::from_secs(60));

    // Wait for Bob to see the chat.
    wait_until("bob has dm", Duration::from_secs(30), || {
        bob.state().chat_list.iter().any(|c| c.chat_id == chat_id)
    });

    // Alice sets a per-chat profile in the DM.
    alice.dispatch(AppAction::SaveGroupProfile {
        chat_id: chat_id.clone(),
        name: "DM Alice".to_owned(),
        about: "dm only".to_owned(),
    });

    // Verify Alice sees her own per-chat profile state.
    alice.dispatch(AppAction::OpenChat {
        chat_id: chat_id.clone(),
    });
    wait_until("alice sees own dm profile", Duration::from_secs(10), || {
        alice
            .state()
            .current_chat
            .as_ref()
            .and_then(|c| c.my_group_profile.as_ref())
            .map(|p| p.name == "DM Alice" && p.about == "dm only")
            .unwrap_or(false)
    });
}
