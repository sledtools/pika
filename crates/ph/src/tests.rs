use super::*;
use std::fs;
use std::net::TcpListener;
use std::path::Path;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use axum::routing::{get, post};
use axum::{Json, Router};
use nostr::Keys;
use nostr::ToBech32;
use tempfile::tempdir;

use crate::api::{
    ApiClient, BranchDetailResponse, BranchState, BranchSummary, CiLane, CiLaneExecutionReason,
    CiLaneFailureKind, CiRun, CiTargetHealthState, ForgeCiStatus, TutorialStatus,
};
use crate::commands::{
    branch_wait_snapshot, cmd_close, cmd_fail_lane, cmd_login, cmd_merge, cmd_requeue_lane,
    cmd_wait, cmd_wake_ci, cmd_whoami, render_branch_status,
};
use crate::resolve::{BranchRef, infer_current_branch, resolve_branch_ref};
use crate::session::{Session, load_session, save_session};

fn cwd_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn infer_current_branch_reads_git_worktree() {
    let _guard = cwd_test_lock().lock().expect("lock cwd test");
    let dir = tempdir().expect("temp dir");
    git(dir.path(), &["init"]);
    git(dir.path(), &["config", "user.name", "Test User"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    fs::write(dir.path().join("README.md"), "hello\n").expect("write file");
    git(dir.path(), &["add", "README.md"]);
    git(dir.path(), &["commit", "-m", "init"]);
    git(dir.path(), &["checkout", "-b", "feature/ph"]);

    let cwd = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(dir.path()).expect("chdir");
    let branch = infer_current_branch().expect("infer branch");
    std::env::set_current_dir(cwd).expect("restore cwd");

    assert_eq!(branch, "feature/ph");
}

#[test]
fn login_persists_session_against_auth_flow() {
    let state_dir = tempdir().expect("state dir");
    let keys = Keys::generate();
    let nsec = keys.secret_key().to_secret_hex();
    let expected_npub = keys.public_key().to_bech32().expect("npub");
    let base_url = spawn_test_server(
        Router::new()
            .route(
                "/news/auth/challenge",
                post(|| async { Json(serde_json::json!({"challenge": "nonce-123"})) }),
            )
            .route("/news/auth/verify", {
                let expected_npub = expected_npub.clone();
                post(move |Json(body): Json<serde_json::Value>| {
                    let expected_npub = expected_npub.clone();
                    async move {
                        let event_raw = body["event"].as_str().expect("event json");
                        let event: serde_json::Value =
                            serde_json::from_str(event_raw).expect("parse event");
                        assert_eq!(event["content"], "nonce-123");
                        Json(serde_json::json!({
                            "token": "bearer-123",
                            "npub": expected_npub,
                            "is_admin": false,
                            "can_forge_write": true
                        }))
                    }
                })
            }),
    );

    let cli = Cli::parse_from([
        "ph",
        "--state-dir",
        state_dir.path().to_str().expect("state dir path"),
        "--base-url",
        &base_url,
        "login",
        "--nsec",
        &nsec,
    ]);
    cmd_login(
        &cli,
        LoginArgs {
            nsec: Some(nsec.clone()),
            nsec_file: None,
        },
    )
    .expect("login");

    let session = load_session(state_dir.path()).expect("session");
    assert_eq!(session.token, "bearer-123");
    assert_eq!(session.npub, expected_npub);
    assert!(session.can_forge_write);
}

#[test]
fn wait_returns_error_when_ci_fails() {
    let state_dir = tempdir().expect("state dir");
    save_session(
        state_dir.path(),
        &Session {
            base_url: "http://placeholder".to_string(),
            token: "token".to_string(),
            npub: "npub1test".to_string(),
            is_admin: false,
            can_forge_write: true,
        },
    )
    .expect("save session");

    let calls = Arc::new(AtomicUsize::new(0));
    let base_url = spawn_test_server({
        let calls = Arc::clone(&calls);
        Router::new()
            .route("/news/api/forge/branch/resolve", get(|| async {
                Json(serde_json::json!({
                    "branch_id": 7,
                    "repo": "sledtools/pika",
                    "branch_name": "feature/wait",
                    "branch_state": "open"
                }))
            }))
            .route(
                "/news/api/forge/branch/7",
                get(move || {
                    let calls = Arc::clone(&calls);
                    async move {
                        let idx = calls.fetch_add(1, Ordering::SeqCst);
                        let ci_status = if idx == 0 { "running" } else { "failed" };
                        Json(serde_json::json!({
                            "branch": {
                                "branch_id": 7,
                                "repo": "sledtools/pika",
                                "branch_name": "feature/wait",
                                "title": "wait",
                                "branch_state": "open",
                                "updated_at": "2026-03-19T00:00:00Z",
                                "target_branch": "master",
                                "head_sha": "deadbeef",
                                "merge_base_sha": "base",
                                "merge_commit_sha": null,
                                "tutorial_status": "ready",
                                "ci_status": ci_status,
                                "error_message": null
                            },
                            "ci_runs": [{
                                "id": 5,
                                "source_head_sha": "deadbeef",
                                "status": ci_status,
                                "lane_count": 1,
                                "rerun_of_run_id": null,
                                "created_at": "2026-03-19T00:00:00Z",
                                "started_at": "2026-03-19T00:00:01Z",
                                "finished_at": if ci_status == "failed" { serde_json::json!("2026-03-19T00:00:02Z") } else { serde_json::Value::Null },
                                "lanes": [{
                                    "id": 9,
                                    "lane_id": "check-pika",
                                    "title": "check-pika",
                                    "entrypoint": "just checks::pre-merge-pika",
                                    "status": ci_status,
                                    "pikaci_run_id": null,
                                    "pikaci_target_id": null,
                                    "log_text": "boom",
                                    "retry_count": 0,
                                    "rerun_of_lane_run_id": null,
                                    "created_at": "2026-03-19T00:00:00Z",
                                    "started_at": "2026-03-19T00:00:01Z",
                                    "finished_at": if ci_status == "failed" { serde_json::json!("2026-03-19T00:00:02Z") } else { serde_json::Value::Null }
                                }]
                            }]
                        }))
                    }
                }),
            )
    });
    let mut session = load_session(state_dir.path()).expect("session");
    session.base_url = base_url.clone();
    save_session(state_dir.path(), &session).expect("update session");
    let cli = Cli::parse_from([
        "ph",
        "--state-dir",
        state_dir.path().to_str().expect("state dir path"),
        "wait",
        "--poll-secs",
        "0",
        "feature/wait",
    ]);
    let result = cmd_wait(
        &cli,
        match &cli.command {
            PhCommand::Wait { branch_or_id, .. } => branch_or_id.as_deref(),
            _ => unreachable!(),
        },
        0,
    );
    assert!(result.is_err());
}

#[test]
fn branch_status_renders_blocked_unhealthy_and_failure_details() {
    let branch = BranchDetailResponse {
        branch: BranchSummary {
            branch_id: 7,
            repo: "sledtools/pika".to_string(),
            branch_name: "feature/state".to_string(),
            title: "state".to_string(),
            branch_state: BranchState::Open,
            updated_at: "2026-03-24T00:00:00Z".to_string(),
            target_branch: "master".to_string(),
            head_sha: "deadbeefdeadbeef".to_string(),
            merge_base_sha: "cafebabecafebabe".to_string(),
            merge_commit_sha: None,
            tutorial_status: TutorialStatus::Ready,
            ci_status: ForgeCiStatus::Running,
            error_message: None,
        },
        ci_runs: vec![CiRun {
            id: 11,
            source_head_sha: "deadbeefdeadbeef".to_string(),
            status: ForgeCiStatus::Running,
            status_tone: Some("warning".to_string()),
            lane_count: 3,
            rerun_of_run_id: None,
            created_at: "2026-03-24T00:00:00Z".to_string(),
            started_at: Some("2026-03-24T00:00:01Z".to_string()),
            finished_at: None,
            timing_summary: None,
            lanes: vec![
                CiLane {
                    id: 91,
                    lane_id: "wait-capacity".to_string(),
                    title: "wait".to_string(),
                    entrypoint: "just wait".to_string(),
                    status: pika_forge_model::CiLaneStatus::Queued,
                    status_tone: Some("warning".to_string()),
                    status_badge_class: None,
                    is_failed: None,
                    execution_reason: CiLaneExecutionReason::WaitingForCapacity,
                    execution_reason_label: Some("waiting for capacity".to_string()),
                    failure_kind: None,
                    failure_kind_label: None,
                    pikaci_run_id: None,
                    pikaci_target_id: None,
                    ci_target_key: None,
                    target_health_state: None,
                    target_health_summary: None,
                    log_text: None,
                    retry_count: 0,
                    rerun_of_lane_run_id: None,
                    created_at: "2026-03-24T00:00:00Z".to_string(),
                    started_at: None,
                    finished_at: None,
                    timing_summary: None,
                    last_heartbeat_at: None,
                    lease_expires_at: None,
                    operator_hint: None,
                },
                CiLane {
                    id: 92,
                    lane_id: "apple-sanity".to_string(),
                    title: "apple".to_string(),
                    entrypoint: "just apple".to_string(),
                    status: pika_forge_model::CiLaneStatus::Queued,
                    status_tone: Some("warning".to_string()),
                    status_badge_class: None,
                    is_failed: None,
                    execution_reason: CiLaneExecutionReason::TargetUnhealthy,
                    execution_reason_label: Some("target unhealthy".to_string()),
                    failure_kind: None,
                    failure_kind_label: None,
                    pikaci_run_id: None,
                    pikaci_target_id: None,
                    ci_target_key: Some("apple-host".to_string()),
                    target_health_state: Some(CiTargetHealthState::Unhealthy),
                    target_health_summary: Some(
                        "target apple-host unhealthy · consecutive infra failures 2 · cooloff until 2026-03-24T00:15:00Z"
                            .to_string(),
                    ),
                    log_text: None,
                    retry_count: 1,
                    rerun_of_lane_run_id: None,
                    created_at: "2026-03-24T00:00:00Z".to_string(),
                    started_at: None,
                    finished_at: None,
                    timing_summary: None,
                    last_heartbeat_at: None,
                    lease_expires_at: None,
                    operator_hint: None,
                },
                CiLane {
                    id: 93,
                    lane_id: "linux-tests".to_string(),
                    title: "linux".to_string(),
                    entrypoint: "just linux".to_string(),
                    status: pika_forge_model::CiLaneStatus::Failed,
                    status_tone: Some("danger".to_string()),
                    status_badge_class: None,
                    is_failed: None,
                    execution_reason: CiLaneExecutionReason::Running,
                    execution_reason_label: Some("running".to_string()),
                    failure_kind: Some(CiLaneFailureKind::Infrastructure),
                    failure_kind_label: Some("infrastructure".to_string()),
                    pikaci_run_id: Some("pikaci-123".to_string()),
                    pikaci_target_id: Some("pre-merge-pika-rust".to_string()),
                    ci_target_key: Some("pre-merge-pika-rust".to_string()),
                    target_health_state: Some(CiTargetHealthState::Healthy),
                    target_health_summary: None,
                    log_text: Some("ci runner error: permission denied".to_string()),
                    retry_count: 0,
                    rerun_of_lane_run_id: None,
                    created_at: "2026-03-24T00:00:00Z".to_string(),
                    started_at: Some("2026-03-24T00:00:01Z".to_string()),
                    finished_at: Some("2026-03-24T00:02:00Z".to_string()),
                    timing_summary: None,
                    last_heartbeat_at: None,
                    lease_expires_at: None,
                    operator_hint: None,
                },
            ],
        }],
    };

    let rendered = render_branch_status(&branch);
    assert!(rendered.contains("wait-capacity queued · waiting for capacity"));
    assert!(rendered.contains("apple-sanity queued · target unhealthy"));
    assert!(rendered.contains("target apple-host unhealthy"));
    assert!(rendered.contains("linux-tests failed · failure=infrastructure"));

    let snapshot = branch_wait_snapshot(&branch);
    assert!(snapshot.contains("waiting_for_capacity"));
    assert!(snapshot.contains("target_unhealthy"));
}

#[test]
fn authenticated_commands_refuse_cross_host_token_reuse() {
    let state_dir = tempdir().expect("state dir");
    save_session(
        state_dir.path(),
        &Session {
            base_url: "https://git.pikachat.org".to_string(),
            token: "token".to_string(),
            npub: "npub1test".to_string(),
            is_admin: false,
            can_forge_write: true,
        },
    )
    .expect("save session");

    let cli = Cli::parse_from([
        "ph",
        "--state-dir",
        state_dir.path().to_str().expect("state dir path"),
        "--base-url",
        "https://other-host.example",
        "whoami",
    ]);
    let err = cmd_whoami(&cli).expect_err("cross-host token reuse should fail");
    assert!(err.to_string().contains("refusing to reuse its token"));
}

#[test]
fn resolve_branch_ref_accepts_closed_branch_name() {
    let state_dir = tempdir().expect("state dir");
    save_session(
        state_dir.path(),
        &Session {
            base_url: "http://placeholder".to_string(),
            token: "token".to_string(),
            npub: "npub1test".to_string(),
            is_admin: false,
            can_forge_write: true,
        },
    )
    .expect("save session");

    let base_url = spawn_test_server(Router::new().route(
        "/news/api/forge/branch/resolve",
        get(|| async {
            Json(serde_json::json!({
                "branch_id": 19,
                "repo": "sledtools/pika",
                "branch_name": "feature/history",
                "branch_state": "merged"
            }))
        }),
    ));
    let mut session = load_session(state_dir.path()).expect("session");
    session.base_url = base_url.clone();
    save_session(state_dir.path(), &session).expect("save session");
    let api = ApiClient::new(base_url, Some(session.token)).expect("api");

    let resolved = resolve_branch_ref(&api, Some("feature/history")).expect("resolve branch");

    assert_eq!(
        resolved,
        BranchRef {
            branch_id: 19,
            branch_name: Some("feature/history".to_string()),
        }
    );
}

#[test]
fn merge_and_close_use_authenticated_json_endpoints() {
    let state_dir = tempdir().expect("state dir");
    save_session(
        state_dir.path(),
        &Session {
            base_url: "http://placeholder".to_string(),
            token: "token-123".to_string(),
            npub: "npub1test".to_string(),
            is_admin: false,
            can_forge_write: true,
        },
    )
    .expect("save session");
    let merge_auth = Arc::new(AtomicUsize::new(0));
    let close_auth = Arc::new(AtomicUsize::new(0));
    let base_url = spawn_test_server({
        let merge_auth = Arc::clone(&merge_auth);
        let close_auth = Arc::clone(&close_auth);
        Router::new()
            .route(
                "/news/api/forge/branch/resolve",
                get(|| async {
                    Json(serde_json::json!({
                        "branch_id": 11,
                        "repo": "sledtools/pika",
                        "branch_name": "feature/merge",
                        "branch_state": "open"
                    }))
                }),
            )
            .route(
                "/news/api/forge/branch/11/merge",
                post(move |headers: axum::http::HeaderMap| {
                    let merge_auth = Arc::clone(&merge_auth);
                    async move {
                        if headers.get("authorization").and_then(|v| v.to_str().ok())
                            == Some("Bearer token-123")
                        {
                            merge_auth.fetch_add(1, Ordering::SeqCst);
                        }
                        Json(serde_json::json!({
                            "status": "ok",
                            "branch_id": 11,
                            "merge_commit_sha": "abc123"
                        }))
                    }
                }),
            )
            .route(
                "/news/api/forge/branch/11/close",
                post(move |headers: axum::http::HeaderMap| {
                    let close_auth = Arc::clone(&close_auth);
                    async move {
                        if headers.get("authorization").and_then(|v| v.to_str().ok())
                            == Some("Bearer token-123")
                        {
                            close_auth.fetch_add(1, Ordering::SeqCst);
                        }
                        Json(serde_json::json!({
                            "status": "ok",
                            "branch_id": 11,
                            "deleted": true
                        }))
                    }
                }),
            )
    });
    let mut session = load_session(state_dir.path()).expect("session");
    session.base_url = base_url;
    save_session(state_dir.path(), &session).expect("save session");
    let cli = Cli::parse_from([
        "ph",
        "--state-dir",
        state_dir.path().to_str().expect("state dir path"),
        "merge",
        "feature/merge",
    ]);
    cmd_merge(&cli, Some("feature/merge")).expect("merge");
    let cli = Cli::parse_from([
        "ph",
        "--state-dir",
        state_dir.path().to_str().expect("state dir path"),
        "close",
        "feature/merge",
    ]);
    cmd_close(&cli, Some("feature/merge")).expect("close");
    assert_eq!(merge_auth.load(Ordering::SeqCst), 1);
    assert_eq!(close_auth.load(Ordering::SeqCst), 1);
}

#[test]
fn fail_lane_resolves_branch_lane_name_against_latest_run() {
    let state_dir = tempdir().expect("state dir");
    save_session(
        state_dir.path(),
        &Session {
            base_url: "http://placeholder".to_string(),
            token: "token-123".to_string(),
            npub: "npub1test".to_string(),
            is_admin: false,
            can_forge_write: true,
        },
    )
    .expect("save session");
    let fail_auth = Arc::new(AtomicUsize::new(0));
    let base_url = spawn_test_server({
        let fail_auth = Arc::clone(&fail_auth);
        Router::new()
            .route(
                "/news/api/forge/branch/resolve",
                get(|| async {
                    Json(serde_json::json!({
                        "branch_id": 7,
                        "repo": "sledtools/pika",
                        "branch_name": "feature/recover",
                        "branch_state": "open"
                    }))
                }),
            )
            .route(
                "/news/api/forge/branch/7",
                get(|| async {
                    Json(serde_json::json!({
                        "branch": {
                            "branch_id": 7,
                            "repo": "sledtools/pika",
                            "branch_name": "feature/recover",
                            "title": "recover",
                            "branch_state": "open",
                            "updated_at": "2026-03-19T00:00:00Z",
                            "target_branch": "master",
                            "head_sha": "deadbeef",
                            "merge_base_sha": "base",
                            "merge_commit_sha": null,
                            "tutorial_status": "ready",
                            "ci_status": "running",
                            "error_message": null
                        },
                        "ci_runs": [
                            {
                                "id": 5,
                                "source_head_sha": "deadbeef",
                                "status": "running",
                                "lane_count": 1,
                                "rerun_of_run_id": null,
                                "created_at": "2026-03-19T00:00:00Z",
                                "started_at": "2026-03-19T00:00:01Z",
                                "finished_at": null,
                                "lanes": [{
                                    "id": 91,
                                    "lane_id": "check-pika",
                                    "title": "check-pika",
                                    "entrypoint": "just checks::pre-merge-pika",
                                    "status": "running",
                                    "pikaci_run_id": null,
                                    "pikaci_target_id": null,
                                    "log_text": null,
                                    "retry_count": 0,
                                    "rerun_of_lane_run_id": null,
                                    "created_at": "2026-03-19T00:00:00Z",
                                    "started_at": "2026-03-19T00:00:01Z",
                                    "finished_at": null
                                }]
                            },
                            {
                                "id": 4,
                                "source_head_sha": "cafebabe",
                                "status": "failed",
                                "lane_count": 1,
                                "rerun_of_run_id": null,
                                "created_at": "2026-03-18T00:00:00Z",
                                "started_at": "2026-03-18T00:00:01Z",
                                "finished_at": "2026-03-18T00:00:10Z",
                                "lanes": [{
                                    "id": 90,
                                    "lane_id": "check-pika",
                                    "title": "check-pika",
                                    "entrypoint": "just checks::pre-merge-pika",
                                    "status": "failed",
                                    "pikaci_run_id": null,
                                    "pikaci_target_id": null,
                                    "log_text": "boom",
                                    "retry_count": 0,
                                    "rerun_of_lane_run_id": null,
                                    "created_at": "2026-03-18T00:00:00Z",
                                    "started_at": "2026-03-18T00:00:01Z",
                                    "finished_at": "2026-03-18T00:00:10Z"
                                }]
                            }
                        ]
                    }))
                }),
            )
            .route(
                "/news/branch/7/ci/fail/91",
                post(move |headers: axum::http::HeaderMap| {
                    let fail_auth = Arc::clone(&fail_auth);
                    async move {
                        if headers.get("authorization").and_then(|v| v.to_str().ok())
                            == Some("Bearer token-123")
                        {
                            fail_auth.fetch_add(1, Ordering::SeqCst);
                        }
                        Json(serde_json::json!({
                            "status": "ok",
                            "branch_id": 7,
                            "lane_run_id": 91,
                            "lane_status": "failed"
                        }))
                    }
                }),
            )
    });
    let mut session = load_session(state_dir.path()).expect("session");
    session.base_url = base_url;
    save_session(state_dir.path(), &session).expect("save session");
    let cli = Cli::parse_from([
        "ph",
        "--state-dir",
        state_dir.path().to_str().expect("state dir path"),
        "fail-lane",
        "feature/recover",
        "--lane",
        "check-pika",
    ]);

    cmd_fail_lane(
        &cli,
        match &cli.command {
            PhCommand::FailLane(args) => args,
            _ => unreachable!(),
        },
    )
    .expect("fail lane");

    assert_eq!(fail_auth.load(Ordering::SeqCst), 1);
}

#[test]
fn requeue_lane_resolves_nightly_lane_name() {
    let state_dir = tempdir().expect("state dir");
    save_session(
        state_dir.path(),
        &Session {
            base_url: "http://placeholder".to_string(),
            token: "token-123".to_string(),
            npub: "npub1test".to_string(),
            is_admin: false,
            can_forge_write: true,
        },
    )
    .expect("save session");
    let requeue_auth = Arc::new(AtomicUsize::new(0));
    let base_url = spawn_test_server({
        let requeue_auth = Arc::clone(&requeue_auth);
        Router::new()
            .route(
                "/news/api/forge/nightly/12",
                get(|| async {
                    Json(serde_json::json!({
                        "nightly_run_id": 12,
                        "repo": "sledtools/pika",
                        "scheduled_for": "2026-03-19T00:00:00Z",
                        "created_at": "2026-03-19T00:00:00Z",
                        "source_ref": "refs/heads/master",
                        "source_head_sha": "deadbeef",
                        "status": "running",
                        "summary": null,
                        "rerun_of_run_id": null,
                        "started_at": "2026-03-19T00:00:01Z",
                        "finished_at": null,
                        "lanes": [{
                            "id": 44,
                            "lane_id": "nightly_pika",
                            "title": "nightly-pika",
                            "entrypoint": "just checks::nightly-pika-e2e",
                            "status": "running",
                            "pikaci_run_id": null,
                            "pikaci_target_id": null,
                            "log_text": null,
                            "retry_count": 0,
                            "rerun_of_lane_run_id": null,
                            "created_at": "2026-03-19T00:00:00Z",
                            "started_at": "2026-03-19T00:00:01Z",
                            "finished_at": null
                        }]
                    }))
                }),
            )
            .route(
                "/news/nightly/12/requeue/44",
                post(move |headers: axum::http::HeaderMap| {
                    let requeue_auth = Arc::clone(&requeue_auth);
                    async move {
                        if headers.get("authorization").and_then(|v| v.to_str().ok())
                            == Some("Bearer token-123")
                        {
                            requeue_auth.fetch_add(1, Ordering::SeqCst);
                        }
                        Json(serde_json::json!({
                            "status": "ok",
                            "nightly_run_id": 12,
                            "lane_run_id": 44,
                            "lane_status": "queued"
                        }))
                    }
                }),
            )
    });
    let mut session = load_session(state_dir.path()).expect("session");
    session.base_url = base_url;
    save_session(state_dir.path(), &session).expect("save session");
    let cli = Cli::parse_from([
        "ph",
        "--state-dir",
        state_dir.path().to_str().expect("state dir path"),
        "requeue-lane",
        "--nightly-run-id",
        "12",
        "--lane",
        "nightly_pika",
    ]);

    cmd_requeue_lane(
        &cli,
        match &cli.command {
            PhCommand::RequeueLane(args) => args,
            _ => unreachable!(),
        },
    )
    .expect("requeue lane");

    assert_eq!(requeue_auth.load(Ordering::SeqCst), 1);
}

#[test]
fn wake_ci_hits_scheduler_wake_endpoint() {
    let state_dir = tempdir().expect("state dir");
    save_session(
        state_dir.path(),
        &Session {
            base_url: "http://placeholder".to_string(),
            token: "token-123".to_string(),
            npub: "npub1test".to_string(),
            is_admin: false,
            can_forge_write: true,
        },
    )
    .expect("save session");
    let wake_auth = Arc::new(AtomicUsize::new(0));
    let base_url = spawn_test_server({
        let wake_auth = Arc::clone(&wake_auth);
        Router::new().route(
            "/news/api/forge/ci/wake",
            post(move |headers: axum::http::HeaderMap| {
                let wake_auth = Arc::clone(&wake_auth);
                async move {
                    if headers.get("authorization").and_then(|v| v.to_str().ok())
                        == Some("Bearer token-123")
                    {
                        wake_auth.fetch_add(1, Ordering::SeqCst);
                    }
                    Json(serde_json::json!({
                        "status": "ok",
                        "message": "scheduler wake requested"
                    }))
                }
            }),
        )
    });
    let mut session = load_session(state_dir.path()).expect("session");
    session.base_url = base_url;
    save_session(state_dir.path(), &session).expect("save session");
    let cli = Cli::parse_from([
        "ph",
        "--state-dir",
        state_dir.path().to_str().expect("state dir path"),
        "wake-ci",
    ]);

    cmd_wake_ci(&cli).expect("wake ci");

    assert_eq!(wake_auth.load(Ordering::SeqCst), 1);
}

fn git<P: AsRef<Path>>(cwd: P, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn spawn_test_server(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    listener
        .set_nonblocking(true)
        .expect("set listener nonblocking");
    let addr = listener.local_addr().expect("local addr");
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async move {
            axum::serve(
                tokio::net::TcpListener::from_std(listener).expect("tokio listener"),
                app,
            )
            .await
            .expect("serve test app");
        });
    });
    std::thread::sleep(Duration::from_millis(50));
    format!("http://{}", addr)
}
