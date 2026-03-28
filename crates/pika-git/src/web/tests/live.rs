#[tokio::test]
async fn completed_branch_ci_stream_returns_initial_snapshot_and_closes() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let branch = store
        .upsert_branch_record(&branch_upsert_input("feature/live-branch", "head-live"))
        .expect("insert branch");
    store
        .queue_branch_ci_run_for_head(
            branch.branch_id,
            "head-live",
            &[crate::ci_manifest::ForgeLane {
                id: "pika".to_string(),
                title: "check-pika".to_string(),
                entrypoint: "just checks::pre-merge-pika".to_string(),
                command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
                paths: vec![],
                concurrency_group: None,
            }],
        )
        .expect("queue branch ci");
    let lane = store
        .claim_pending_branch_ci_lane_runs(1, 120)
        .expect("claim branch lane")
        .into_iter()
        .next()
        .expect("branch lane");
    store
        .finish_branch_ci_lane_run(
            lane.lane_run_id,
            lane.claim_token,
            crate::ci_state::CiLaneStatus::Success,
            "ok",
        )
        .expect("finish branch lane");

    let mut config = forge_test_config_without_admins();
    config
        .forge_repo
        .as_mut()
        .expect("forge repo")
        .ci_concurrency = None;
    let state = test_state(store, config);
    let response = branch_ci_stream_handler(
        State(state),
        Path(branch.branch_id),
        Query(ReviewModeQuery::default()),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read branch stream body");
    let text = String::from_utf8(body.to_vec()).expect("decode branch stream");
    assert!(text.contains("event: ci-update"));
    assert!(text.contains("\"html\":"));
    assert!(text.contains("check-pika"));
    assert!(text.contains("CI: success"));
}

#[tokio::test]
async fn completed_nightly_stream_returns_initial_snapshot_and_closes() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let repo_id = store
        .ensure_forge_repo_metadata(
            "sledtools/pika",
            "/tmp/pika.git",
            "master",
            "crates/pikaci/src/ci_catalog.rs",
        )
        .expect("ensure repo metadata");
    let lane = crate::ci_manifest::ForgeLane {
        id: "nightly_pika".to_string(),
        title: "nightly-pika".to_string(),
        entrypoint: "just checks::nightly-pika-e2e".to_string(),
        command: vec!["just".to_string(), "checks::nightly-pika-e2e".to_string()],
        paths: vec![],
        concurrency_group: None,
    };
    store
        .queue_nightly_run(
            repo_id,
            "refs/heads/master",
            "nightly-head",
            "2026-03-19T08:00:00Z",
            std::slice::from_ref(&lane),
        )
        .expect("queue nightly");
    let claimed = store
        .claim_pending_nightly_lane_runs(1, 120)
        .expect("claim nightly lane")
        .into_iter()
        .next()
        .expect("nightly lane");
    store
        .finish_nightly_lane_run(
            claimed.lane_run_id,
            claimed.claim_token,
            crate::ci_state::CiLaneStatus::Failed,
            "boom",
        )
        .expect("finish nightly lane");

    let mut config = forge_test_config_without_admins();
    config
        .forge_repo
        .as_mut()
        .expect("forge repo")
        .ci_concurrency = None;
    let nightly_run_id = claimed.nightly_run_id;
    let state = test_state(store, config);
    let response = nightly_stream_handler(State(state), Path(nightly_run_id))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read nightly stream body");
    let text = String::from_utf8(body.to_vec()).expect("decode nightly stream");
    assert!(text.contains("event: ci-update"));
    assert!(text.contains("nightly-pika"));
    assert!(text.contains("nightly: failed"));
}

#[tokio::test]
async fn branch_live_snapshot_html_updates_across_lane_transitions() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let branch = store
        .upsert_branch_record(&branch_upsert_input(
            "feature/live-progress",
            "head-progress",
        ))
        .expect("insert branch");
    store
        .queue_branch_ci_run_for_head(
            branch.branch_id,
            "head-progress",
            &[crate::ci_manifest::ForgeLane {
                id: "pika".to_string(),
                title: "check-pika".to_string(),
                entrypoint: "just checks::pre-merge-pika".to_string(),
                command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
                paths: vec![],
                concurrency_group: None,
            }],
        )
        .expect("queue ci");
    let mut config = forge_test_config_without_admins();
    config
        .forge_repo
        .as_mut()
        .expect("forge repo")
        .ci_concurrency = None;
    let state = test_state(store.clone(), config);

    let queued = load_branch_ci_live_snapshot(Arc::clone(&state), branch.branch_id)
        .await
        .expect("load queued snapshot")
        .expect("queued snapshot exists");
    assert!(queued.html.contains("CI: queued"));
    assert!(queued.html.contains("data-branch-ci-active=\"true\""));

    let claimed = store
        .claim_pending_branch_ci_lane_runs(1, 120)
        .expect("claim branch lane")
        .into_iter()
        .next()
        .expect("lane");
    let running = load_branch_ci_live_snapshot(Arc::clone(&state), branch.branch_id)
        .await
        .expect("load running snapshot")
        .expect("running snapshot exists");
    assert!(running.html.contains("running"));
    assert!(running.html.contains("data-branch-ci-active=\"true\""));

    store
        .finish_branch_ci_lane_run(
            claimed.lane_run_id,
            claimed.claim_token,
            crate::ci_state::CiLaneStatus::Success,
            "ok",
        )
        .expect("finish lane");
    let finished = load_branch_ci_live_snapshot(state, branch.branch_id)
        .await
        .expect("load finished snapshot")
        .expect("finished snapshot exists");
    assert!(finished.html.contains("CI: success"));
    assert!(finished.html.contains("data-branch-ci-active=\"false\""));
}

#[tokio::test]
async fn nightly_live_snapshot_html_updates_across_lane_transitions() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let repo_id = store
        .ensure_forge_repo_metadata(
            "sledtools/pika",
            "/tmp/pika.git",
            "master",
            "crates/pikaci/src/ci_catalog.rs",
        )
        .expect("ensure repo metadata");
    let lane = crate::ci_manifest::ForgeLane {
        id: "nightly_pika".to_string(),
        title: "nightly-pika".to_string(),
        entrypoint: "just checks::nightly-pika-e2e".to_string(),
        command: vec!["just".to_string(), "checks::nightly-pika-e2e".to_string()],
        paths: vec![],
        concurrency_group: None,
    };
    store
        .queue_nightly_run(
            repo_id,
            "refs/heads/master",
            "nightly-live-head",
            "2026-03-19T08:00:00Z",
            std::slice::from_ref(&lane),
        )
        .expect("queue nightly");
    let nightly_run_id = store
        .list_recent_nightly_runs(1)
        .expect("nightly feed")
        .into_iter()
        .next()
        .expect("nightly run")
        .nightly_run_id;
    let mut config = forge_test_config_without_admins();
    config
        .forge_repo
        .as_mut()
        .expect("forge repo")
        .ci_concurrency = None;
    let state = test_state(store.clone(), config);

    let queued = load_nightly_live_snapshot(Arc::clone(&state), nightly_run_id)
        .await
        .expect("load queued nightly")
        .expect("nightly exists");
    assert!(queued.html.contains("nightly: queued"));
    assert!(queued.html.contains("data-nightly-active=\"true\""));

    let claimed = store
        .claim_pending_nightly_lane_runs(1, 120)
        .expect("claim nightly")
        .into_iter()
        .next()
        .expect("nightly lane");
    let running = load_nightly_live_snapshot(Arc::clone(&state), nightly_run_id)
        .await
        .expect("load running nightly")
        .expect("nightly exists");
    assert!(running.html.contains("running"));
    assert!(running.html.contains("data-nightly-active=\"true\""));

    store
        .finish_nightly_lane_run(
            claimed.lane_run_id,
            claimed.claim_token,
            crate::ci_state::CiLaneStatus::Failed,
            "boom",
        )
        .expect("finish nightly");
    let finished = load_nightly_live_snapshot(state, nightly_run_id)
        .await
        .expect("load finished nightly")
        .expect("nightly exists");
    assert!(finished.html.contains("nightly: failed"));
    assert!(finished.html.contains("data-nightly-active=\"false\""));
}

#[tokio::test]
async fn branch_live_stream_recovers_with_fresh_snapshot_after_lag() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let branch = store
        .upsert_branch_record(&branch_upsert_input("feature/live-lag", "head-lag"))
        .expect("insert branch");
    store
        .queue_branch_ci_run_for_head(
            branch.branch_id,
            "head-lag",
            &[crate::ci_manifest::ForgeLane {
                id: "pika".to_string(),
                title: "check-pika".to_string(),
                entrypoint: "just checks::pre-merge-pika".to_string(),
                command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
                paths: vec![],
                concurrency_group: None,
            }],
        )
        .expect("queue ci");
    let mut config = forge_test_config_without_admins();
    config
        .forge_repo
        .as_mut()
        .expect("forge repo")
        .ci_concurrency = None;
    let state = test_state_with_live_buffer(store.clone(), config, 1);
    let mut receiver = state.live_updates.subscribe();
    let claimed = store
        .claim_pending_branch_ci_lane_runs(1, 120)
        .expect("claim branch lane")
        .into_iter()
        .next()
        .expect("claimed lane");
    store
        .finish_branch_ci_lane_run(
            claimed.lane_run_id,
            claimed.claim_token,
            crate::ci_state::CiLaneStatus::Success,
            "ok",
        )
        .expect("finish branch lane");

    state
        .live_updates
        .branch_changed(branch.branch_id, "lane_claimed");
    state
        .live_updates
        .branch_changed(branch.branch_id, "lane_finished");

    let snapshot = next_branch_ci_live_snapshot(&mut receiver, state, branch.branch_id)
        .await
        .expect("lagged snapshot");
    assert!(snapshot.html.contains("CI: success"));
    assert!(!snapshot.active);
}

#[tokio::test]
async fn nightly_live_stream_recovers_with_fresh_snapshot_after_lag() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let repo_id = store
        .ensure_forge_repo_metadata(
            "sledtools/pika",
            "/tmp/pika.git",
            "master",
            "crates/pikaci/src/ci_catalog.rs",
        )
        .expect("ensure repo metadata");
    let lane = crate::ci_manifest::ForgeLane {
        id: "nightly_pika".to_string(),
        title: "nightly-pika".to_string(),
        entrypoint: "just checks::nightly-pika-e2e".to_string(),
        command: vec!["just".to_string(), "checks::nightly-pika-e2e".to_string()],
        paths: vec![],
        concurrency_group: None,
    };
    store
        .queue_nightly_run(
            repo_id,
            "refs/heads/master",
            "nightly-lag-head",
            "2026-03-19T08:00:00Z",
            std::slice::from_ref(&lane),
        )
        .expect("queue nightly");
    let nightly_run_id = store
        .list_recent_nightly_runs(1)
        .expect("nightly feed")
        .into_iter()
        .next()
        .expect("nightly run")
        .nightly_run_id;
    let mut config = forge_test_config_without_admins();
    config
        .forge_repo
        .as_mut()
        .expect("forge repo")
        .ci_concurrency = None;
    let state = test_state_with_live_buffer(store.clone(), config, 1);
    let mut receiver = state.live_updates.subscribe();
    let claimed = store
        .claim_pending_nightly_lane_runs(1, 120)
        .expect("claim nightly")
        .into_iter()
        .next()
        .expect("claimed nightly lane");
    store
        .finish_nightly_lane_run(
            claimed.lane_run_id,
            claimed.claim_token,
            crate::ci_state::CiLaneStatus::Failed,
            "boom",
        )
        .expect("finish nightly lane");

    state
        .live_updates
        .nightly_changed(nightly_run_id, "lane_claimed");
    state
        .live_updates
        .nightly_changed(nightly_run_id, "lane_finished");

    let snapshot = next_nightly_live_snapshot(&mut receiver, state, nightly_run_id)
        .await
        .expect("lagged nightly snapshot");
    assert!(snapshot.html.contains("nightly: failed"));
    assert!(!snapshot.active);
}
