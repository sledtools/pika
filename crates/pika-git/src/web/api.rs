fn map_forge_branch_summary(detail: BranchDetailRecord) -> ForgeBranchSummaryResponse {
    ForgeBranchSummaryResponse {
        branch_id: detail.branch_id,
        repo: detail.repo,
        branch_name: detail.branch_name,
        title: detail.title,
        branch_state: detail.branch_state,
        updated_at: detail.updated_at,
        target_branch: detail.target_branch,
        head_sha: detail.head_sha,
        merge_base_sha: detail.merge_base_sha,
        merge_commit_sha: detail.merge_commit_sha,
        tutorial_status: detail.tutorial_status,
        ci_status: detail.ci_status,
        error_message: detail.error_message,
    }
}

fn map_api_ci_run(run: BranchCiRunRecord, now: DateTime<Utc>) -> CiRun {
    let view = map_ci_run_view(run, now);
    CiRun {
        id: view.id,
        source_head_sha: view.source_head_sha,
        status: ForgeCiStatus::from(view.status),
        status_tone: Some(view.status_tone),
        lane_count: view.lane_count,
        rerun_of_run_id: view.rerun_of_run_id,
        created_at: view.created_at,
        started_at: view.started_at,
        finished_at: view.finished_at,
        timing_summary: view.timing_summary,
        lanes: view
            .lanes
            .into_iter()
            .map(map_api_ci_lane_from_view)
            .collect(),
    }
}

fn map_api_ci_lane(lane: BranchCiLaneRecord, now: DateTime<Utc>) -> CiLane {
    map_api_ci_lane_from_view(map_ci_lane_view(lane, now))
}

fn map_api_ci_lane_from_view(view: CiLaneView) -> CiLane {
    CiLane {
        id: view.id,
        lane_id: view.lane_id,
        title: view.title,
        entrypoint: view.entrypoint,
        status: ForgeCiLaneStatus::from(view.status),
        status_tone: Some(view.status_tone),
        status_badge_class: None,
        is_failed: None,
        execution_reason: ForgeCiLaneExecutionReason::from(view.execution_reason),
        execution_reason_label: Some(view.execution_reason_label),
        failure_kind: view.failure_kind.map(ForgeCiLaneFailureKind::from),
        failure_kind_label: view.failure_kind_label,
        ci_run_id: view.ci_run_id,
        ci_target_id: view.ci_target_id,
        ci_target_key: view.ci_target_key,
        log_text: view.log_text,
        retry_count: view.retry_count,
        rerun_of_lane_run_id: view.rerun_of_lane_run_id,
        created_at: view.created_at,
        started_at: view.started_at,
        finished_at: view.finished_at,
        timing_summary: view.timing_summary,
        last_heartbeat_at: view.last_heartbeat_at,
        lease_expires_at: view.lease_expires_at,
        operator_hint: view.operator_hint,
    }
}

fn map_api_nightly_lane(lane: NightlyLaneRecord) -> CiLane {
    let view = map_nightly_lane_view(lane);
    CiLane {
        id: view.id,
        lane_id: view.lane_id,
        title: view.title,
        entrypoint: view.entrypoint,
        status: ForgeCiLaneStatus::from(view.status),
        status_tone: None,
        status_badge_class: Some(view.status_badge_class),
        is_failed: Some(view.is_failed),
        execution_reason: ForgeCiLaneExecutionReason::from(view.execution_reason),
        execution_reason_label: Some(view.execution_reason_label),
        failure_kind: view.failure_kind.map(ForgeCiLaneFailureKind::from),
        failure_kind_label: view.failure_kind_label,
        ci_run_id: view.ci_run_id,
        ci_target_id: view.ci_target_id,
        ci_target_key: view.ci_target_key,
        log_text: view.log_text,
        retry_count: view.retry_count,
        rerun_of_lane_run_id: view.rerun_of_lane_run_id,
        created_at: view.created_at,
        started_at: view.started_at,
        finished_at: view.finished_at,
        timing_summary: None,
        last_heartbeat_at: view.last_heartbeat_at,
        lease_expires_at: view.lease_expires_at,
        operator_hint: view.operator_hint,
    }
}

fn select_branch_log_lane(
    ci_runs: &[BranchCiRunRecord],
    lane_id: Option<&str>,
    lane_run_id: Option<i64>,
) -> Option<(i64, BranchCiLaneRecord)> {
    if let Some(lane_run_id) = lane_run_id {
        return ci_runs.iter().find_map(|run| {
            run.lanes
                .iter()
                .find(|lane| lane.id == lane_run_id)
                .cloned()
                .map(|lane| (run.id, lane))
        });
    }
    if let Some(lane_id) = lane_id {
        return ci_runs.iter().find_map(|run| {
            run.lanes
                .iter()
                .find(|lane| lane.lane_id == lane_id)
                .cloned()
                .map(|lane| (run.id, lane))
        });
    }
    ci_runs
        .iter()
        .find_map(|run| {
            run.lanes
                .iter()
                .find(|lane| lane.status == CiLaneStatus::Failed)
                .cloned()
                .map(|lane| (run.id, lane))
        })
        .or_else(|| {
            ci_runs.iter().find_map(|run| {
                run.lanes
                    .iter()
                    .find(|lane| {
                        lane.log_text
                            .as_ref()
                            .is_some_and(|text| !text.trim().is_empty())
                    })
                    .cloned()
                    .map(|lane| (run.id, lane))
            })
        })
        .or_else(|| {
            ci_runs
                .first()
                .and_then(|run| run.lanes.first().cloned().map(|lane| (run.id, lane)))
        })
}

fn json_error_response(status: StatusCode, message: &str) -> axum::response::Response {
    (status, Json(serde_json::json!({"error": message}))).into_response()
}

fn not_found_json_response(message: &str) -> axum::response::Response {
    json_error_response(StatusCode::NOT_FOUND, message)
}

struct LaneMutationEnvelope {
    branch_id: Option<i64>,
    nightly_run_id: Option<i64>,
    lane_run_id: i64,
    lane_status: ForgeCiLaneStatus,
}

struct RecoverRunEnvelope {
    branch_id: Option<i64>,
    run_id: Option<i64>,
    nightly_run_id: Option<i64>,
    recovered_lane_count: usize,
}

fn not_found_json(message: &str) -> axum::response::Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": message})),
    )
        .into_response()
}

fn lane_mutation_json_response(
    result: Result<Option<LaneMutationEnvelope>, ForgeServiceError>,
    missing_message: &str,
) -> axum::response::Response {
    match result {
        Ok(Some(payload)) => Json(LaneMutationResponse {
            status: "ok".to_string(),
            branch_id: payload.branch_id,
            nightly_run_id: payload.nightly_run_id,
            lane_run_id: payload.lane_run_id,
            lane_status: payload.lane_status,
        })
        .into_response(),
        Ok(None) => not_found_json(missing_message),
        Err(err) => forge_service_json_error(err),
    }
}

fn recover_run_json_response(
    result: Result<Option<RecoverRunEnvelope>, ForgeServiceError>,
    missing_message: &str,
) -> axum::response::Response {
    match result {
        Ok(Some(payload)) => Json(RecoverRunResponse {
            status: "ok".to_string(),
            branch_id: payload.branch_id,
            run_id: payload.run_id,
            nightly_run_id: payload.nightly_run_id,
            recovered_lane_count: payload.recovered_lane_count,
        })
        .into_response(),
        Ok(None) => not_found_json(missing_message),
        Err(err) => forge_service_json_error(err),
    }
}

async fn merge_handler(
    State(state): State<Arc<AppState>>,
    Path(branch_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_trusted_auth(&state.auth, &headers) {
        Ok(npub) => npub,
        Err(resp) => return resp,
    };
    match state.forge_service.merge_branch(branch_id, &npub).await {
        Ok(MergeBranchResult {
            branch_id,
            merge_commit_sha,
        }) => Json(BranchActionResponse {
            status: "ok".to_string(),
            branch_id,
            merge_commit_sha: Some(merge_commit_sha),
            deleted: None,
        })
        .into_response(),
        Err(err) => forge_service_json_error(err),
    }
}

async fn close_handler(
    State(state): State<Arc<AppState>>,
    Path(branch_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_trusted_auth(&state.auth, &headers) {
        Ok(npub) => npub,
        Err(resp) => return resp,
    };
    match state.forge_service.close_branch(branch_id, &npub).await {
        Ok(CloseBranchResult { branch_id, deleted }) => Json(BranchActionResponse {
            status: "ok".to_string(),
            branch_id,
            merge_commit_sha: None,
            deleted: Some(deleted),
        })
        .into_response(),
        Err(err) => forge_service_json_error(err),
    }
}

async fn rerun_branch_ci_lane_handler(
    State(state): State<Arc<AppState>>,
    Path((branch_id, lane_run_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_trusted_auth(&state.auth, &headers) {
        return resp;
    }
    match state
        .forge_service
        .rerun_branch_ci_lane(branch_id, lane_run_id)
        .await
    {
        Ok(Some(BranchLaneRerunResult {
            branch_id,
            rerun_suite_id,
        })) => Json(serde_json::json!({
            "status": "ok",
            "branch_id": branch_id,
            "rerun_suite_id": rerun_suite_id
        }))
        .into_response(),
        Ok(None) => not_found_json_response("branch lane not found"),
        Err(err) => forge_service_json_error(err),
    }
}

async fn rerun_nightly_lane_handler(
    State(state): State<Arc<AppState>>,
    Path((nightly_run_id, lane_run_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_trusted_auth(&state.auth, &headers) {
        return resp;
    }
    match state
        .forge_service
        .rerun_nightly_lane(nightly_run_id, lane_run_id)
        .await
    {
        Ok(Some(NightlyLaneRerunResult {
            nightly_run_id,
            rerun_run_id,
        })) => Json(serde_json::json!({
            "status": "ok",
            "nightly_run_id": nightly_run_id,
            "rerun_run_id": rerun_run_id
        }))
        .into_response(),
        Ok(None) => not_found_json_response("nightly lane not found"),
        Err(err) => forge_service_json_error(err),
    }
}

async fn fail_branch_ci_lane_handler(
    State(state): State<Arc<AppState>>,
    Path((branch_id, lane_run_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_trusted_auth(&state.auth, &headers) {
        Ok(npub) => npub,
        Err(resp) => return resp,
    };
    lane_mutation_json_response(
        state
            .forge_service
            .fail_branch_ci_lane(branch_id, lane_run_id, &npub)
            .await
            .map(|result| {
                result.map(|BranchLaneMutationResult {
                    branch_id,
                    lane_run_id,
                    lane_status,
                }| LaneMutationEnvelope {
                    branch_id: Some(branch_id),
                    nightly_run_id: None,
                    lane_run_id,
                    lane_status: ForgeCiLaneStatus::from(lane_status.as_str()),
                })
            }),
        "branch lane not found",
    )
}

async fn requeue_branch_ci_lane_handler(
    State(state): State<Arc<AppState>>,
    Path((branch_id, lane_run_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_trusted_auth(&state.auth, &headers) {
        return resp;
    }
    lane_mutation_json_response(
        state
            .forge_service
            .requeue_branch_ci_lane(branch_id, lane_run_id)
            .await
            .map(|result| {
                result.map(|BranchLaneMutationResult {
                    branch_id,
                    lane_run_id,
                    lane_status,
                }| LaneMutationEnvelope {
                    branch_id: Some(branch_id),
                    nightly_run_id: None,
                    lane_run_id,
                    lane_status: ForgeCiLaneStatus::from(lane_status.as_str()),
                })
            }),
        "branch lane not found",
    )
}

async fn recover_branch_ci_run_handler(
    State(state): State<Arc<AppState>>,
    Path((branch_id, run_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_trusted_auth(&state.auth, &headers) {
        return resp;
    }
    recover_run_json_response(
        state
            .forge_service
            .recover_branch_ci_run(branch_id, run_id)
            .await
            .map(|result| {
                result.map(|BranchRunRecoveryResult {
                    branch_id,
                    run_id,
                    recovered_lane_count,
                }| RecoverRunEnvelope {
                    branch_id: Some(branch_id),
                    run_id: Some(run_id),
                    nightly_run_id: None,
                    recovered_lane_count,
                })
            }),
        "branch run not found",
    )
}

async fn fail_nightly_lane_handler(
    State(state): State<Arc<AppState>>,
    Path((nightly_run_id, lane_run_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_trusted_auth(&state.auth, &headers) {
        Ok(npub) => npub,
        Err(resp) => return resp,
    };
    lane_mutation_json_response(
        state
            .forge_service
            .fail_nightly_lane(nightly_run_id, lane_run_id, &npub)
            .await
            .map(|result| {
                result.map(|NightlyLaneMutationResult {
                    nightly_run_id,
                    lane_run_id,
                    lane_status,
                }| LaneMutationEnvelope {
                    branch_id: None,
                    nightly_run_id: Some(nightly_run_id),
                    lane_run_id,
                    lane_status: ForgeCiLaneStatus::from(lane_status.as_str()),
                })
            }),
        "nightly lane not found",
    )
}

async fn requeue_nightly_lane_handler(
    State(state): State<Arc<AppState>>,
    Path((nightly_run_id, lane_run_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_trusted_auth(&state.auth, &headers) {
        return resp;
    }
    lane_mutation_json_response(
        state
            .forge_service
            .requeue_nightly_lane(nightly_run_id, lane_run_id)
            .await
            .map(|result| {
                result.map(|NightlyLaneMutationResult {
                    nightly_run_id,
                    lane_run_id,
                    lane_status,
                }| LaneMutationEnvelope {
                    branch_id: None,
                    nightly_run_id: Some(nightly_run_id),
                    lane_run_id,
                    lane_status: ForgeCiLaneStatus::from(lane_status.as_str()),
                })
            }),
        "nightly lane not found",
    )
}

async fn recover_nightly_run_handler(
    State(state): State<Arc<AppState>>,
    Path(nightly_run_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_trusted_auth(&state.auth, &headers) {
        return resp;
    }
    recover_run_json_response(
        state
            .forge_service
            .recover_nightly_run(nightly_run_id)
            .await
            .map(|result| {
                result.map(|NightlyRunRecoveryResult {
                    nightly_run_id,
                    recovered_lane_count,
                }| RecoverRunEnvelope {
                    branch_id: None,
                    run_id: None,
                    nightly_run_id: Some(nightly_run_id),
                    recovered_lane_count,
                })
            }),
        "nightly run not found",
    )
}

async fn wake_ci_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_trusted_auth(&state.auth, &headers) {
        return resp;
    }
    state.forge_service.wake_ci();
    Json(WakeCiResponse {
        status: "ok".to_string(),
        message: "scheduler wake requested".to_string(),
    })
    .into_response()
}

async fn api_forge_branch_resolve_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ForgeBranchResolveQuery>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_auth(&state.auth, &headers) {
        return resp;
    }
    let branch_name = query.branch_name.trim().to_string();
    if branch_name.is_empty() {
        return json_error_response(StatusCode::BAD_REQUEST, "branch_name is required");
    }
    match state
        .forge_service
        .resolve_branch_by_name(&branch_name)
        .await
    {
        Ok(Some(branch)) => Json(ForgeBranchResolveResponse {
            branch_id: branch.branch_id,
            repo: branch.repo,
            branch_name: branch.branch_name,
            branch_state: branch.branch_state,
        })
        .into_response(),
        Ok(None) => not_found_json_response("branch not found"),
        Err(err) => forge_service_json_error(err),
    }
}

async fn api_forge_branch_detail_handler(
    State(state): State<Arc<AppState>>,
    Path(branch_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_auth(&state.auth, &headers) {
        return resp;
    }
    match state
        .forge_service
        .branch_detail_and_runs(branch_id, 8)
        .await
    {
        Ok(Some(BranchDetailAndRuns { detail, ci_runs })) => {
            let now = Utc::now();
            Json(ForgeBranchDetailResponse {
                branch: map_forge_branch_summary(detail),
                ci_runs: ci_runs
                    .into_iter()
                    .map(|run| map_api_ci_run(run, now))
                    .collect(),
            })
        }
        .into_response(),
        Ok(None) => not_found_json_response("branch not found"),
        Err(err) => forge_service_json_error(err),
    }
}

async fn api_forge_branch_logs_handler(
    State(state): State<Arc<AppState>>,
    Path(branch_id): Path<i64>,
    Query(query): Query<ForgeBranchLogsQuery>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_auth(&state.auth, &headers) {
        return resp;
    }
    match state
        .forge_service
        .branch_detail_and_runs(branch_id, 8)
        .await
    {
        Ok(Some(BranchDetailAndRuns { detail, ci_runs })) => {
            let Some((run_id, lane)) =
                select_branch_log_lane(&ci_runs, query.lane.as_deref(), query.lane_run_id)
            else {
                return not_found_json_response("no matching lane logs found");
            };
            let bundle = lane.ci_run_id.as_deref().and_then(|ci_run_id| {
                state
                    .jerichoci_run_store
                    .as_ref()
                    .and_then(|store| store.load_run_bundle(ci_run_id).ok())
            });
            let (ci_run, ci_log_metadata, ci_prepared_outputs) = match bundle {
                Some(bundle) => (Some(bundle.run), Some(bundle.logs), bundle.prepared_outputs),
                None => (None, None, None),
            };
            let now = Utc::now();
            Json(ForgeBranchLogsResponse {
                branch_id: detail.branch_id,
                branch_name: detail.branch_name,
                run_id,
                lane: map_api_ci_lane(lane, now),
                ci_run,
                ci_log_metadata,
                ci_prepared_outputs,
            })
            .into_response()
        }
        Ok(None) => not_found_json_response("branch not found"),
        Err(err) => forge_service_json_error(err),
    }
}

async fn api_forge_ci_run_handler(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_auth(&state.auth, &headers) {
        return resp;
    }
    match require_jerichoci_run_store(state.jerichoci_run_store.as_ref())
        .and_then(|store| store.load_run(&run_id))
    {
        Ok(run) => Json(run).into_response(),
        Err(err) => json_error_response(StatusCode::NOT_FOUND, &err.to_string()),
    }
}

async fn api_forge_ci_logs_handler(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    Query(query): Query<ForgeCiLogsQuery>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_auth(&state.auth, &headers) {
        return resp;
    }
    match require_jerichoci_run_store(state.jerichoci_run_store.as_ref()).and_then(|store| {
        store.load_logs(
            &run_id,
            query.job.as_deref(),
            map_forge_ci_log_kind(query.kind),
        )
    }) {
        Ok(logs) => Json(ForgeCiLogsResponse {
            run_id,
            job: query.job,
            host: logs.host,
            guest: logs.guest,
        })
        .into_response(),
        Err(err) => json_error_response(StatusCode::NOT_FOUND, &err.to_string()),
    }
}

async fn api_forge_ci_prepared_outputs_handler(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_auth(&state.auth, &headers) {
        return resp;
    }
    match require_jerichoci_run_store(state.jerichoci_run_store.as_ref())
        .and_then(|store| store.load_run_bundle(&run_id))
    {
        Ok(bundle) => match bundle.prepared_outputs {
            Some(prepared_outputs) => Json(ForgeCiPreparedOutputsResponse {
                run_id,
                prepared_outputs,
            })
            .into_response(),
            None => json_error_response(
                StatusCode::NOT_FOUND,
                &format!("prepared outputs not found for run `{run_id}`"),
            ),
        },
        Err(err) => json_error_response(StatusCode::NOT_FOUND, &err.to_string()),
    }
}

fn map_forge_ci_log_kind(kind: ForgeCiLogKind) -> LogKind {
    match kind {
        ForgeCiLogKind::Host => LogKind::Host,
        ForgeCiLogKind::Guest => LogKind::Guest,
        ForgeCiLogKind::Both => LogKind::Both,
    }
}

async fn api_forge_nightly_detail_handler(
    State(state): State<Arc<AppState>>,
    Path(nightly_run_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = require_auth(&state.auth, &headers) {
        return resp;
    }
    match state.forge_service.nightly_run(nightly_run_id).await {
        Ok(Some(run)) => {
            Json(ForgeNightlyDetailResponse {
                nightly_run_id: run.nightly_run_id,
                repo: run.repo,
                scheduled_for: run.scheduled_for,
                created_at: run.created_at,
                source_ref: run.source_ref,
                source_head_sha: run.source_head_sha,
                status: run.status,
                summary: run.summary,
                rerun_of_run_id: run.rerun_of_run_id,
                started_at: run.started_at,
                finished_at: run.finished_at,
                lanes: run.lanes.into_iter().map(map_api_nightly_lane).collect(),
            })
        }
        .into_response(),
        Ok(None) => not_found_json_response("nightly run not found"),
        Err(err) => forge_service_json_error(err),
    }
}

// --- Auth handlers ---
