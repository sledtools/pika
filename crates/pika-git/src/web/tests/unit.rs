    #[test]
    fn ci_follow_up_wake_only_for_material_progress() {
        assert!(!ci_pass_needs_follow_up_wake(&ci::CiPassResult::default()));

        assert!(ci_pass_needs_follow_up_wake(&ci::CiPassResult {
            claimed: 1,
            ..Default::default()
        }));
        assert!(ci_pass_needs_follow_up_wake(&ci::CiPassResult {
            succeeded: 1,
            ..Default::default()
        }));
        assert!(ci_pass_needs_follow_up_wake(&ci::CiPassResult {
            failed: 1,
            ..Default::default()
        }));
        assert!(ci_pass_needs_follow_up_wake(&ci::CiPassResult {
            nightlies_scheduled: 1,
            ..Default::default()
        }));
        assert!(ci_pass_needs_follow_up_wake(&ci::CiPassResult {
            retries_recovered: 1,
            ..Default::default()
        }));
    }

    #[test]
    fn sanitizes_markdown_html_output() {
        let rendered = markdown_to_safe_html("ok<script>alert('xss')</script>");
        assert!(rendered.contains("ok"));
        assert!(!rendered.contains("<script>"));
    }

    #[test]
    fn valid_signature_accepted() {
        let secret = "test-secret";
        let payload = b"hello world";

        // Compute expected signature.
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(payload);
        let sig = hex::encode(mac.finalize().into_bytes());
        let header = format!("sha256={}", sig);

        assert!(verify_signature(secret, payload, &header));
    }

    #[test]
    fn forge_startup_issues_surface_missing_secret_and_mirror_remote() {
        let root = tempfile::tempdir().expect("create temp root");
        let mut config = forge_test_config_with_git_dir(&root.path().join("pika.git"));
        config.forge_repo.as_mut().expect("forge repo").hook_url =
            Some("http://127.0.0.1:8788/git/webhook".to_string());
        config.bootstrap_admin_npubs.clear();
        let forge_repo = config.effective_forge_repo().expect("forge repo");
        let issues = collect_forge_startup_issues(&config, &forge_repo, None);
        let codes: Vec<&str> = issues.iter().map(|issue| issue.code.as_str()).collect();
        assert!(codes.contains(&"webhook_secret_missing"));
        assert!(codes.contains(&"mirror_remote_missing"));
        assert!(!codes.contains(&"canonical_repo_unavailable"));
    }

    #[test]
    fn forge_runtime_issues_clear_after_hook_install_recovery() {
        let root = tempfile::tempdir().expect("create temp root");
        let canonical = root.path().join("recovered.git");
        let mut config = forge_test_config_with_git_dir(&canonical);
        config.forge_repo.as_mut().expect("forge repo").hook_url =
            Some("http://127.0.0.1:8788/git/webhook".to_string());
        config.bootstrap_admin_npubs.clear();

        let output = Command::new("git")
            .args([
                "init",
                "--bare",
                canonical.to_str().expect("canonical path"),
            ])
            .output()
            .expect("init bare repo");
        assert!(
            output.status.success(),
            "git init --bare failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::remove_dir_all(canonical.join("hooks")).expect("remove hooks dir");
        fs::write(canonical.join("hooks"), "blocked").expect("create blocking hooks file");

        let issues = current_forge_runtime_issues(&config, Some("secret"));
        assert!(issues
            .iter()
            .any(|issue| issue.code == "hook_install_failed"));

        fs::remove_file(canonical.join("hooks")).expect("remove blocking hooks file");

        let issues = current_forge_runtime_issues(&config, Some("secret"));
        assert!(!issues
            .iter()
            .any(|issue| issue.code == "hook_install_failed"));
    }

    #[test]
    fn browser_auth_bootstrap_uses_local_storage_across_tabs() {
        let base_template = include_str!("../../../templates/base.html");
        let inbox_template = include_str!("../../../templates/inbox.html");
        let admin_template = include_str!("../../../templates/admin.html");

        assert!(base_template.contains("const authStorage = window.localStorage;"));
        assert!(base_template.contains("window.addEventListener('storage'"));
        assert!(!base_template.contains("sessionStorage.getItem('pika_git_token')"));
        assert!(!base_template.contains("sessionStorage.setItem('pika_git_token'"));
        assert!(!base_template.contains("sessionStorage.removeItem('pika_git_token'"));
        assert!(!inbox_template.contains("sessionStorage.removeItem('pika_git_token'"));
        assert!(!admin_template.contains("sessionStorage.removeItem('pika_git_token'"));
    }

    #[test]
    fn mirror_health_distinguishes_disabled_and_error_states() {
        let disabled = build_mirror_health_status(
            &MirrorRuntimeStatus {
                configured: true,
                remote_name: Some("github".to_string()),
                background_enabled: false,
                background_interval_secs: Some(0),
                timeout_secs: Some(120),
                active_run: None,
                github_token_env: "GITHUB_TOKEN".to_string(),
            },
            None,
        );
        assert_eq!(disabled.state, "disabled");

        let errored = build_mirror_health_status(
            &MirrorRuntimeStatus {
                configured: true,
                remote_name: Some("github".to_string()),
                background_enabled: true,
                background_interval_secs: Some(300),
                timeout_secs: Some(120),
                active_run: None,
                github_token_env: "GITHUB_TOKEN".to_string(),
            },
            Some(&MirrorStatusRecord {
                remote_name: "github".to_string(),
                last_attempt: Some(MirrorSyncRunRecord {
                    id: 1,
                    remote_name: "github".to_string(),
                    trigger_source: "background".to_string(),
                    status: "failed".to_string(),
                    failure_kind: Some("config".to_string()),
                    local_default_head: None,
                    remote_default_head: None,
                    lagging_ref_count: None,
                    synced_ref_count: None,
                    error_text: Some("boom".to_string()),
                    created_at: "2026-03-19T10:00:00Z".to_string(),
                    finished_at: "2026-03-19T10:00:01Z".to_string(),
                }),
                last_success_at: None,
                last_failure_at: Some("2026-03-19T10:00:01Z".to_string()),
                consecutive_failure_count: 1,
                current_lagging_ref_count: None,
                current_failure_kind: Some("config".to_string()),
            }),
        );
        assert_eq!(errored.state, "error");
    }

    #[test]
    fn mirror_health_surfaces_active_and_stale_lock_state() {
        let active = build_mirror_health_status(
            &MirrorRuntimeStatus {
                configured: true,
                remote_name: Some("github".to_string()),
                background_enabled: true,
                background_interval_secs: Some(300),
                timeout_secs: Some(120),
                active_run: Some(crate::forge::MirrorLockStatus {
                    state: "active".to_string(),
                    pid: Some(4242),
                    trigger_source: Some("post-mutation".to_string()),
                    operation: Some("git push --prune mirror".to_string()),
                    started_at: Some("2026-03-24T12:00:00Z".to_string()),
                    age_secs: Some(7),
                }),
                github_token_env: "GITHUB_TOKEN".to_string(),
            },
            None,
        );
        assert_eq!(active.state, "active");

        let stale = build_mirror_health_status(
            &MirrorRuntimeStatus {
                configured: true,
                remote_name: Some("github".to_string()),
                background_enabled: true,
                background_interval_secs: Some(300),
                timeout_secs: Some(120),
                active_run: Some(crate::forge::MirrorLockStatus {
                    state: "stale".to_string(),
                    pid: Some(4242),
                    trigger_source: Some("background".to_string()),
                    operation: Some("git push --prune mirror".to_string()),
                    started_at: Some("2026-03-24T12:00:00Z".to_string()),
                    age_secs: Some(999),
                }),
                github_token_env: "GITHUB_TOKEN".to_string(),
            },
            None,
        );
        assert_eq!(stale.state, "error");
        assert!(stale
            .summary
            .unwrap_or_default()
            .contains("stale mirror run"));
    }

    #[test]
    fn compact_duration_formatter_prefers_short_human_readable_forms() {
        assert_eq!(
            super::format_compact_duration(chrono::TimeDelta::seconds(12)),
            "12s"
        );
        assert_eq!(
            super::format_compact_duration(chrono::TimeDelta::seconds(68)),
            "1m 08s"
        );
        assert_eq!(
            super::format_compact_duration(chrono::TimeDelta::seconds(2 * 3_600 + 3 * 60)),
            "2h 03m"
        );
    }

    #[test]
    fn ci_timing_summary_tracks_queued_running_and_finished_time() {
        let now = Utc
            .with_ymd_and_hms(2026, 3, 24, 12, 0, 45)
            .single()
            .expect("valid timestamp");

        assert_eq!(
            super::ci_timing_summary("2026-03-24T12:00:31Z", None, None, now)
                .expect("queued summary"),
            "queued 14s"
        );
        assert_eq!(
            super::ci_timing_summary(
                "2026-03-24T12:00:00Z",
                Some("2026-03-24T12:00:14Z"),
                None,
                now,
            )
            .expect("running summary"),
            "queued 14s · ran 31s"
        );
        assert_eq!(
            super::ci_timing_summary(
                "2026-03-24T12:00:00Z",
                Some("2026-03-24T12:00:14Z"),
                Some("2026-03-24T12:00:45Z"),
                now,
            )
            .expect("finished summary"),
            "queued 14s · ran 31s"
        );
        assert_eq!(
            super::ci_timing_summary(
                "2026-03-24T12:00:00Z",
                None,
                Some("2026-03-24T12:00:14Z"),
                now,
            )
            .expect("finished-while-never-started summary"),
            "queued 14s"
        );
    }

    #[test]
    fn branch_ci_templates_render_timing_summaries() {
        let summary_html = super::BranchCiSummaryTemplate {
            ci_status: "running".to_string(),
            ci_status_tone: "warning".to_string(),
            live_active: true,
            ci_details_path: "/git/branch/7/ci".to_string(),
            latest_run: Some(super::CiSummaryRunView {
                id: 14,
                status: "running".to_string(),
                status_tone: "warning".to_string(),
                lane_count: 1,
                created_at: "2026-03-24T12:00:00Z".to_string(),
                source_head_sha: "abc123".to_string(),
                rerun_of_run_id: None,
                timing_summary: Some("queued 14s · running 31s".to_string()),
                success_count: 0,
                active_count: 1,
                failed_count: 0,
                lanes: vec![super::CiSummaryLaneView {
                    title: "pre-merge-pika".to_string(),
                    status: "running".to_string(),
                    status_tone: "warning".to_string(),
                }],
            }),
            page_notices: Vec::new(),
        }
        .render()
        .expect("render summary template");
        assert!(summary_html.contains("queued 14s"));
        assert!(summary_html.contains("running 31s"));

        let live_html = super::BranchCiLiveTemplate {
            branch_id: 7,
            branch_state: "open".to_string(),
            tutorial_status: "ready".to_string(),
            ci_status: "running".to_string(),
            ci_status_tone: "warning".to_string(),
            live_active: true,
            ci_runs: vec![super::CiRunView {
                id: 14,
                source_head_sha: "abc123".to_string(),
                status: "running".to_string(),
                status_tone: "warning".to_string(),
                lane_count: 1,
                rerun_of_run_id: None,
                created_at: "2026-03-24T12:00:00Z".to_string(),
                started_at: Some("2026-03-24T12:00:14Z".to_string()),
                finished_at: None,
                timing_summary: Some("queued 14s · running 31s".to_string()),
                lanes: vec![super::CiLaneView {
                    id: 23,
                    lane_id: "pika".to_string(),
                    title: "pre-merge-pika".to_string(),
                    entrypoint: "just checks::pre-merge-pika".to_string(),
                    status: "running".to_string(),
                    status_tone: "warning".to_string(),
                    execution_reason: "running".to_string(),
                    execution_reason_label: "running".to_string(),
                    failure_kind: None,
                    failure_kind_label: None,
                    pikaci_run_id: None,
                    pikaci_target_id: None,
                    ci_target_key: None,
                    log_text: None,
                    retry_count: 0,
                    rerun_of_lane_run_id: None,
                    created_at: "2026-03-24T12:00:00Z".to_string(),
                    started_at: Some("2026-03-24T12:00:14Z".to_string()),
                    finished_at: None,
                    timing_summary: Some("queued 14s · running 31s".to_string()),
                    last_heartbeat_at: None,
                    lease_expires_at: None,
                    operator_hint: None,
                }],
            }],
            page_notices: Vec::new(),
            latest_failed_lane_count: 0,
        }
        .render()
        .expect("render live template");
        assert!(live_html.contains("queued 14s · running 31s"));
    }

    #[test]
    fn wrong_secret_rejected() {
        let payload = b"hello world";

        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"right-secret").unwrap();
        mac.update(payload);
        let sig = hex::encode(mac.finalize().into_bytes());
        let header = format!("sha256={}", sig);

        assert!(!verify_signature("wrong-secret", payload, &header));
    }

    #[test]
    fn missing_prefix_rejected() {
        assert!(!verify_signature("secret", b"body", "bad-header"));
    }

    #[test]
    fn invalid_hex_rejected() {
        assert!(!verify_signature("secret", b"body", "sha256=zzzz"));
    }

    #[test]
    fn summarize_webhook_ref_updates_counts_branch_refs() {
        let payload = b"
oldsha newsha refs/heads/master
oldsha newsha refs/tags/v1
oldsha newsha refs/heads/feature/mirror
";
        assert_eq!(summarize_webhook_ref_updates(payload), (3, 2));
    }

    #[test]
    fn summarize_webhook_ref_updates_ignores_blank_and_malformed_lines() {
        let payload = b"

invalid
oldsha newsha
oldsha newsha refs/tags/v1
";
        assert_eq!(summarize_webhook_ref_updates(payload), (3, 0));
    }
