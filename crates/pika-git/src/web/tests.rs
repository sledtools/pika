use std::fs;
use std::process::Command;
use std::sync::Arc;

use askama::Template;
use axum::body::to_bytes;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header::AUTHORIZATION, HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use chrono::{TimeZone, Utc};

use crate::jerichoci_store::{JerichociRunStore, TestJerichociJobFixture, TestJerichociRunFixture};

use super::{
    api_forge_branch_detail_handler, api_forge_branch_logs_handler,
    api_forge_branch_resolve_handler, api_forge_ci_logs_handler,
    api_forge_ci_prepared_outputs_handler, api_forge_ci_run_handler, api_inbox_count_handler,
    api_inbox_list_handler, api_inbox_mark_reviewed_handler, auth_challenge_handler,
    branch_ci_stream_handler, fail_branch_ci_lane_handler, fail_nightly_lane_handler,
    inbox_review_handler, load_branch_ci_live_snapshot, load_nightly_live_snapshot,
    markdown_to_safe_html, next_branch_ci_live_snapshot, next_nightly_live_snapshot,
    nightly_stream_handler, recover_branch_ci_run_handler, render_branch_ci_template_with_notices,
    render_detail_template, render_detail_template_with_notices, render_nightly_template,
    rerun_branch_ci_lane_handler, rerun_nightly_lane_handler,
    should_backfill_managed_allowlist_entry, summarize_webhook_ref_updates, verify_signature,
    wake_ci_handler, webhook_handler, AppState, CiLiveUpdates, ForgeBranchLogsQuery,
    ForgeBranchResolveQuery, ForgeCiLogsQuery, InboxListParams, PageNoticeView, ReviewModeQuery,
};
use crate::auth::AuthState;
use crate::branch_store::BranchUpsertInput;
use crate::ci;
use crate::config::{Config, ForgeRepoConfig};
use crate::forge;
use crate::forge_runtime::{
    build_mirror_health_status, ci_pass_needs_follow_up_wake, collect_forge_startup_issues,
    current_forge_runtime_issues, ForgeRuntime,
};
use crate::forge_service::ForgeService;
use crate::mirror::MirrorRuntimeStatus;
use crate::mirror_store::{MirrorStatusRecord, MirrorSyncRunRecord};
use crate::poller;
use crate::storage::ChatAllowlistEntry;
use crate::storage::Store;
use crate::test_support::GitTestRepo;

const TRUSTED_NPUB: &str = "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y";

fn branch_upsert_input(branch_name: &str, head_sha: &str) -> BranchUpsertInput {
    BranchUpsertInput {
        repo: "sledtools/pika".to_string(),
        canonical_git_dir: "/tmp/pika.git".to_string(),
        default_branch: "master".to_string(),
        ci_entrypoint: "just pre-merge".to_string(),
        branch_name: branch_name.to_string(),
        title: format!("{branch_name} title"),
        head_sha: head_sha.to_string(),
        merge_base_sha: "base123".to_string(),
        author_name: Some("alice".to_string()),
        author_email: Some("alice@example.com".to_string()),
        updated_at: "2026-03-18T12:00:00Z".to_string(),
    }
}

fn forge_test_repo_config(canonical_git_dir: impl Into<String>) -> ForgeRepoConfig {
    ForgeRepoConfig {
        repo: "sledtools/pika".to_string(),
        canonical_git_dir: canonical_git_dir.into(),
        default_branch: "master".to_string(),
        ci_concurrency: Some(2),
        mirror_remote: None,
        mirror_poll_interval_secs: None,
        mirror_timeout_secs: None,
        ci_command: vec!["just".to_string(), "pre-merge".to_string()],
        hook_url: None,
    }
}

fn forge_test_config() -> Config {
    let mut config = Config::test_with_forge_repo(forge_test_repo_config("/tmp/pika.git"));
    config.bootstrap_admin_npubs = vec![TRUSTED_NPUB.to_string()];
    config
}

fn forge_test_config_without_admins() -> Config {
    let mut config = forge_test_config();
    config.bootstrap_admin_npubs.clear();
    config
}

fn forge_test_config_with_git_dir(canonical_git_dir: &std::path::Path) -> Config {
    let mut config = forge_test_config();
    config
        .forge_repo
        .as_mut()
        .expect("forge repo")
        .canonical_git_dir = canonical_git_dir.to_string_lossy().into_owned();
    config
}

struct WebTestContext {
    _dir: tempfile::TempDir,
    store: Store,
}

impl WebTestContext {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("pika-git.db");
        let store = Store::open(&db_path).expect("open store");
        Self { _dir: dir, store }
    }

    fn store(&self) -> Store {
        self.store.clone()
    }

    fn state(&self, config: Config) -> Arc<AppState> {
        test_state(self.store(), config)
    }

    fn trusted_headers(&self, npub: &str) -> HeaderMap {
        trusted_headers(&self.store, npub)
    }
}

fn write_pikaci_run_fixture(config: &Config, run_id: &str) {
    let run_store = JerichociRunStore::from_config(config).expect("CI run store");
    let mut fixture = TestJerichociRunFixture::passed(
        run_id,
        Some("pre-merge-pika-rust"),
        Some("Run staged pika rust"),
    );
    let mut job = TestJerichociJobFixture::passed_remote_linux("job-one", "job one");
    job.remote_linux_vm_execution = Some(jerichoci::RemoteLinuxVmExecutionRecord {
        backend: jerichoci::RemoteLinuxVmBackend::Incus,
        incus_image: Some(jerichoci::RemoteLinuxVmImageRecord {
            guest_role: None,
            project: "pika-managed-agents".to_string(),
            alias: "pikaci/dev".to_string(),
            fingerprint: Some("abc123".to_string()),
        }),
        phases: Vec::new(),
    });
    fixture.jobs.push(job);
    fixture.prepared_outputs = Some(
            serde_json::from_str(
                r#"{"schema_version":1,"outputs":[{"node_id":"prepare-pika-core-linux-rust-workspace-build","installable":"path:/tmp/snapshot#ci.x86_64-linux.workspaceBuild","output_name":"ci.x86_64-linux.workspaceBuild","protocol":"nix_store_path_v1","residency":"local_authoritative","consumer":"host_local_symlink_mounts_v1","realized_path":"/nix/store/workspace-build","consumer_request_path":null,"consumer_result_path":null,"consumer_launch_request_path":null,"consumer_transport_request_path":null,"exposures":[],"requested_exposures":[]}]}"#,
            )
            .expect("parse prepared outputs fixture"),
        );
    run_store
        .write_fixture(&fixture)
        .expect("write run fixture");
}

fn test_state_with_live_buffer(store: Store, config: Config, live_buffer: usize) -> Arc<AppState> {
    let forge_mode = config.effective_forge_repo().is_some();
    let live_updates = CiLiveUpdates::new(live_buffer);
    let forge_runtime = Arc::new(ForgeRuntime::blank(forge_mode));
    let forge_service = Arc::new(ForgeService::new(
        store.clone(),
        config.clone(),
        live_updates.clone(),
        Arc::clone(&forge_runtime),
    ));
    let jerichoci_run_store = JerichociRunStore::from_config(&config);
    Arc::new(AppState {
        auth: Arc::new(AuthState::new(&config.bootstrap_admin_npubs, store.clone())),
        store,
        config,
        live_updates,
        jerichoci_run_store,
        webhook_secret: None,
        forge_runtime,
        forge_service,
    })
}

fn test_state(store: Store, config: Config) -> Arc<AppState> {
    test_state_with_live_buffer(store, config, 64)
}

fn trusted_headers(store: &Store, npub: &str) -> HeaderMap {
    let token = "test-token";
    store
        .insert_auth_token(token, npub)
        .expect("insert auth token");
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).expect("auth header"),
    );
    headers
}

include!("tests/unit.rs");

include!("tests/webhook_inbox.rs");

include!("tests/api.rs");

include!("tests/live.rs");

include!("tests/render.rs");
