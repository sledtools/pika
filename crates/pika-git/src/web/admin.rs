async fn inbox_handler(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let template = InboxTemplate {};
    match template.render() {
        Ok(rendered) => Html(rendered).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to render inbox template: {}", err),
        )
            .into_response(),
    }
}

async fn admin_handler(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let template = AdminTemplate {};
    match template.render() {
        Ok(rendered) => Html(rendered).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to render admin template: {}", err),
        )
            .into_response(),
    }
}

#[allow(clippy::result_large_err)]
#[derive(serde::Deserialize)]
struct AdminAllowlistUpsertRequest {
    npub: String,
    note: Option<String>,
    active: bool,
    #[serde(default)]
    can_forge_write: bool,
}

async fn api_admin_allowlist_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let _admin_npub = match require_admin_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };

    let store = state.store.clone();
    let bootstrap_admin_npubs = state.auth.bootstrap_admin_npubs();
    match tokio::task::spawn_blocking(move || {
        let entries = store.list_chat_allowlist_entries()?;
        Ok::<_, anyhow::Error>((entries, bootstrap_admin_npubs))
    })
    .await
    {
        Ok(Ok((entries, bootstrap_admin_npubs))) => Json(serde_json::json!({
            "bootstrap_admin_npubs": bootstrap_admin_npubs,
            "entries": entries,
        }))
        .into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn api_admin_forge_status_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let _admin_npub = match require_admin_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };

    let store = state.store.clone();
    let mirror_config = state.config.clone();
    let health_config = state.config.clone();
    match tokio::task::spawn_blocking(move || mirror::get_mirror_status(&store, &mirror_config))
        .await
    {
        Ok(Ok(mirror_admin)) => {
            let mirror_status = mirror_admin.detail.as_ref().map(|(status, _)| status);
            let forge_health = state
                .forge_runtime
                .health_snapshot(&health_config, mirror_status);
            let mirror_runtime = mirror_admin.runtime;
            match mirror_admin.detail {
                Some((mirror_status, mirror_history)) => Json(serde_json::json!({
                    "forge_health": forge_health,
                    "mirror_runtime": mirror_runtime,
                    "mirror_status": mirror_status,
                    "mirror_history": mirror_history,
                }))
                .into_response(),
                None => Json(serde_json::json!({
                    "forge_health": forge_health,
                    "mirror_runtime": mirror_runtime,
                    "mirror_status": null,
                    "mirror_history": [],
                }))
                .into_response(),
            }
        }
        Ok(Err(err)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": err.to_string()})),
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": err.to_string()})),
        )
            .into_response(),
    }
}

async fn api_admin_mirror_sync_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let _admin_npub = match require_admin_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };

    let store = state.store.clone();
    let config = state.config.clone();
    match state.forge_runtime.run_manual_mirror_pass(store, config).await {
        Ok(ManualMirrorPassStatus::Attempted(result)) => {
            Json(serde_json::json!({
                "attempted": result.attempted,
                "status": result.status,
                "lagging_ref_count": result.lagging_ref_count,
            }))
            .into_response()
        }
        Ok(ManualMirrorPassStatus::AlreadyRunning) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "mirror sync already running"
            })),
        )
            .into_response(),
        Ok(ManualMirrorPassStatus::Unavailable) => {
            (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "mirror sync is unavailable; configure forge_repo.mirror_remote to enable mirroring"
                })),
            )
                .into_response()
        }
        Err(err) => {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": err.to_string()})),
            )
                .into_response()
        }
    }
}

async fn api_admin_allowlist_upsert_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<AdminAllowlistUpsertRequest>,
) -> impl IntoResponse {
    let admin_npub = match require_admin_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };

    let npub = match normalize_npub(&body.npub) {
        Ok(value) => value,
        Err(msg) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": msg})),
            )
                .into_response();
        }
    };

    if state.auth.is_config_managed_chat_principal(&npub) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "This pubkey is managed by config and cannot be changed from the admin page"
            })),
        )
            .into_response();
    }

    let note = body
        .note
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let active = body.active;
    let can_forge_write = body.can_forge_write;
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || {
        let existing = store.get_chat_allowlist_entry(&npub)?;
        let entry = store.upsert_chat_allowlist_entry(
            &npub,
            active,
            can_forge_write,
            note.as_deref(),
            &admin_npub,
        )?;
        let backfilled = if should_backfill_managed_allowlist_entry(
            existing.as_ref(),
            active,
            can_forge_write,
        ) {
            store.backfill_branch_inbox_for_npub(&npub)?
        } else {
            0
        };
        Ok::<_, anyhow::Error>((entry, backfilled))
    })
    .await
    {
        Ok(Ok((entry, backfilled))) => Json(serde_json::json!({
            "entry": entry,
            "backfilled": backfilled,
        }))
        .into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

fn should_backfill_managed_allowlist_entry(
    existing: Option<&ChatAllowlistEntry>,
    active: bool,
    can_forge_write: bool,
) -> bool {
    let was_reviewable = existing
        .map(|entry| entry.active || entry.can_forge_write)
        .unwrap_or(false);
    let is_reviewable = active || can_forge_write;
    is_reviewable && !was_reviewable
}

#[derive(serde::Deserialize)]
struct InboxListParams {
    page: Option<i64>,
}

async fn api_inbox_list_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(params): Query<InboxListParams>,
) -> impl IntoResponse {
    let npub = match require_inbox_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let page = params.page.unwrap_or(1).max(1);
    let offset = (page - 1) * 50;
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || {
        let items = store.list_branch_inbox(&npub, 50, offset)?;
        let review_needed = store.branch_inbox_count(&npub)?;
        let total = store.branch_inbox_total(&npub)?;
        Ok::<_, anyhow::Error>((items, total, review_needed))
    })
    .await
    {
        Ok(Ok((items, total, review_needed))) => Json(serde_json::json!({
            "items": items,
            "total": total,
            "review_needed": review_needed,
            "page": page
        }))
        .into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn api_inbox_count_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_inbox_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.branch_inbox_count(&npub)).await {
        Ok(Ok(count)) => Json(serde_json::json!({"count": count})).into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
struct InboxDismissRequest {
    branch_ids: Option<Vec<i64>>,
    all: Option<bool>,
}

async fn api_inbox_dismiss_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<InboxDismissRequest>,
) -> impl IntoResponse {
    let npub = match require_inbox_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let store = state.store.clone();
    let dismissed = if body.all.unwrap_or(false) {
        tokio::task::spawn_blocking(move || store.dismiss_all_branch_inbox(&npub)).await
    } else {
        let review_ids = body.branch_ids.unwrap_or_default();
        tokio::task::spawn_blocking(move || store.dismiss_branch_inbox_items(&npub, &review_ids))
            .await
    };
    match dismissed {
        Ok(Ok(count)) => Json(serde_json::json!({"dismissed": count})).into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn api_inbox_mark_reviewed_handler(
    State(state): State<Arc<AppState>>,
    Path(review_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_inbox_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.mark_branch_inbox_reviewed(&npub, review_id))
        .await
    {
        Ok(Ok(count)) => Json(serde_json::json!({"marked": count})).into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn api_inbox_neighbors_handler(
    State(state): State<Arc<AppState>>,
    Path(review_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let npub = match require_inbox_auth(&state.auth, &headers) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.branch_inbox_review_context(&npub, review_id))
        .await
    {
        Ok(Ok(Some(InboxReviewContext {
            prev,
            next,
            position,
            total,
        }))) => Json(
            serde_json::json!({"prev": prev, "next": next, "position": position, "total": total}),
        )
        .into_response(),
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "inbox item not found"})),
        )
            .into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
