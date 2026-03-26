async fn feed_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let branch_store = state.store.clone();
    let nightly_store = state.store.clone();
    let items =
        match tokio::task::spawn_blocking(move || branch_store.list_branch_feed_items()).await {
            Ok(Ok(items)) => items,
            Ok(Err(err)) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to query feed items: {}", err),
                )
                    .into_response();
            }
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("feed worker task failed: {}", err),
                )
                    .into_response();
            }
        };
    let nightly_items =
        match tokio::task::spawn_blocking(move || nightly_store.list_recent_nightly_runs(12)).await
        {
            Ok(Ok(items)) => items,
            Ok(Err(err)) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to query nightly runs: {}", err),
                )
                    .into_response();
            }
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("nightly worker task failed: {}", err),
                )
                    .into_response();
            }
        };

    let mut open_items = Vec::new();
    let mut history_items = Vec::new();

    for item in items {
        let view = map_feed_item(item);
        if view.state == "open" {
            open_items.push(view);
        } else {
            history_items.push(view);
        }
    }

    let template = FeedTemplate {
        open_items,
        history_items,
        nightly_items: nightly_items
            .into_iter()
            .map(map_nightly_feed_item)
            .collect(),
    };

    match template.render() {
        Ok(rendered) => Html(rendered).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to render feed template: {}", err),
        )
            .into_response(),
    }
}

async fn nightly_handler(
    State(state): State<Arc<AppState>>,
    Path(nightly_run_id): Path<i64>,
) -> impl IntoResponse {
    let nightly = match state.forge_service.nightly_run(nightly_run_id).await {
        Ok(Some(run)) => run,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                format!("nightly run {} not found", nightly_run_id),
            )
                .into_response();
        }
        Err(err) => {
            let (status, message) = map_forge_service_error(err);
            return (status, message).into_response();
        }
    };
    let template = render_nightly_template_with_notices(nightly, nightly_page_notices(&state));
    match template.render() {
        Ok(rendered) => Html(rendered).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to render nightly template: {}", err),
        )
            .into_response(),
    }
}

async fn detail_handler(
    State(state): State<Arc<AppState>>,
    Path(branch_id): Path<i64>,
) -> impl IntoResponse {
    detail_page(state, branch_id, false).await
}

async fn branch_ci_page_handler(
    State(state): State<Arc<AppState>>,
    Path(branch_id): Path<i64>,
    Query(query): Query<ReviewModeQuery>,
) -> impl IntoResponse {
    let BranchDetailAndRuns { detail, ci_runs } = match state
        .forge_service
        .branch_detail_and_runs(branch_id, 8)
        .await
    {
        Ok(Some(result)) => result,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                format!("branch {} not found", branch_id),
            )
                .into_response();
        }
        Err(err) => {
            let (status, message) = map_forge_service_error(err);
            return (status, message).into_response();
        }
    };

    match render_branch_ci_template_with_notices(
        detail,
        ci_runs,
        branch_page_notices(&state),
        query.review,
    ) {
        Ok(template) => match template.render() {
            Ok(rendered) => Html(rendered).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to render branch ci template: {}", err),
            )
                .into_response(),
        },
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to build branch ci view: {}", err),
        )
            .into_response(),
    }
}

async fn inbox_review_handler(
    State(state): State<Arc<AppState>>,
    Path(review_id): Path<i64>,
) -> impl IntoResponse {
    detail_page(state, review_id, true).await
}

async fn detail_page(
    state: Arc<AppState>,
    branch_id: i64,
    review_mode: bool,
) -> axum::response::Response {
    let BranchDetailAndRuns { detail, ci_runs } = match state
        .forge_service
        .branch_detail_and_runs(branch_id, 8)
        .await
    {
        Ok(Some(result)) => result,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                format!("branch {} not found", branch_id),
            )
                .into_response();
        }
        Err(err) => {
            let (status, message) = map_forge_service_error(err);
            return (status, message).into_response();
        }
    };

    match render_detail_template_with_notices(
        detail,
        ci_runs,
        review_mode,
        branch_page_notices(&state),
    ) {
        Ok(template) => match template.render() {
            Ok(rendered) => Html(rendered).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to render detail template: {}", err),
            )
                .into_response(),
        },
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to build detail view: {}", err),
        )
            .into_response(),
    }
}
