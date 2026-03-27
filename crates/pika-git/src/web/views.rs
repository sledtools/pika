fn map_feed_item(item: BranchFeedItem) -> FeedItemView {
    FeedItemView {
        branch_id: item.branch_id,
        repo: item.repo,
        branch_name: item.branch_name,
        title: item.title,
        state: item.state.as_str().to_string(),
        updated_at: item.updated_at,
        tutorial_status: item.tutorial_status.as_str().to_string(),
        ci_status: item.ci_status.as_str().to_string(),
    }
}

fn map_nightly_feed_item(item: NightlyFeedItem) -> NightlyFeedItemView {
    NightlyFeedItemView {
        nightly_run_id: item.nightly_run_id,
        repo: item.repo,
        source_head_sha: item.source_head_sha,
        status: item.status.as_str().to_string(),
        summary: item.summary,
        scheduled_for: item.scheduled_for,
        created_at: item.created_at,
    }
}

#[cfg(test)]
fn render_detail_template(
    record: BranchDetailRecord,
    ci_runs: Vec<BranchCiRunRecord>,
    review_mode: bool,
) -> anyhow::Result<DetailTemplate> {
    render_detail_template_with_notices(record, ci_runs, review_mode, Vec::new())
}

fn render_detail_template_with_notices(
    record: BranchDetailRecord,
    ci_runs: Vec<BranchCiRunRecord>,
    review_mode: bool,
    page_notices: Vec<PageNoticeView>,
) -> anyhow::Result<DetailTemplate> {
    let mut steps = Vec::new();
    let mut executive_html = None;
    let mut media_links = Vec::new();
    if let Some(tutorial_json) = &record.tutorial_json {
        let tutorial: TutorialDoc = serde_json::from_str(tutorial_json)
            .context("parse stored tutorial JSON for detail page")?;

        executive_html = Some(markdown_to_safe_html(&tutorial.executive_summary));
        media_links = tutorial
            .media_links
            .into_iter()
            .map(|link| MediaLinkView {
                href: if is_safe_http_url(&link) {
                    link.clone()
                } else {
                    "#".to_string()
                },
                label: link,
            })
            .collect();
        for step in tutorial.steps {
            steps.push(StepView {
                title: step.title,
                intent: step.intent,
                affected_files: step.affected_files.join(", "),
                evidence_snippets: step.evidence_snippets,
                body_html: markdown_to_safe_html(&step.body_markdown),
            });
        }
    }

    let branch_ci_summary_html =
        render_branch_ci_summary_html(&record, &ci_runs, &page_notices, review_mode)?;
    let branch_ci_summary_enabled = branch_ci_runs_are_active(&ci_runs);

    Ok(DetailTemplate {
        page_title: format!(
            "{} #{}: {}",
            record.repo, record.branch_id, record.branch_name
        ),
        repo: record.repo,
        branch_id: record.branch_id,
        branch_chat_artifact_id: record.current_artifact_id,
        branch_name: record.branch_name,
        title: record.title,
        target_branch: record.target_branch,
        branch_state: record.branch_state.as_str().to_string(),
        merge_commit_sha: record.merge_commit_sha,
        tutorial_status: record.tutorial_status.as_str().to_string(),
        ci_status: record.ci_status.as_str().to_string(),
        executive_html,
        media_links,
        error_message: record.error_message,
        steps,
        diff_json: record.unified_diff.map(|d| {
            // Escape `</` as `<\/` to prevent the browser HTML parser from
            // prematurely closing the <script> tag when the diff contains
            // literal `</script>` sequences (e.g. from HTML source diffs).
            // `<\/` is valid JSON so JSON.parse still recovers the original.
            serde_json::to_string(&d)
                .unwrap_or_default()
                .replace("</", r"<\/")
        }),
        branch_ci_summary_html,
        branch_ci_summary_enabled,
        branch_chat_ready: record.claude_session_id.is_some(),
        review_mode,
    })
}

fn render_branch_ci_template_with_notices(
    record: BranchDetailRecord,
    ci_runs: Vec<BranchCiRunRecord>,
    page_notices: Vec<PageNoticeView>,
    review_mode: bool,
) -> anyhow::Result<BranchCiTemplate> {
    let branch_ci_live_html = render_branch_ci_live_html(&record, &ci_runs, &page_notices)?;
    let branch_ci_live_enabled = branch_ci_runs_are_active(&ci_runs);
    Ok(BranchCiTemplate {
        page_title: format!("{} #{} CI", record.repo, record.branch_id),
        repo: record.repo,
        branch_id: record.branch_id,
        branch_name: record.branch_name,
        title: record.title,
        target_branch: record.target_branch,
        updated_at: record.updated_at,
        branch_state: record.branch_state.as_str().to_string(),
        head_sha: record.head_sha,
        merge_base_sha: record.merge_base_sha,
        review_mode,
        back_href: branch_detail_path(record.branch_id, review_mode),
        branch_ci_live_html,
        branch_ci_live_enabled,
    })
}

#[cfg(test)]
fn render_nightly_template(run: NightlyRunRecord) -> NightlyTemplate {
    render_nightly_template_with_notices(run, Vec::new())
}

fn render_nightly_template_with_notices(
    run: NightlyRunRecord,
    page_notices: Vec<PageNoticeView>,
) -> NightlyTemplate {
    let nightly_live_html = render_nightly_live_html(&run, &page_notices)
        .unwrap_or_else(|_| "<section class=\"panel\"><h2>Lanes</h2><p class=\"muted\">Failed to render nightly lane state.</p></section>".to_string());
    let nightly_live_enabled = nightly_run_is_active(&run);
    NightlyTemplate {
        page_title: format!("{} nightly #{}", run.repo, run.nightly_run_id),
        repo: run.repo,
        nightly_run_id: run.nightly_run_id,
        summary: run.summary,
        scheduled_for: run.scheduled_for,
        created_at: run.created_at,
        nightly_live_html,
        nightly_live_enabled,
    }
}

fn branch_ci_runs_are_active(ci_runs: &[BranchCiRunRecord]) -> bool {
    ci_runs.iter().any(|run| {
        matches!(run.status, ForgeCiStatus::Queued | ForgeCiStatus::Running)
            || run
                .lanes
                .iter()
                .any(|lane| matches!(lane.status, CiLaneStatus::Queued | CiLaneStatus::Running))
    })
}

fn nightly_run_is_active(run: &NightlyRunRecord) -> bool {
    matches!(run.status, ForgeCiStatus::Queued | ForgeCiStatus::Running)
        || run
            .lanes
            .iter()
            .any(|lane| matches!(lane.status, CiLaneStatus::Queued | CiLaneStatus::Running))
}

fn render_branch_ci_live_html(
    record: &BranchDetailRecord,
    ci_runs: &[BranchCiRunRecord],
    page_notices: &[PageNoticeView],
) -> anyhow::Result<String> {
    render_branch_ci_live_html_at(record, ci_runs, page_notices, Utc::now())
}

fn render_branch_ci_live_html_at(
    record: &BranchDetailRecord,
    ci_runs: &[BranchCiRunRecord],
    page_notices: &[PageNoticeView],
    now: DateTime<Utc>,
) -> anyhow::Result<String> {
    let latest_failed_lane_count = ci_runs
        .first()
        .map(|run| {
            run.lanes
                .iter()
                .filter(|lane| lane.status == CiLaneStatus::Failed)
                .count()
        })
        .unwrap_or(0);
    BranchCiLiveTemplate {
        branch_id: record.branch_id,
        branch_state: record.branch_state.as_str().to_string(),
        tutorial_status: record.tutorial_status.as_str().to_string(),
        ci_status: record.ci_status.as_str().to_string(),
        ci_status_tone: ci_status_tone(record.ci_status.as_str()).to_string(),
        live_active: branch_ci_runs_are_active(ci_runs),
        ci_runs: ci_runs
            .iter()
            .cloned()
            .map(|run| map_ci_run_view(run, now))
            .collect(),
        page_notices: page_notices.to_vec(),
        latest_failed_lane_count,
    }
    .render()
    .context("render branch ci live template")
}

fn render_branch_ci_summary_html(
    record: &BranchDetailRecord,
    ci_runs: &[BranchCiRunRecord],
    page_notices: &[PageNoticeView],
    review_mode: bool,
) -> anyhow::Result<String> {
    render_branch_ci_summary_html_at(record, ci_runs, page_notices, review_mode, Utc::now())
}

fn render_branch_ci_summary_html_at(
    record: &BranchDetailRecord,
    ci_runs: &[BranchCiRunRecord],
    page_notices: &[PageNoticeView],
    review_mode: bool,
    now: DateTime<Utc>,
) -> anyhow::Result<String> {
    let latest_run = ci_runs.first().map(|run| map_ci_summary_run(run, now));
    BranchCiSummaryTemplate {
        ci_status: record.ci_status.as_str().to_string(),
        ci_status_tone: ci_status_tone(record.ci_status.as_str()).to_string(),
        live_active: branch_ci_runs_are_active(ci_runs),
        ci_details_path: branch_ci_page_path(record.branch_id, review_mode),
        latest_run,
        page_notices: page_notices.to_vec(),
    }
    .render()
    .context("render branch ci summary template")
}

fn render_nightly_live_html(
    run: &NightlyRunRecord,
    page_notices: &[PageNoticeView],
) -> anyhow::Result<String> {
    let failed_lane_count = run
        .lanes
        .iter()
        .filter(|lane| lane.status == CiLaneStatus::Failed)
        .count();
    NightlyLiveTemplate {
        nightly_run_id: run.nightly_run_id,
        status: run.status.as_str().to_string(),
        live_active: nightly_run_is_active(run),
        source_ref: run.source_ref.clone(),
        source_head_sha: run.source_head_sha.clone(),
        rerun_of_run_id: run.rerun_of_run_id,
        started_at: run.started_at.clone(),
        finished_at: run.finished_at.clone(),
        lanes: run
            .lanes
            .iter()
            .cloned()
            .map(map_nightly_lane_view)
            .collect(),
        page_notices: page_notices.to_vec(),
        failed_lane_count,
    }
    .render()
    .context("render nightly live template")
}

fn map_ci_run_view(run: BranchCiRunRecord, now: DateTime<Utc>) -> CiRunView {
    let status_tone = ci_status_tone(run.status.as_str()).to_string();
    let timing_summary = ci_timing_summary(
        &run.created_at,
        run.started_at.as_deref(),
        run.finished_at.as_deref(),
        now,
    );
    CiRunView {
        id: run.id,
        source_head_sha: run.source_head_sha,
        status: run.status.as_str().to_string(),
        status_tone,
        lane_count: run.lane_count,
        rerun_of_run_id: run.rerun_of_run_id,
        created_at: run.created_at,
        started_at: run.started_at,
        finished_at: run.finished_at,
        timing_summary,
        lanes: run
            .lanes
            .into_iter()
            .map(|lane| map_ci_lane_view(lane, now))
            .collect(),
    }
}

fn map_ci_lane_view(lane: BranchCiLaneRecord, now: DateTime<Utc>) -> CiLaneView {
    let operator_hint = lane_operator_hint(&LaneHintContext {
        now,
        status: lane.status.as_str(),
        execution_reason: lane.execution_reason,
        failure_kind: lane.failure_kind,
        created_at: &lane.created_at,
        started_at: lane.started_at.as_deref(),
        finished_at: lane.finished_at.as_deref(),
        last_heartbeat_at: lane.last_heartbeat_at.as_deref(),
        lease_expires_at: lane.lease_expires_at.as_deref(),
    });
    let status_tone = ci_status_tone(lane.status.as_str()).to_string();
    let failure_kind = lane.failure_kind.map(|kind| kind.as_str().to_string());
    let failure_kind_label = lane.failure_kind.map(|kind| kind.label().to_string());
    let timing_summary = ci_timing_summary(
        &lane.created_at,
        lane.started_at.as_deref(),
        lane.finished_at.as_deref(),
        now,
    );
    CiLaneView {
        id: lane.id,
        lane_id: lane.lane_id,
        title: lane.title,
        entrypoint: lane.entrypoint,
        status: lane.status.as_str().to_string(),
        status_tone,
        execution_reason: lane.execution_reason.as_str().to_string(),
        execution_reason_label: lane.execution_reason.label().to_string(),
        failure_kind,
        failure_kind_label,
        pikaci_run_id: lane.pikaci_run_id,
        pikaci_target_id: lane.pikaci_target_id,
        ci_target_key: lane.ci_target_key,
        log_text: lane.log_text,
        retry_count: lane.retry_count,
        rerun_of_lane_run_id: lane.rerun_of_lane_run_id,
        created_at: lane.created_at,
        started_at: lane.started_at,
        finished_at: lane.finished_at,
        timing_summary,
        last_heartbeat_at: lane.last_heartbeat_at,
        lease_expires_at: lane.lease_expires_at,
        operator_hint,
    }
}

fn map_ci_summary_run(run: &BranchCiRunRecord, now: DateTime<Utc>) -> CiSummaryRunView {
    let (success_count, active_count, failed_count) = ci_lane_counts(run);
    CiSummaryRunView {
        id: run.id,
        status: run.status.as_str().to_string(),
        status_tone: ci_status_tone(run.status.as_str()).to_string(),
        lane_count: run.lane_count,
        created_at: run.created_at.clone(),
        source_head_sha: run.source_head_sha.clone(),
        rerun_of_run_id: run.rerun_of_run_id,
        timing_summary: ci_timing_summary(
            &run.created_at,
            run.started_at.as_deref(),
            run.finished_at.as_deref(),
            now,
        ),
        success_count,
        active_count,
        failed_count,
        lanes: run
            .lanes
            .iter()
            .map(|lane| CiSummaryLaneView {
                title: lane.title.clone(),
                status: lane.status.as_str().to_string(),
                status_tone: ci_status_tone(lane.status.as_str()).to_string(),
            })
            .collect(),
    }
}

fn map_nightly_lane_view(lane: NightlyLaneRecord) -> NightlyLaneView {
    let now = Utc::now();
    let status_badge_class = lane_status_badge_class(lane.status.as_str()).to_string();
    let is_failed = lane.status == CiLaneStatus::Failed;
    let operator_hint = lane_operator_hint(&LaneHintContext {
        now,
        status: lane.status.as_str(),
        execution_reason: lane.execution_reason,
        failure_kind: lane.failure_kind,
        created_at: &lane.created_at,
        started_at: lane.started_at.as_deref(),
        finished_at: lane.finished_at.as_deref(),
        last_heartbeat_at: lane.last_heartbeat_at.as_deref(),
        lease_expires_at: lane.lease_expires_at.as_deref(),
    });
    let failure_kind = lane.failure_kind.map(|kind| kind.as_str().to_string());
    let failure_kind_label = lane.failure_kind.map(|kind| kind.label().to_string());
    NightlyLaneView {
        id: lane.id,
        lane_id: lane.lane_id,
        title: lane.title,
        entrypoint: lane.entrypoint,
        status: lane.status.as_str().to_string(),
        status_badge_class,
        is_failed,
        execution_reason: lane.execution_reason.as_str().to_string(),
        execution_reason_label: lane.execution_reason.label().to_string(),
        failure_kind,
        failure_kind_label,
        pikaci_run_id: lane.pikaci_run_id,
        pikaci_target_id: lane.pikaci_target_id,
        ci_target_key: lane.ci_target_key,
        log_text: lane.log_text,
        retry_count: lane.retry_count,
        rerun_of_lane_run_id: lane.rerun_of_lane_run_id,
        created_at: lane.created_at,
        started_at: lane.started_at,
        finished_at: lane.finished_at,
        last_heartbeat_at: lane.last_heartbeat_at,
        lease_expires_at: lane.lease_expires_at,
        operator_hint,
    }
}

fn lane_status_badge_class(status: &str) -> &'static str {
    match status {
        "failed" => "status-failed",
        "success" => "status-success",
        "running" => "status-running",
        "queued" => "status-queued",
        "skipped" => "status-skipped",
        _ => "status-neutral",
    }
}

fn parse_ci_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|value| DateTime::<Utc>::from_naive_utc_and_offset(value, Utc))
        })
}

fn ci_timing_summary(
    created_at: &str,
    started_at: Option<&str>,
    finished_at: Option<&str>,
    now: DateTime<Utc>,
) -> Option<String> {
    let created_at = parse_ci_timestamp(created_at);
    let started_at = started_at.and_then(parse_ci_timestamp);
    let finished_at = finished_at.and_then(parse_ci_timestamp);

    let queued = created_at.and_then(|created_at| {
        let queued_end = started_at.or(finished_at).unwrap_or(now);
        compact_duration_part("queued", queued_end.signed_duration_since(created_at))
    });

    let ran = started_at.and_then(|started_at| {
        let end = finished_at.unwrap_or(now);
        compact_duration_part("ran", end.signed_duration_since(started_at))
    });

    let parts = [queued, ran].into_iter().flatten().collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

fn compact_duration_part(label: &str, duration: TimeDelta) -> Option<String> {
    if duration.num_seconds() < 0 {
        return None;
    }
    Some(format!("{label} {}", format_compact_duration(duration)))
}

fn format_compact_duration(duration: TimeDelta) -> String {
    let total_seconds = duration.num_seconds().max(0);
    let days = total_seconds / 86_400;
    let hours = (total_seconds % 86_400) / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;

    if days > 0 {
        format!("{days}d {hours:02}h")
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

struct LaneHintContext<'a> {
    now: DateTime<Utc>,
    status: &'a str,
    execution_reason: CiLaneExecutionReason,
    failure_kind: Option<CiLaneFailureKind>,
    created_at: &'a str,
    started_at: Option<&'a str>,
    finished_at: Option<&'a str>,
    last_heartbeat_at: Option<&'a str>,
    lease_expires_at: Option<&'a str>,
}

fn lane_operator_hint(context: &LaneHintContext<'_>) -> Option<String> {
    match context.status {
        "queued" => match context.execution_reason {
            CiLaneExecutionReason::BlockedByConcurrencyGroup => {
                Some("Blocked by another lane in the same concurrency group.".to_string())
            }
            CiLaneExecutionReason::WaitingForCapacity => Some(
                "Waiting for scheduler capacity. Other runnable lanes are already consuming the active worker slots."
                    .to_string(),
            ),
            CiLaneExecutionReason::StaleRecovered => Some(
                "Recovered after a stale lease expired. This lane is ready to be reclaimed."
                    .to_string(),
            ),
            _ => {
                let age = parse_ci_timestamp(context.created_at)
                    .map(|created| context.now.signed_duration_since(created).num_minutes());
                if age.is_some_and(|minutes| minutes >= 15) {
                    Some(format!(
                        "Queued too long since {}. Wake CI or requeue if the scheduler is wedged.",
                        context.created_at
                    ))
                } else {
                    Some(format!("Queued since {}.", context.created_at))
                }
            }
        },
        "running" => {
            let lease_note = context.lease_expires_at.map_or_else(
                || "Running with no lease metadata.".to_string(),
                |lease| {
                    let prefix = match parse_ci_timestamp(lease) {
                        Some(expires_at) if expires_at <= context.now => {
                            "Running with an expired lease"
                        }
                        _ => "Running with lease",
                    };
                    format!("{prefix} until {lease}.")
                },
            );
            let heartbeat_note = context
                .last_heartbeat_at
                .map(|heartbeat| format!(" Last heartbeat {heartbeat}."))
                .unwrap_or_default();
            Some(format!("{lease_note}{heartbeat_note}"))
        }
        "failed" => {
            let failure_detail = context
                .failure_kind
                .map(|kind| format!(" Classified as {}.", kind.label()))
                .unwrap_or_default();
            Some(match context.finished_at {
                Some(finished_at) => format!("Failed at {finished_at}.{failure_detail}"),
                None => format!("Failed.{failure_detail}"),
            })
        }
        "success" | "skipped" => None,
        _ => context
            .started_at
            .map(|started_at| format!("State updated after start at {started_at}."))
            .or_else(|| Some(format!("Current state: {}.", context.status))),
    }
}
#[derive(serde::Deserialize)]
struct ForgeBranchResolveQuery {
    branch_name: String,
}

#[derive(serde::Deserialize)]
struct ForgeBranchLogsQuery {
    lane: Option<String>,
    lane_run_id: Option<i64>,
}


fn markdown_to_safe_html(markdown: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(markdown, opts);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    let mut builder = ammonia::Builder::default();
    builder.add_tags(&["table", "thead", "tbody", "tr", "th", "td"]);
    builder.clean(&html_output).to_string()
}
