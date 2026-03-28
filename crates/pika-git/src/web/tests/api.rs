#[tokio::test]
async fn api_forge_branch_resolve_returns_open_branch() {
    let ctx = WebTestContext::new();
    let store = ctx.store();
    let branch = store
        .upsert_branch_record(&branch_upsert_input("feature/api-resolve", "head-resolve"))
        .expect("insert branch");
    let mut config = forge_test_config_without_admins();
    config
        .forge_repo
        .as_mut()
        .expect("forge repo")
        .ci_concurrency = None;
    let headers = ctx.trusted_headers(TRUSTED_NPUB);
    let state = ctx.state(config);

    let response = api_forge_branch_resolve_handler(
        State(state),
        Query(ForgeBranchResolveQuery {
            branch_name: "feature/api-resolve".to_string(),
        }),
        headers,
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
    assert_eq!(json["branch_id"], branch.branch_id);
    assert_eq!(json["branch_name"], "feature/api-resolve");
    assert_eq!(json["branch_state"], "open");
}

#[tokio::test]
async fn api_forge_branch_resolve_returns_closed_branch_history() {
    let ctx = WebTestContext::new();
    let store = ctx.store();
    let branch = store
        .upsert_branch_record(&branch_upsert_input("feature/api-history", "head-history"))
        .expect("insert branch");
    store
        .mark_branch_closed(branch.branch_id, TRUSTED_NPUB)
        .expect("close branch");
    let mut config = forge_test_config_without_admins();
    config
        .forge_repo
        .as_mut()
        .expect("forge repo")
        .ci_concurrency = None;
    let headers = ctx.trusted_headers(TRUSTED_NPUB);
    let state = ctx.state(config);

    let response = api_forge_branch_resolve_handler(
        State(state),
        Query(ForgeBranchResolveQuery {
            branch_name: "feature/api-history".to_string(),
        }),
        headers,
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
    assert_eq!(json["branch_id"], branch.branch_id);
    assert_eq!(json["branch_name"], "feature/api-history");
    assert_eq!(json["branch_state"], "closed");
}

#[tokio::test]
async fn auth_challenge_handler_allows_forge_only_auth_mode() {
    let ctx = WebTestContext::new();
    let store = ctx.store();
    store
        .upsert_chat_allowlist_entry(
            TRUSTED_NPUB,
            false,
            true,
            Some("forge-only"),
            "npub1admin",
        )
        .expect("upsert forge-only allowlist entry");
    let mut config = forge_test_config_without_admins();
    config
        .forge_repo
        .as_mut()
        .expect("forge repo")
        .ci_concurrency = None;
    let state = ctx.state(config);

    let response = auth_challenge_handler(State(state)).await.into_response();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn api_forge_branch_detail_returns_ci_summary() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let branch = store
        .upsert_branch_record(&branch_upsert_input("feature/api-detail", "head-detail"))
        .expect("insert branch");
    store
        .queue_branch_ci_run_for_head(
            branch.branch_id,
            "head-detail",
            &[crate::ci_manifest::ForgeLane {
                id: "pre-merge-pika-rust".to_string(),
                title: "check-pika".to_string(),
                entrypoint: "just checks::pre-merge-pika".to_string(),
                command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
                paths: vec![],
                concurrency_group: None,
            }],
        )
        .expect("queue ci");
    let claimed = store
        .claim_pending_branch_ci_lane_runs(1, 120)
        .expect("claim lane")
        .into_iter()
        .next()
        .expect("claimed lane");
    store
        .record_branch_ci_lane_ci_run(
            claimed.lane_run_id,
            claimed.claim_token,
            "pikaci-api-detail",
            Some("pre-merge-pika-rust"),
        )
        .expect("record CI metadata");
    let mut config = forge_test_config_without_admins();
    config
        .forge_repo
        .as_mut()
        .expect("forge repo")
        .ci_concurrency = None;
    let headers = trusted_headers(&store, TRUSTED_NPUB);
    let state = test_state(store, config);

    let response =
        api_forge_branch_detail_handler(State(state), Path(branch.branch_id), headers)
            .await
            .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
    assert_eq!(json["branch"]["branch_name"], "feature/api-detail");
    assert_eq!(
        json["ci_runs"][0]["lanes"][0]["ci_run_id"],
        "pikaci-api-detail"
    );
    assert_eq!(
        json["ci_runs"][0]["lanes"][0]["ci_target_id"],
        "pre-merge-pika-rust"
    );
    assert_eq!(
        json["ci_runs"][0]["lanes"][0]["execution_reason"],
        "running"
    );
    assert_eq!(
        json["ci_runs"][0]["lanes"][0]["ci_target_key"],
        "pre-merge-pika-rust"
    );
}

#[tokio::test]
async fn api_forge_branch_detail_exposes_waiting_lane_state() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let branch = store
        .upsert_branch_record(&branch_upsert_input("feature/api-waiting", "head-waiting"))
        .expect("insert branch");
    store
        .queue_branch_ci_run_for_head(
            branch.branch_id,
            "head-waiting",
            &[
                crate::ci_manifest::ForgeLane {
                    id: "wait-capacity".to_string(),
                    title: "wait-capacity".to_string(),
                    entrypoint: "just checks::wait-capacity".to_string(),
                    command: vec!["just".to_string(), "checks::wait-capacity".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                },
                crate::ci_manifest::ForgeLane {
                    id: "apple-sanity".to_string(),
                    title: "apple-sanity".to_string(),
                    entrypoint: "just checks::apple-sanity".to_string(),
                    command: vec!["just".to_string(), "checks::apple-sanity".to_string()],
                    paths: vec![],
                    concurrency_group: Some("apple-host".to_string()),
                },
            ],
        )
        .expect("queue ci");
    store
        .with_connection(|conn| {
            conn.execute(
                "UPDATE branch_ci_run_lanes
                 SET execution_reason = 'waiting_for_capacity'
                 WHERE lane_id = 'wait-capacity'",
                [],
            )?;
            Ok::<(), anyhow::Error>(())
        })
        .expect("set waiting state");
    let mut config = forge_test_config_without_admins();
    config
        .forge_repo
        .as_mut()
        .expect("forge repo")
        .ci_concurrency = Some(1);
    let headers = trusted_headers(&store, TRUSTED_NPUB);
    let state = test_state(store, config);

    let response =
        api_forge_branch_detail_handler(State(state), Path(branch.branch_id), headers)
            .await
            .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
    assert_eq!(
        json["ci_runs"][0]["lanes"][0]["execution_reason"],
        "waiting_for_capacity"
    );
    assert_eq!(
        json["ci_runs"][0]["lanes"][1]["execution_reason"],
        "queued"
    );
}

#[tokio::test]
async fn api_forge_branch_logs_defaults_to_latest_failed_lane() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let branch = store
        .upsert_branch_record(&branch_upsert_input("feature/api-logs", "head-logs"))
        .expect("insert branch");
    store
        .queue_branch_ci_run_for_head(
            branch.branch_id,
            "head-logs",
            &[
                crate::ci_manifest::ForgeLane {
                    id: "pika".to_string(),
                    title: "check-pika".to_string(),
                    entrypoint: "just checks::pre-merge-pika".to_string(),
                    command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                },
                crate::ci_manifest::ForgeLane {
                    id: "fixture".to_string(),
                    title: "check-fixture".to_string(),
                    entrypoint: "just checks::pre-merge-fixture".to_string(),
                    command: vec!["just".to_string(), "checks::pre-merge-fixture".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                },
            ],
        )
        .expect("queue ci");
    let claimed = store
        .claim_pending_branch_ci_lane_runs(2, 120)
        .expect("claim lanes");
    let success_lane = claimed
        .iter()
        .find(|lane| lane.lane_id == "pika")
        .expect("success lane");
    store
        .finish_branch_ci_lane_run(
            success_lane.lane_run_id,
            success_lane.claim_token,
            crate::ci_state::CiLaneStatus::Success,
            "ok",
        )
        .expect("finish success lane");
    let failed_lane = claimed
        .iter()
        .find(|lane| lane.lane_id == "fixture")
        .expect("failed lane");
    store
        .finish_branch_ci_lane_run(
            failed_lane.lane_run_id,
            failed_lane.claim_token,
            crate::ci_state::CiLaneStatus::Failed,
            "fixture boom",
        )
        .expect("finish failed lane");
    let mut config = forge_test_config_without_admins();
    config
        .forge_repo
        .as_mut()
        .expect("forge repo")
        .ci_concurrency = None;
    let headers = trusted_headers(&store, TRUSTED_NPUB);
    let state = test_state(store, config);

    let response = api_forge_branch_logs_handler(
        State(state),
        Path(branch.branch_id),
        Query(ForgeBranchLogsQuery {
            lane: None,
            lane_run_id: None,
        }),
        headers,
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
    assert_eq!(json["lane"]["lane_id"], "fixture");
    assert_eq!(json["lane"]["status"], "failed");
    assert_eq!(json["lane"]["log_text"], "fixture boom");
}

#[tokio::test]
async fn api_forge_branch_logs_includes_persisted_ci_run_metadata() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let branch = store
        .upsert_branch_record(&branch_upsert_input("feature/api-pikaci", "head-pikaci"))
        .expect("insert branch");
    store
        .queue_branch_ci_run_for_head(
            branch.branch_id,
            "head-pikaci",
            &[crate::ci_manifest::ForgeLane {
                id: "pika-rust".to_string(),
                title: "check-pika-rust".to_string(),
                entrypoint: "./scripts/pikaci-staged-linux-remote.sh run pre-merge-pika-rust"
                    .to_string(),
                command: vec![
                    "./scripts/pikaci-staged-linux-remote.sh".to_string(),
                    "run".to_string(),
                    "pre-merge-pika-rust".to_string(),
                ],
                paths: vec![],
                concurrency_group: None,
            }],
        )
        .expect("queue ci");
    let job = store
        .claim_pending_branch_ci_lane_runs(1, 120)
        .expect("claim lane")
        .into_iter()
        .next()
        .expect("lane");
    store
        .record_branch_ci_lane_ci_run(
            job.lane_run_id,
            job.claim_token,
            "pikaci-run-123",
            Some("pre-merge-pika-rust"),
        )
        .expect("record CI run");
    store
        .finish_branch_ci_lane_run(
            job.lane_run_id,
            job.claim_token,
            crate::ci_state::CiLaneStatus::Success,
            "ok",
        )
        .expect("finish lane");

    let config = forge_test_config_with_git_dir(&dir.path().join("pika.git"));
    write_pikaci_run_fixture(&config, "pikaci-run-123");
    let headers = trusted_headers(&store, TRUSTED_NPUB);
    let state = test_state(store, config);

    let response = api_forge_branch_logs_handler(
        State(state),
        Path(branch.branch_id),
        Query(ForgeBranchLogsQuery {
            lane: Some("pika-rust".to_string()),
            lane_run_id: None,
        }),
        headers,
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
    assert_eq!(json["ci_run"]["run_id"], "pikaci-run-123");
    assert_eq!(json["ci_log_metadata"]["jobs"][0]["id"], "job-one");
    assert_eq!(
        json["ci_log_metadata"]["jobs"][0]["host_log_exists"],
        true
    );
    assert_eq!(
        json["ci_prepared_outputs"]["outputs"][0]["output_name"],
        "ci.x86_64-linux.workspaceBuild"
    );
}

#[tokio::test]
async fn api_forge_branch_logs_keeps_run_metadata_when_prepared_outputs_are_invalid() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let branch = store
        .upsert_branch_record(&branch_upsert_input(
            "feature/api-pikaci-invalid-prepared",
            "head-pikaci-invalid-prepared",
        ))
        .expect("insert branch");
    store
        .queue_branch_ci_run_for_head(
            branch.branch_id,
            "head-pikaci-invalid-prepared",
            &[crate::ci_manifest::ForgeLane {
                id: "pika-rust".to_string(),
                title: "check-pika-rust".to_string(),
                entrypoint: "./scripts/pikaci-staged-linux-remote.sh run pre-merge-pika-rust"
                    .to_string(),
                command: vec![
                    "./scripts/pikaci-staged-linux-remote.sh".to_string(),
                    "run".to_string(),
                    "pre-merge-pika-rust".to_string(),
                ],
                paths: vec![],
                concurrency_group: None,
            }],
        )
        .expect("queue ci");
    let job = store
        .claim_pending_branch_ci_lane_runs(1, 120)
        .expect("claim lane")
        .into_iter()
        .next()
        .expect("lane");
    store
        .record_branch_ci_lane_ci_run(
            job.lane_run_id,
            job.claim_token,
            "pikaci-run-invalid-prepared",
            Some("pre-merge-pika-rust"),
        )
        .expect("record CI run");
    store
        .finish_branch_ci_lane_run(
            job.lane_run_id,
            job.claim_token,
            crate::ci_state::CiLaneStatus::Success,
            "ok",
        )
        .expect("finish lane");

    let config = forge_test_config_with_git_dir(&dir.path().join("pika.git"));
    write_pikaci_run_fixture(&config, "pikaci-run-invalid-prepared");
    let prepared_outputs_path = JerichociRunStore::from_config(&config)
        .expect("CI run store")
        .prepared_outputs_path("pikaci-run-invalid-prepared");
    fs::write(&prepared_outputs_path, "{not valid json").expect("corrupt prepared outputs");
    let headers = trusted_headers(&store, TRUSTED_NPUB);
    let state = test_state(store, config);

    let response = api_forge_branch_logs_handler(
        State(state),
        Path(branch.branch_id),
        Query(ForgeBranchLogsQuery {
            lane: Some("pika-rust".to_string()),
            lane_run_id: None,
        }),
        headers,
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("parse json");
    assert_eq!(json["ci_run"]["run_id"], "pikaci-run-invalid-prepared");
    assert_eq!(json["ci_log_metadata"]["jobs"][0]["id"], "job-one");
    assert!(json["ci_prepared_outputs"].is_null());
}

#[tokio::test]
async fn api_forge_ci_handlers_load_persisted_run_and_logs() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let config = forge_test_config_with_git_dir(&dir.path().join("pika.git"));
    write_pikaci_run_fixture(&config, "pikaci-run-abc");
    let headers = trusted_headers(&store, TRUSTED_NPUB);
    let state = test_state(store, config);

    let run_response = api_forge_ci_run_handler(
        State(Arc::clone(&state)),
        Path("pikaci-run-abc".to_string()),
        headers.clone(),
    )
    .await
    .into_response();
    assert_eq!(run_response.status(), StatusCode::OK);
    let run_body = to_bytes(run_response.into_body(), usize::MAX)
        .await
        .expect("read run body");
    let run_json: serde_json::Value = serde_json::from_slice(&run_body).expect("parse run");
    assert_eq!(run_json["run_id"], "pikaci-run-abc");
    assert_eq!(
        run_json["jobs"][0]["remote_linux_vm_execution"]["incus_image"]["alias"],
        "jericho/dev"
    );

    let logs_response = api_forge_ci_logs_handler(
        State(state),
        Path("pikaci-run-abc".to_string()),
        Query(ForgeCiLogsQuery {
            job: Some("job-one".to_string()),
            kind: super::ForgeCiLogKind::Both,
        }),
        headers,
    )
    .await
    .into_response();
    assert_eq!(logs_response.status(), StatusCode::OK);
    let logs_body = to_bytes(logs_response.into_body(), usize::MAX)
        .await
        .expect("read logs body");
    let logs_json: serde_json::Value = serde_json::from_slice(&logs_body).expect("parse logs");
    assert_eq!(logs_json["run_id"], "pikaci-run-abc");
    assert_eq!(logs_json["job"], "job-one");
    assert_eq!(logs_json["host"], "host fixture\n");
    assert_eq!(logs_json["guest"], "guest fixture\n");
}

#[tokio::test]
async fn api_forge_ci_prepared_outputs_handler_loads_persisted_record() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let config = forge_test_config_with_git_dir(&dir.path().join("pika.git"));
    write_pikaci_run_fixture(&config, "pikaci-run-prepared");
    let headers = trusted_headers(&store, TRUSTED_NPUB);
    let state = test_state(store, config);

    let response = api_forge_ci_prepared_outputs_handler(
        State(state),
        Path("pikaci-run-prepared".to_string()),
        headers,
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    let json: serde_json::Value =
        serde_json::from_slice(&body).expect("parse prepared outputs");
    assert_eq!(json["run_id"], "pikaci-run-prepared");
    assert_eq!(
        json["prepared_outputs"]["outputs"][0]["realized_path"],
        "/nix/store/workspace-build"
    );
}

#[tokio::test]
async fn rerun_branch_handler_rejects_lane_from_another_branch() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let first = store
        .upsert_branch_record(&branch_upsert_input("feature/one", "head-1"))
        .expect("insert first branch");
    let second = store
        .upsert_branch_record(&branch_upsert_input("feature/two", "head-2"))
        .expect("insert second branch");
    store
        .queue_branch_ci_run_for_head(
            first.branch_id,
            "head-1",
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
    let job = store
        .claim_pending_branch_ci_lane_runs(1, 120)
        .expect("claim branch job")
        .into_iter()
        .next()
        .expect("job");
    store
        .finish_branch_ci_lane_run(
            job.lane_run_id,
            job.claim_token,
            crate::ci_state::CiLaneStatus::Failed,
            "boom",
        )
        .expect("finish lane");

    let config = forge_test_config();
    let headers = trusted_headers(&store, TRUSTED_NPUB);
    let state = test_state(store, config);
    let response = rerun_branch_ci_lane_handler(
        State(state),
        Path((second.branch_id, job.lane_run_id)),
        headers,
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rerun_nightly_handler_rejects_lane_from_another_run() {
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
            "head-a",
            "2026-03-17T08:00:00Z",
            std::slice::from_ref(&lane),
        )
        .expect("queue first nightly");
    store
        .queue_nightly_run(
            repo_id,
            "refs/heads/master",
            "head-b",
            "2026-03-18T08:00:00Z",
            std::slice::from_ref(&lane),
        )
        .expect("queue second nightly");
    let job = store
        .claim_pending_nightly_lane_runs(1, 120)
        .expect("claim nightly job")
        .into_iter()
        .next()
        .expect("job");
    store
        .finish_nightly_lane_run(
            job.lane_run_id,
            job.claim_token,
            crate::ci_state::CiLaneStatus::Failed,
            "boom",
        )
        .expect("finish lane");
    let wrong_nightly = store
        .list_recent_nightly_runs(8)
        .expect("list nightly runs")
        .into_iter()
        .find(|run| run.nightly_run_id != job.nightly_run_id)
        .expect("other nightly");

    let config = forge_test_config();
    let headers = trusted_headers(&store, TRUSTED_NPUB);
    let state = test_state(store, config);
    let response = rerun_nightly_lane_handler(
        State(state),
        Path((wrong_nightly.nightly_run_id, job.lane_run_id)),
        headers,
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn fail_branch_handler_requires_trusted_access() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let branch = store
        .upsert_branch_record(&branch_upsert_input("feature/forbidden", "head-1"))
        .expect("insert branch");
    store
        .queue_branch_ci_run_for_head(
            branch.branch_id,
            "head-1",
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
        .list_branch_ci_runs(branch.branch_id, 1)
        .expect("list branch runs")[0]
        .lanes[0]
        .id;
    let mut headers = HeaderMap::new();
    store
        .insert_auth_token("reader-token", "npub1reader")
        .expect("insert reader token");
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer reader-token"),
    );
    let state = test_state(store, forge_test_config());

    let response =
        fail_branch_ci_lane_handler(State(state), Path((branch.branch_id, lane)), headers)
            .await
            .into_response();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn recover_branch_handler_rejects_run_from_another_branch() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let first = store
        .upsert_branch_record(&branch_upsert_input("feature/one", "head-1"))
        .expect("insert first branch");
    let second = store
        .upsert_branch_record(&branch_upsert_input("feature/two", "head-2"))
        .expect("insert second branch");
    let lane = crate::ci_manifest::ForgeLane {
        id: "pika".to_string(),
        title: "check-pika".to_string(),
        entrypoint: "just checks::pre-merge-pika".to_string(),
        command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
        paths: vec![],
        concurrency_group: None,
    };
    store
        .queue_branch_ci_run_for_head(first.branch_id, "head-1", std::slice::from_ref(&lane))
        .expect("queue first branch");
    store
        .queue_branch_ci_run_for_head(second.branch_id, "head-2", std::slice::from_ref(&lane))
        .expect("queue second branch");
    let wrong_run_id = store
        .list_branch_ci_runs(first.branch_id, 1)
        .expect("first runs")[0]
        .id;

    let headers = trusted_headers(&store, TRUSTED_NPUB);
    let state = test_state(store, forge_test_config());
    let response = recover_branch_ci_run_handler(
        State(state),
        Path((second.branch_id, wrong_run_id)),
        headers,
    )
    .await
    .into_response();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn fail_nightly_handler_rejects_lane_from_another_run() {
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
            "head-a",
            "2026-03-17T08:00:00Z",
            std::slice::from_ref(&lane),
        )
        .expect("queue first nightly");
    store
        .queue_nightly_run(
            repo_id,
            "refs/heads/master",
            "head-b",
            "2026-03-18T08:00:00Z",
            std::slice::from_ref(&lane),
        )
        .expect("queue second nightly");
    let wrong_nightly = store
        .list_recent_nightly_runs(8)
        .expect("list nightly runs")
        .into_iter()
        .max_by_key(|run| run.nightly_run_id)
        .expect("other nightly");
    let lane_run_id = store
        .get_nightly_run(wrong_nightly.nightly_run_id)
        .expect("nightly detail")
        .expect("nightly run")
        .lanes[0]
        .id;
    let other_nightly_id = store
        .list_recent_nightly_runs(8)
        .expect("list nightly runs")
        .into_iter()
        .find(|run| run.nightly_run_id != wrong_nightly.nightly_run_id)
        .expect("mismatched nightly")
        .nightly_run_id;

    let headers = trusted_headers(&store, TRUSTED_NPUB);
    let state = test_state(store, forge_test_config());
    let response =
        fail_nightly_lane_handler(State(state), Path((other_nightly_id, lane_run_id)), headers)
            .await
            .into_response();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn wake_ci_handler_requires_trusted_access() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("pika-git.db");
    let store = Store::open(&db_path).expect("open store");
    let state = test_state(store.clone(), forge_test_config());
    store
        .insert_auth_token("reader-token", "npub1reader")
        .expect("insert reader token");
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer reader-token"),
    );

    let response = wake_ci_handler(State(state), headers).await.into_response();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
