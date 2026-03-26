#[tokio::test]
async fn webhook_branch_ref_updates_request_forced_mirror() {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let ctx = WebTestContext::new();
    let mut state = ctx.state(forge_test_config());
    let secret = "webhook-secret".to_string();
    Arc::get_mut(&mut state)
        .expect("unique app state")
        .webhook_secret = Some(secret.clone());

    let payload = Bytes::from("oldsha newsha refs/heads/master\n");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("valid hmac key");
    mac.update(payload.as_ref());
    let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-pika-signature-256",
        HeaderValue::from_str(&signature).expect("signature header"),
    );

    let response = webhook_handler(State(Arc::clone(&state)), headers, payload)
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(state.forge_runtime.mirror_requested_for_test());
}

#[tokio::test]
async fn webhook_non_branch_updates_do_not_request_forced_mirror() {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let ctx = WebTestContext::new();
    let mut state = ctx.state(forge_test_config());
    let secret = "webhook-secret".to_string();
    Arc::get_mut(&mut state)
        .expect("unique app state")
        .webhook_secret = Some(secret.clone());

    let payload = Bytes::from("oldsha newsha refs/tags/v1\n");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("valid hmac key");
    mac.update(payload.as_ref());
    let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-pika-signature-256",
        HeaderValue::from_str(&signature).expect("signature header"),
    );

    let response = webhook_handler(State(Arc::clone(&state)), headers, payload)
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!state.forge_runtime.mirror_requested_for_test());
}

#[test]
fn managed_allowlist_backfills_only_for_new_reviewable_entries() {
    assert!(should_backfill_managed_allowlist_entry(None, true, false));
    assert!(!should_backfill_managed_allowlist_entry(None, false, false));

    let existing_active = ChatAllowlistEntry {
        npub: "npub1existing".to_string(),
        active: true,
        can_forge_write: false,
        note: Some("note".to_string()),
        updated_by: "npub1admin".to_string(),
        updated_at: "2026-03-08 00:00:00".to_string(),
    };
    assert!(!should_backfill_managed_allowlist_entry(
        Some(&existing_active),
        true,
        false
    ));
    assert!(!should_backfill_managed_allowlist_entry(
        Some(&existing_active),
        false,
        false
    ));

    let existing_inactive = ChatAllowlistEntry {
        active: false,
        can_forge_write: false,
        ..existing_active
    };
    assert!(should_backfill_managed_allowlist_entry(
        Some(&existing_inactive),
        true,
        false
    ));

    let existing_forge_only = ChatAllowlistEntry {
        active: false,
        can_forge_write: true,
        ..existing_inactive
    };
    assert!(should_backfill_managed_allowlist_entry(None, false, true));
    assert!(!should_backfill_managed_allowlist_entry(
        Some(&existing_forge_only),
        false,
        true
    ));
}

#[tokio::test]
async fn forge_only_writer_can_list_branch_inbox() {
    let ctx = WebTestContext::new();
    let store = ctx.store();
    let branch = store
        .upsert_branch_record(&branch_upsert_input("feature/forge-only-inbox", "head-1"))
        .expect("insert branch");
    let artifact_id = store
        .with_connection(|conn| {
            conn.query_row(
                "SELECT id
                 FROM branch_artifact_versions
                 WHERE branch_id = ?1
                 ORDER BY version DESC
                 LIMIT 1",
                rusqlite::params![branch.branch_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .expect("branch artifact id");
    let forge_only = "npub1umzqpag02ldgc9v8cww29vfmcqcrf28cyvd53ewhrk6zmgafdz9qaqfc58";
    store
        .mark_branch_generation_ready(
            artifact_id,
            r#"{"executive_summary":"ok","media_links":[],"steps":[]}"#,
            "<p>ok</p>",
            "head-1",
            "diff",
            None,
        )
        .expect("mark ready");
    store
        .upsert_chat_allowlist_entry(forge_only, false, true, Some("forge-only"), TRUSTED_NPUB)
        .expect("upsert forge-only writer");
    store
        .backfill_branch_inbox_for_npub(forge_only)
        .expect("backfill inbox");

    let state = ctx.state(forge_test_config());
    let headers = ctx.trusted_headers(forge_only);
    let response = api_inbox_list_handler(
        State(state.clone()),
        headers.clone(),
        Query(InboxListParams { page: Some(1) }),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let count_response = api_inbox_count_handler(State(state), headers)
        .await
        .into_response();
    assert_eq!(count_response.status(), StatusCode::OK);
}

#[tokio::test]
async fn forge_inbox_mark_reviewed_updates_review_needed_count() {
    let ctx = WebTestContext::new();
    let store = ctx.store();
    let branch = store
        .upsert_branch_record(&branch_upsert_input("feature/review-progress", "head-1"))
        .expect("insert branch");
    let artifact_id = store
        .with_connection(|conn| {
            conn.query_row(
                "SELECT id
                 FROM branch_artifact_versions
                 WHERE branch_id = ?1
                 ORDER BY version DESC
                 LIMIT 1",
                rusqlite::params![branch.branch_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .expect("branch artifact id");
    let reviewer = TRUSTED_NPUB;
    store
        .mark_branch_generation_ready(
            artifact_id,
            r#"{"executive_summary":"ok","media_links":[],"steps":[]}"#,
            "<p>ok</p>",
            "head-1",
            "diff",
            None,
        )
        .expect("mark ready");
    store
        .populate_branch_inbox(artifact_id, &[reviewer.to_string()])
        .expect("populate branch inbox");

    let state = ctx.state(forge_test_config());
    let headers = ctx.trusted_headers(reviewer);
    let response = api_inbox_mark_reviewed_handler(
        State(state.clone()),
        Path(branch.branch_id),
        headers.clone(),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);

    let count_response = api_inbox_count_handler(State(state), headers)
        .await
        .into_response();
    assert_eq!(count_response.status(), StatusCode::OK);
    let body = to_bytes(count_response.into_body(), usize::MAX)
        .await
        .expect("count body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("parse count json");
    assert_eq!(json["count"], 0);
}

#[tokio::test]
async fn inbox_review_route_resolves_branch_ids_in_forge_mode() {
    let ctx = WebTestContext::new();
    let store = ctx.store();
    let branch = store
        .upsert_branch_record(&branch_upsert_input("feature/review", "head-1"))
        .expect("insert branch");
    let artifact_id = store
        .with_connection(|conn| {
            conn.query_row(
                "SELECT id
                 FROM branch_artifact_versions
                 WHERE branch_id = ?1
                 ORDER BY version DESC
                 LIMIT 1",
                rusqlite::params![branch.branch_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .expect("branch artifact id");
    store
        .mark_branch_generation_ready(
            artifact_id,
            r#"{"executive_summary":"ok","media_links":[],"steps":[]}"#,
            "<p>ok</p>",
            "head-1",
            "diff",
            None,
        )
        .expect("mark ready");
    store
        .populate_branch_inbox(artifact_id, &["npub1reviewer".to_string()])
        .expect("populate branch inbox");

    let config = forge_test_config_without_admins();
    let state = ctx.state(config);

    let response = inbox_review_handler(State(state), Path(branch.branch_id))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
}

