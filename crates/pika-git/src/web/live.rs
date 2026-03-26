struct LiveSnapshot {
    html: String,
    active: bool,
}

async fn load_branch_ci_summary_snapshot(
    state: Arc<AppState>,
    branch_id: i64,
    review_mode: bool,
) -> Result<Option<LiveSnapshot>, (StatusCode, String)> {
    let BranchDetailAndRuns { detail, ci_runs } = match state
        .forge_service
        .branch_detail_and_runs(branch_id, 8)
        .await
    {
        Ok(Some(result)) => result,
        Ok(None) => return Ok(None),
        Err(err) => return Err(map_forge_service_error(err)),
    };
    let html =
        render_branch_ci_summary_html(&detail, &ci_runs, &branch_page_notices(&state), review_mode)
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let active = branch_ci_runs_are_active(&ci_runs);
    Ok(Some(LiveSnapshot { html, active }))
}

async fn load_branch_ci_live_snapshot(
    state: Arc<AppState>,
    branch_id: i64,
) -> Result<Option<LiveSnapshot>, (StatusCode, String)> {
    let BranchDetailAndRuns { detail, ci_runs } = match state
        .forge_service
        .branch_detail_and_runs(branch_id, 8)
        .await
    {
        Ok(Some(result)) => result,
        Ok(None) => return Ok(None),
        Err(err) => return Err(map_forge_service_error(err)),
    };
    let html = render_branch_ci_live_html(&detail, &ci_runs, &branch_page_notices(&state))
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let active = branch_ci_runs_are_active(&ci_runs);
    Ok(Some(LiveSnapshot { html, active }))
}

async fn load_nightly_live_snapshot(
    state: Arc<AppState>,
    nightly_run_id: i64,
) -> Result<Option<LiveSnapshot>, (StatusCode, String)> {
    let nightly = match state.forge_service.nightly_run(nightly_run_id).await {
        Ok(Some(run)) => run,
        Ok(None) => return Ok(None),
        Err(err) => return Err(map_forge_service_error(err)),
    };
    let html = render_nightly_live_html(&nightly, &nightly_page_notices(&state))
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    let active = nightly_run_is_active(&nightly);
    Ok(Some(LiveSnapshot { html, active }))
}

fn live_html_event(html: String) -> Result<Event, Infallible> {
    let payload = serde_json::to_string(&LiveHtmlPayload { html }).unwrap_or_else(|_| {
        serde_json::json!({"html": "<p class=\"muted\">Failed to encode live update.</p>"})
            .to_string()
    });
    Ok(Event::default().event("ci-update").data(payload))
}

fn branch_live_update_error_html(status: StatusCode, message: &str) -> String {
    format!(
        "<section class=\"panel\"><h2>CI</h2><p class=\"muted\">Live update failed: {} {}</p></section>",
        status.as_u16(),
        message
    )
}

fn nightly_live_update_error_html(status: StatusCode, message: &str) -> String {
    format!(
        "<section class=\"panel\"><h2>Lanes</h2><p class=\"muted\">Live update failed: {} {}</p></section>",
        status.as_u16(),
        message
    )
}

fn live_update_error_snapshot(
    panel_title: &str,
    status: StatusCode,
    message: &str,
) -> LiveSnapshot {
    let html = match panel_title {
        "CI" => branch_live_update_error_html(status, message),
        _ => nightly_live_update_error_html(status, message),
    };
    LiveSnapshot { html, active: false }
}

async fn next_branch_ci_summary_snapshot(
    receiver: &mut tokio::sync::broadcast::Receiver<CiLiveUpdate>,
    state: Arc<AppState>,
    branch_id: i64,
    review_mode: bool,
) -> Option<LiveSnapshot> {
    loop {
        match receiver.recv().await {
            Ok(CiLiveUpdate::BranchChanged {
                branch_id: updated_branch_id,
                ..
            }) if updated_branch_id == branch_id => {
                return match load_branch_ci_summary_snapshot(
                    Arc::clone(&state),
                    branch_id,
                    review_mode,
                )
                .await
                {
                    Ok(Some(snapshot)) => Some(snapshot),
                    Ok(None) => None,
                    Err((status, message)) => Some(live_update_error_snapshot("CI", status, &message)),
                };
            }
            Ok(_) => continue,
            Err(RecvError::Lagged(_)) => {
                return match load_branch_ci_summary_snapshot(
                    Arc::clone(&state),
                    branch_id,
                    review_mode,
                )
                .await
                {
                    Ok(Some(snapshot)) => Some(snapshot),
                    Ok(None) => None,
                    Err((status, message)) => Some(live_update_error_snapshot("CI", status, &message)),
                };
            }
            Err(RecvError::Closed) => return None,
        }
    }
}

async fn next_branch_ci_live_snapshot(
    receiver: &mut tokio::sync::broadcast::Receiver<CiLiveUpdate>,
    state: Arc<AppState>,
    branch_id: i64,
) -> Option<LiveSnapshot> {
    loop {
        match receiver.recv().await {
            Ok(CiLiveUpdate::BranchChanged {
                branch_id: updated_branch_id,
                ..
            }) if updated_branch_id == branch_id => {
                return match load_branch_ci_live_snapshot(Arc::clone(&state), branch_id).await {
                    Ok(Some(snapshot)) => Some(snapshot),
                    Ok(None) => None,
                    Err((status, message)) => Some(live_update_error_snapshot("CI", status, &message)),
                };
            }
            Ok(_) => continue,
            Err(RecvError::Lagged(_)) => {
                return match load_branch_ci_live_snapshot(Arc::clone(&state), branch_id).await {
                    Ok(Some(snapshot)) => Some(snapshot),
                    Ok(None) => None,
                    Err((status, message)) => Some(live_update_error_snapshot("CI", status, &message)),
                };
            }
            Err(RecvError::Closed) => return None,
        }
    }
}

async fn next_nightly_live_snapshot(
    receiver: &mut tokio::sync::broadcast::Receiver<CiLiveUpdate>,
    state: Arc<AppState>,
    nightly_run_id: i64,
) -> Option<LiveSnapshot> {
    loop {
        match receiver.recv().await {
            Ok(CiLiveUpdate::NightlyChanged {
                nightly_run_id: updated_nightly_run_id,
                ..
            }) if updated_nightly_run_id == nightly_run_id => {
                return match load_nightly_live_snapshot(Arc::clone(&state), nightly_run_id).await {
                    Ok(Some(snapshot)) => Some(snapshot),
                    Ok(None) => None,
                    Err((status, message)) => Some(live_update_error_snapshot("Lanes", status, &message)),
                };
            }
            Ok(_) => continue,
            Err(RecvError::Lagged(_)) => {
                return match load_nightly_live_snapshot(Arc::clone(&state), nightly_run_id).await {
                    Ok(Some(snapshot)) => Some(snapshot),
                    Ok(None) => None,
                    Err((status, message)) => Some(live_update_error_snapshot("Lanes", status, &message)),
                };
            }
            Err(RecvError::Closed) => return None,
        }
    }
}

async fn branch_ci_stream_handler(
    State(state): State<Arc<AppState>>,
    Path(branch_id): Path<i64>,
    Query(query): Query<ReviewModeQuery>,
) -> impl IntoResponse {
    let review_mode = query.review;
    let initial =
        match load_branch_ci_summary_snapshot(Arc::clone(&state), branch_id, review_mode).await {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    format!("branch {} not found", branch_id),
                )
                    .into_response();
            }
            Err((status, message)) => return (status, message).into_response(),
        };
    let receiver = state.live_updates.subscribe();
    let stream = stream::unfold(
        Some((Some(initial), receiver, state, branch_id)),
        move |state| async move {
            let (pending, mut receiver, state, branch_id) = match state {
                Some(state) => state,
                None => return None,
            };
            if let Some(snapshot) = pending {
                let next_state = if snapshot.active {
                    Some((None, receiver, state, branch_id))
                } else {
                    None
                };
                return Some((live_html_event(snapshot.html), next_state));
            }
            let snapshot = match next_branch_ci_summary_snapshot(
                &mut receiver,
                Arc::clone(&state),
                branch_id,
                review_mode,
            )
            .await
            {
                Some(snapshot) => snapshot,
                None => return None,
            };
            let next_state = if snapshot.active {
                Some((None, receiver, state, branch_id))
            } else {
                None
            };
            Some((live_html_event(snapshot.html), next_state))
        },
    );
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

async fn branch_ci_full_stream_handler(
    State(state): State<Arc<AppState>>,
    Path(branch_id): Path<i64>,
) -> impl IntoResponse {
    let initial = match load_branch_ci_live_snapshot(Arc::clone(&state), branch_id).await {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                format!("branch {} not found", branch_id),
            )
                .into_response();
        }
        Err((status, message)) => return (status, message).into_response(),
    };
    let receiver = state.live_updates.subscribe();
    let stream = stream::unfold(
        Some((Some(initial), receiver, state, branch_id)),
        |state| async move {
            let (pending, mut receiver, state, branch_id) = match state {
                Some(state) => state,
                None => return None,
            };
            if let Some(snapshot) = pending {
                let next_state = if snapshot.active {
                    Some((None, receiver, state, branch_id))
                } else {
                    None
                };
                return Some((live_html_event(snapshot.html), next_state));
            }
            let snapshot =
                match next_branch_ci_live_snapshot(&mut receiver, Arc::clone(&state), branch_id)
                    .await
                {
                    Some(snapshot) => snapshot,
                    None => return None,
                };
            let next_state = if snapshot.active {
                Some((None, receiver, state, branch_id))
            } else {
                None
            };
            Some((live_html_event(snapshot.html), next_state))
        },
    );
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

async fn nightly_stream_handler(
    State(state): State<Arc<AppState>>,
    Path(nightly_run_id): Path<i64>,
) -> impl IntoResponse {
    let initial = match load_nightly_live_snapshot(Arc::clone(&state), nightly_run_id).await {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                format!("nightly run {} not found", nightly_run_id),
            )
                .into_response();
        }
        Err((status, message)) => return (status, message).into_response(),
    };
    let receiver = state.live_updates.subscribe();
    let stream = stream::unfold(
        Some((Some(initial), receiver, state, nightly_run_id)),
        |state| async move {
            let (pending, mut receiver, state, nightly_run_id) = match state {
                Some(state) => state,
                None => return None,
            };
            if let Some(snapshot) = pending {
                let next_state = if snapshot.active {
                    Some((None, receiver, state, nightly_run_id))
                } else {
                    None
                };
                return Some((live_html_event(snapshot.html), next_state));
            }
            let snapshot =
                match next_nightly_live_snapshot(&mut receiver, Arc::clone(&state), nightly_run_id)
                    .await
                {
                    Some(snapshot) => snapshot,
                    None => return None,
                };
            let next_state = if snapshot.active {
                Some((None, receiver, state, nightly_run_id))
            } else {
                None
            };
            Some((live_html_event(snapshot.html), next_state))
        },
    );
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}
