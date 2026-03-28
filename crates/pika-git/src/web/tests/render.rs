    #[test]
    fn merged_branch_page_renders_after_source_branch_deletion() {
        let _manifest_override = crate::ci::install_test_manifest_override(
            crate::ci_manifest::ForgeCiManifest {
                nightly_hour_utc: 23,
                nightly_minute_utc: 59,
                branch_lanes: vec![crate::ci_manifest::ForgeLane {
                    id: "render_history".to_string(),
                    title: "render history".to_string(),
                    entrypoint: "./ci.sh".to_string(),
                    command: vec!["./ci.sh".to_string()],
                    paths: vec![
                        "README.md".to_string(),
                        "feature.txt".to_string(),
                        "crates/pikaci/src/ci_catalog.rs".to_string(),
                    ],
                    concurrency_group: None,
                }],
                nightly_lanes: vec![],
            },
        );
        let repo = GitTestRepo::new();
        repo.write_seed("README.md", "hello\n");
        repo.write_seed(
            "ci.sh",
            "#!/usr/bin/env bash\nset -euo pipefail\necho branch-ci-ok\n",
        );
        repo.write_seed(
            "crates/pikaci/src/ci_catalog.rs",
            r#"
version = 1
nightly_schedule_utc = "23:59"

[[branch.lanes]]
id = "render_history"
title = "render history"
entrypoint = "./ci.sh"
command = ["./ci.sh"]
paths = ["README.md", "feature.txt", "crates/pikaci/src/ci_catalog.rs"]
"#,
        );
        repo.chmod_seed_executable("ci.sh");
        repo.seed_add(&["README.md", "ci.sh", "crates/pikaci/src/ci_catalog.rs"]);
        repo.seed_commit("initial");
        repo.seed_push_master();
        repo.seed_checkout_new_branch("feature/render-history");
        repo.write_seed("feature.txt", "branch work\n");
        repo.seed_add(&["feature.txt"]);
        repo.seed_commit("branch render history");
        repo.seed_push_branch("feature/render-history");

        let store = repo.open_store();
        let config = Config::test_with_forge_repo(ForgeRepoConfig {
            repo: "sledtools/pika".to_string(),
            canonical_git_dir: repo.bare_path().to_str().expect("bare path").to_string(),
            default_branch: "master".to_string(),
            ci_concurrency: Some(2),
            mirror_remote: None,
            mirror_poll_interval_secs: None,
            mirror_timeout_secs: None,
            ci_command: vec!["./ci.sh".to_string()],
            hook_url: Some("http://127.0.0.1:9999/git/webhook".to_string()),
        });

        poller::poll_once_limited(&store, &config, 0).expect("sync branch from bare repo");
        let branch = store
            .list_branch_feed_items()
            .expect("feed items")
            .into_iter()
            .find(|item| item.branch_name == "feature/render-history")
            .expect("branch item");
        let ci_pass = ci::run_ci_pass(&store, &config).expect("run ci pass");
        assert_eq!(ci_pass.succeeded, 1);

        let artifact_id: i64 = store
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
                r#"{"executive_summary":"ok","media_links":[],"steps":[{"title":"Step","intent":"Intent","affected_files":["feature.txt"],"evidence_snippets":["@@ -0,0 +1 @@"],"body_markdown":"body"}]}"#,
                "<p>ok</p>",
                &branch.head_sha,
                "@@ -0,0 +1 @@",
                None,
            )
            .expect("mark artifact ready");

        let forge_repo = config.effective_forge_repo().expect("forge repo");
        let branch_target = store
            .get_branch_action_target(branch.branch_id)
            .expect("branch target")
            .expect("existing branch target");
        let merge = forge::merge_branch(
            &forge_repo,
            &branch_target.branch_name,
            &branch_target.head_sha,
        )
        .expect("merge branch");
        store
            .mark_branch_merged(branch.branch_id, "npub1trusted", &merge.merge_commit_sha)
            .expect("mark merged");
        assert!(
            forge::current_branch_head(&forge_repo, &branch_target.branch_name)
                .expect("resolve branch")
                .is_none()
        );

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail exists");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 4)
            .expect("ci runs");
        let detail_rendered = render_detail_template(detail.clone(), ci_runs.clone(), false)
            .expect("render detail template")
            .render()
            .expect("render detail html");
        assert!(detail_rendered.contains("feature/render-history"));
        assert!(detail_rendered.contains("Open CI Details"));
        assert!(!detail_rendered.contains("branch-ci-ok"));
        assert!(detail_rendered.contains("branch: merged"));
        assert!(!detail_rendered.contains("Merge Into master"));

        let ci_rendered =
            render_branch_ci_template_with_notices(detail, ci_runs, Vec::new(), false)
                .expect("render branch ci template")
                .render()
                .expect("render branch ci html");
        assert!(ci_rendered.contains("branch-ci-ok"));
        assert!(ci_rendered.contains("Run History"));
    }

    #[test]
    fn branch_detail_renders_manual_rerun_provenance() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/rerun-ui", "head-rerun"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-rerun",
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
        let failed = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim ci")
            .into_iter()
            .next()
            .expect("ci lane");
        store
            .finish_branch_ci_lane_run(
                failed.lane_run_id,
                failed.claim_token,
                crate::ci_state::CiLaneStatus::Failed,
                "boom",
            )
            .expect("finish ci");
        store
            .rerun_branch_ci_lane(branch.branch_id, failed.lane_run_id)
            .expect("rerun ci")
            .expect("rerun suite");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");
        let detail_rendered = render_detail_template(detail.clone(), ci_runs.clone(), false)
            .expect("render detail template")
            .render()
            .expect("render detail html");
        assert!(detail_rendered.contains("manual rerun of run #"));
        assert!(!detail_rendered.contains("manual rerun of lane #"));

        let ci_rendered =
            render_branch_ci_template_with_notices(detail, ci_runs, Vec::new(), false)
                .expect("render branch ci template")
                .render()
                .expect("render branch ci html");
        assert!(ci_rendered.contains("manual rerun of lane #"));
    }

    #[test]
    fn branch_detail_renders_pikaci_run_metadata() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/pikaci-ui", "head-pikaci"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-pikaci",
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
        let running = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim ci")
            .into_iter()
            .next()
            .expect("ci lane");
        store
            .record_branch_ci_lane_pikaci_run(
                running.lane_run_id,
                running.claim_token,
                "pikaci-run-branch-ui",
                Some("pre-merge-pika-rust"),
            )
            .expect("persist branch pikaci metadata");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");
        let detail_rendered = render_detail_template(detail.clone(), ci_runs.clone(), false)
            .expect("render detail template")
            .render()
            .expect("render detail html");
        assert!(!detail_rendered.contains("CI run"));

        let ci_rendered =
            render_branch_ci_template_with_notices(detail, ci_runs, Vec::new(), false)
                .expect("render branch ci template")
                .render()
                .expect("render branch ci html");
        assert!(ci_rendered.contains("CI run"));
        assert!(ci_rendered.contains("pikaci-run-branch-ui"));
        assert!(ci_rendered.contains("pre-merge-pika-rust"));
    }

    #[test]
    fn branch_ci_page_renders_waiting_lane_state() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/state-ui", "head-state-ui"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-state-ui",
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

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");
        let rendered = render_branch_ci_template_with_notices(detail, ci_runs, Vec::new(), false)
            .expect("render branch ci template")
            .render()
            .expect("render branch ci html");
        assert!(rendered.contains("waiting for scheduler capacity"));
    }

    #[test]
    fn branch_ci_rendering_shows_queued_duration_for_terminal_lane_without_start_time() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input(
                "feature/queued-terminal",
                "head-queued",
            ))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-queued",
                &[crate::ci_manifest::ForgeLane {
                    id: "pika".to_string(),
                    title: "pre-merge-pika".to_string(),
                    entrypoint: "just checks::pre-merge-pika".to_string(),
                    command: vec!["just".to_string(), "checks::pre-merge-pika".to_string()],
                    paths: vec![],
                    concurrency_group: None,
                }],
            )
            .expect("queue ci");
        store
            .with_connection(|conn| {
                conn.execute(
                    "UPDATE branch_ci_runs
                     SET status = 'failed',
                         created_at = '2026-03-24T12:00:00Z',
                         started_at = NULL,
                         finished_at = '2026-03-24T12:00:14Z'
                     WHERE branch_id = ?1",
                    rusqlite::params![branch.branch_id],
                )?;
                conn.execute(
                    "UPDATE branch_ci_run_lanes
                     SET status = 'failed',
                         created_at = '2026-03-24T12:00:00Z',
                         started_at = NULL,
                         finished_at = '2026-03-24T12:00:14Z',
                         log_text = 'boom'
                     WHERE lane_id = 'pika'",
                    [],
                )?;
                Ok::<(), anyhow::Error>(())
            })
            .expect("mark queued lane terminal");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");
        let now = Utc
            .with_ymd_and_hms(2026, 3, 24, 12, 0, 45)
            .single()
            .expect("valid timestamp");

        let summary_html =
            super::render_branch_ci_summary_html_at(&detail, &ci_runs, &[], false, now)
                .expect("render branch ci summary html");
        let live_html = super::render_branch_ci_live_html_at(&detail, &ci_runs, &[], now)
            .expect("render branch ci live html");

        assert!(summary_html.contains("queued 14s"));
        assert!(live_html.contains("queued 14s"));
        assert!(!live_html.contains("queued 14s · ran"));
    }

    #[test]
    fn review_mode_ci_links_preserve_inbox_context() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/review-ci", "head-review-ci"))
            .expect("insert branch");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");

        let detail_rendered = render_detail_template(detail.clone(), ci_runs.clone(), true)
            .expect("render detail template")
            .render()
            .expect("render detail html");
        assert!(
            detail_rendered.contains(&format!("/git/branch/{}/ci?review=true", branch.branch_id))
        );

        let ci_rendered = render_branch_ci_template_with_notices(detail, ci_runs, Vec::new(), true)
            .expect("render branch ci template")
            .render()
            .expect("render branch ci html");
        assert!(ci_rendered.contains(&format!("href=\"/git/inbox/review/{}\"", branch.branch_id)));
    }

    #[test]
    fn review_mode_query_accepts_numeric_and_text_bools() {
        let uri_numeric: axum::http::Uri =
            "/git/branch/7/ci?review=1".parse().expect("numeric uri");
        let numeric =
            Query::<ReviewModeQuery>::try_from_uri(&uri_numeric).expect("numeric review query");
        assert!(numeric.0.review);

        let uri_text: axum::http::Uri = "/git/branch/7/ci?review=true".parse().expect("text uri");
        let text = Query::<ReviewModeQuery>::try_from_uri(&uri_text).expect("text review query");
        assert!(text.0.review);

        let uri_missing: axum::http::Uri = "/git/branch/7/ci".parse().expect("missing uri");
        let missing =
            Query::<ReviewModeQuery>::try_from_uri(&uri_missing).expect("missing review query");
        assert!(!missing.0.review);
    }

    #[test]
    fn branch_detail_distinguishes_global_generator_health_from_branch_failure() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input(
                "feature/tutorial-failure",
                "head-tutorial",
            ))
            .expect("insert branch");
        let artifact_id: i64 = store
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
            .mark_branch_generation_failed(artifact_id, "model output malformed", false, 0)
            .expect("mark tutorial failed");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");
        let rendered = render_detail_template_with_notices(
            detail,
            ci_runs,
            false,
            vec![PageNoticeView {
                tone: "warning".to_string(),
                message: "Forge health warning: the tutorial generator worker is unhealthy. New tutorials on any branch may be delayed until it recovers; this forge-wide warning does not mean this branch's last tutorial generation attempt failed.".to_string(),
            }],
        )
        .expect("render detail template")
        .render()
        .expect("render detail html");

        assert!(
            rendered.contains("Forge health warning: the tutorial generator worker is unhealthy.")
        );
        assert!(rendered.contains("forge-wide warning does not mean"));
        assert!(rendered.contains("Branch-Specific Tutorial Generation Failed"));
        assert!(rendered.contains("This failure is specific to the current branch head."));
        assert!(rendered.contains(
            "This branch tutorial is unavailable because generation for this branch head failed."
        ));
    }

    #[test]
    fn branch_detail_places_diff_after_review_layout() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/full-width-diff", "head-diff"))
            .expect("insert branch");
        let artifact_id: i64 = store
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
                r#"{"executive_summary":"summary","steps":[],"media_links":[]}"#,
                "<p>ok</p>",
                "head-diff",
                "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n",
                Some("branch-sid-ready"),
            )
            .expect("mark tutorial ready");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");
        let rendered = render_detail_template(detail, ci_runs, false)
            .expect("render detail template")
            .render()
            .expect("render detail html");

        let review_layout = rendered
            .find("class=\"review-layout\"")
            .expect("review layout");
        let diff_row = rendered
            .find("class=\"panel diff-panel diff-row\"")
            .expect("diff row");
        assert!(diff_row > review_layout);
    }

    #[test]
    fn branch_detail_renders_branch_discussion_sidebar_when_artifact_ready() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input(
                "feature/discussion-sidebar",
                "head-chat",
            ))
            .expect("insert branch");
        let artifact_id: i64 = store
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
                r#"{"executive_summary":"summary","steps":[],"media_links":[]}"#,
                "<p>ok</p>",
                "head-chat",
                "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n",
                Some("branch-chat-session"),
            )
            .expect("mark tutorial ready");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");
        let rendered = render_detail_template(detail, ci_runs, false)
            .expect("render detail template")
            .render()
            .expect("render detail html");

        assert!(rendered.contains("Branch Review"));
        assert!(rendered.contains("Discussion"));
        assert!(rendered.contains("artifact ready"));
        assert!(rendered.contains("Ask about this branch review"));
        assert!(rendered.contains("id=\"discussion-toggle\""));
        assert!(rendered.contains("id=\"discussion-sidebar\""));
        assert!(rendered.contains("id=\"branch-page\""));
        assert!(rendered.contains(&format!("const branchChatArtifactId = {};", artifact_id)));
    }

    #[tokio::test]
    async fn branch_chat_history_uses_requested_artifact_not_current_branch_state() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/chat-stable", "head-chat-1"))
            .expect("insert branch");
        let first_artifact_id: i64 = store
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
            .expect("first artifact id");
        store
            .mark_branch_generation_ready(
                first_artifact_id,
                r#"{"executive_summary":"first","steps":[],"media_links":[]}"#,
                "<p>first</p>",
                "head-chat-1",
                "diff-1",
                Some("branch-chat-session-1"),
            )
            .expect("mark first tutorial ready");
        let (first_session_id, _) = store
            .get_or_create_branch_review_chat_session(
                first_artifact_id,
                TRUSTED_NPUB,
                "branch-chat-session-1",
            )
            .expect("first chat session");
        store
            .append_branch_review_chat_message(first_session_id, "assistant", "artifact one")
            .expect("append first message");

        store
            .upsert_branch_record(&branch_upsert_input("feature/chat-stable", "head-chat-2"))
            .expect("upsert second head");
        let second_artifact_id: i64 = store
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
            .expect("second artifact id");
        store
            .mark_branch_generation_ready(
                second_artifact_id,
                r#"{"executive_summary":"second","steps":[],"media_links":[]}"#,
                "<p>second</p>",
                "head-chat-2",
                "diff-2",
                Some("branch-chat-session-2"),
            )
            .expect("mark second tutorial ready");
        let (second_session_id, _) = store
            .get_or_create_branch_review_chat_session(
                second_artifact_id,
                TRUSTED_NPUB,
                "branch-chat-session-2",
            )
            .expect("second chat session");
        store
            .append_branch_review_chat_message(second_session_id, "assistant", "artifact two")
            .expect("append second message");

        let state = test_state(store.clone(), forge_test_config());
        let headers = trusted_headers(&store, TRUSTED_NPUB);
        let response = super::branch_chat_history_handler(
            State(state),
            Path(branch.branch_id),
            Query(super::BranchChatArtifactQuery {
                artifact_id: first_artifact_id,
            }),
            headers,
        )
        .await
        .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let text = String::from_utf8(body.to_vec()).expect("body text");
        assert!(text.contains("artifact one"));
        assert!(!text.contains("artifact two"));
    }

    #[test]
    fn branch_detail_renders_skipped_lane_badges_in_summary_and_body() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");
        let branch = store
            .upsert_branch_record(&branch_upsert_input("feature/skipped-ui", "head-skipped"))
            .expect("insert branch");
        store
            .queue_branch_ci_run_for_head(
                branch.branch_id,
                "head-skipped",
                &[crate::ci_manifest::ForgeLane {
                    id: "pikachat_typescript".to_string(),
                    title: "check-pikachat-typescript".to_string(),
                    entrypoint:
                        "./scripts/pikaci-staged-linux-remote.sh run pre-merge-pikachat-typescript"
                            .to_string(),
                    command: vec![
                        "./scripts/pikaci-staged-linux-remote.sh".to_string(),
                        "run".to_string(),
                        "pre-merge-pikachat-typescript".to_string(),
                    ],
                    paths: vec![],
                    concurrency_group: Some(
                        "staged-linux:pre-merge-pikachat-typescript".to_string(),
                    ),
                }],
            )
            .expect("queue ci");
        let skipped = store
            .claim_pending_branch_ci_lane_runs(1, 120)
            .expect("claim ci")
            .into_iter()
            .next()
            .expect("ci lane");
        store
            .finish_branch_ci_lane_run(
                skipped.lane_run_id,
                skipped.claim_token,
                crate::ci_state::CiLaneStatus::Skipped,
                "skipped; no changed files matched target filters",
            )
            .expect("finish skipped lane");

        let detail = store
            .get_branch_detail(branch.branch_id)
            .expect("branch detail")
            .expect("detail");
        let ci_runs = store
            .list_branch_ci_runs(branch.branch_id, 8)
            .expect("branch ci runs");
        let detail_rendered = render_detail_template(detail.clone(), ci_runs.clone(), false)
            .expect("render detail template")
            .render()
            .expect("render detail html");
        assert!(detail_rendered.contains("check-pikachat-typescript"));
        assert!(detail_rendered.contains("skipped"));

        let ci_rendered =
            render_branch_ci_template_with_notices(detail, ci_runs, Vec::new(), false)
                .expect("render branch ci template")
                .render()
                .expect("render branch ci html");
        assert!(ci_rendered.contains("check-pikachat-typescript"));
        assert!(ci_rendered.contains("skipped"));
    }

    #[test]
    fn nightly_page_renders_manual_rerun_provenance() {
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
        let failed = store
            .claim_pending_nightly_lane_runs(1, 120)
            .expect("claim nightly")
            .into_iter()
            .next()
            .expect("nightly lane");
        store
            .finish_nightly_lane_run(
                failed.lane_run_id,
                failed.claim_token,
                crate::ci_state::CiLaneStatus::Failed,
                "boom",
            )
            .expect("finish nightly");
        let rerun_run_id = store
            .rerun_nightly_lane(failed.nightly_run_id, failed.lane_run_id)
            .expect("rerun nightly")
            .expect("rerun run");

        let run = store
            .get_nightly_run(rerun_run_id)
            .expect("nightly detail")
            .expect("nightly run");
        let rendered = render_nightly_template(run)
            .render()
            .expect("render nightly html");
        assert!(rendered.contains("manual rerun of nightly #"));
        assert!(rendered.contains("manual rerun of lane #"));
    }

    #[test]
    fn nightly_page_renders_pikaci_run_metadata() {
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
                "nightly-pikaci-head",
                "2026-03-19T08:00:00Z",
                std::slice::from_ref(&lane),
            )
            .expect("queue nightly");
        let running = store
            .claim_pending_nightly_lane_runs(1, 120)
            .expect("claim nightly")
            .into_iter()
            .next()
            .expect("nightly lane");
        store
            .record_nightly_lane_pikaci_run(
                running.lane_run_id,
                running.claim_token,
                "pikaci-run-nightly-ui",
                Some("pre-merge-pika-rust"),
            )
            .expect("persist nightly pikaci metadata");

        let run = store
            .get_nightly_run(running.nightly_run_id)
            .expect("nightly detail")
            .expect("nightly run");
        let rendered = render_nightly_template(run)
            .render()
            .expect("render nightly html");
        assert!(rendered.contains("CI run"));
        assert!(rendered.contains("pikaci-run-nightly-ui"));
        assert!(rendered.contains("pre-merge-pika-rust"));
    }
