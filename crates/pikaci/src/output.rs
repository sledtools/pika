use anyhow::Context;
use jerichoci::{RunRecord, RunStatus};

pub(crate) fn print_json(value: &impl serde::Serialize) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut locked = stdout.lock();
    serde_json::to_writer_pretty(&mut locked, value).context("encode json output")?;
    use std::io::Write as _;
    writeln!(&mut locked).context("terminate json output")?;
    Ok(())
}

pub(crate) fn print_json_line(value: &impl serde::Serialize) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut locked = stdout.lock();
    serde_json::to_writer(&mut locked, value).context("encode json line")?;
    use std::io::Write as _;
    writeln!(&mut locked).context("terminate json line")?;
    Ok(())
}

pub(crate) fn print_run_human_summary(run: &RunRecord) {
    if run.jobs.is_empty() {
        let target_id = run.target_id.as_deref().unwrap_or("-");
        println!("{} {} {}", run.run_id, target_id, status_text(run.status));
        if let Some(message) = &run.message {
            eprintln!("{message}");
        }
    } else {
        for job in &run.jobs {
            println!("{} {} {}", run.run_id, job.id, status_text(job.status));
            if let Some(message) = &job.message {
                eprintln!("{message}");
            }
        }
    }
}

pub(crate) fn format_status_lines(run: &RunRecord) -> Vec<String> {
    let mut lines = vec![format!(
        "{} {} {}",
        run.run_id,
        run.target_id.as_deref().unwrap_or("-"),
        status_text(run.status)
    )];
    if let Some(plan_path) = &run.plan_path {
        lines.push(format!("plan={plan_path}"));
    }
    if let Some(prepared_outputs_path) = &run.prepared_outputs_path {
        lines.push(format!("prepared_outputs={prepared_outputs_path}"));
    }
    if let Some(prepared_output_consumer) = run.prepared_output_consumer {
        lines.push(format!(
            "prepared_output_consumer={}",
            prepared_output_consumer_label(prepared_output_consumer)
        ));
    }
    if let Some(prepared_output_mode) = &run.prepared_output_mode {
        lines.push(format!("prepared_output_mode={prepared_output_mode}"));
    }
    if let Some(prepared_output_invocation_mode) = run.prepared_output_invocation_mode {
        lines.push(format!(
            "prepared_output_invocation_mode={}",
            prepared_output_invocation_mode_label(prepared_output_invocation_mode)
        ));
    }
    if let Some(prepared_output_invocation_wrapper_program) =
        &run.prepared_output_invocation_wrapper_program
    {
        lines.push(format!(
            "prepared_output_invocation_launcher_program={prepared_output_invocation_wrapper_program}"
        ));
    }
    if let Some(prepared_output_launcher_transport_mode) =
        run.prepared_output_launcher_transport_mode
    {
        lines.push(format!(
            "prepared_output_launcher_transport_mode={}",
            prepared_output_launcher_transport_mode_label(prepared_output_launcher_transport_mode)
        ));
    }
    if let Some(prepared_output_launcher_transport_program) =
        &run.prepared_output_launcher_transport_program
    {
        lines.push(format!(
            "prepared_output_launcher_transport_program={prepared_output_launcher_transport_program}"
        ));
    }
    if let Some(prepared_output_launcher_transport_host) =
        &run.prepared_output_launcher_transport_host
    {
        lines.push(format!(
            "prepared_output_launcher_transport_host={prepared_output_launcher_transport_host}"
        ));
    }
    if let Some(prepared_output_launcher_transport_remote_launcher_program) =
        &run.prepared_output_launcher_transport_remote_launcher_program
    {
        lines.push(format!(
            "prepared_output_launcher_transport_remote_launcher_program={prepared_output_launcher_transport_remote_launcher_program}"
        ));
    }
    if let Some(prepared_output_launcher_transport_remote_helper_program) =
        &run.prepared_output_launcher_transport_remote_helper_program
    {
        lines.push(format!(
            "prepared_output_launcher_transport_remote_helper_program={prepared_output_launcher_transport_remote_helper_program}"
        ));
    }
    if let Some(prepared_output_launcher_transport_remote_work_dir) =
        &run.prepared_output_launcher_transport_remote_work_dir
    {
        lines.push(format!(
            "prepared_output_launcher_transport_remote_work_dir={prepared_output_launcher_transport_remote_work_dir}"
        ));
    }
    if let Some(message) = &run.message {
        lines.push(message.clone());
    }
    if !run.changed_files.is_empty() {
        lines.push(format!("changed_files={}", run.changed_files.len()));
    }
    for job in &run.jobs {
        lines.push(format!("{} {}", job.id, status_text(job.status)));
    }
    lines
}

pub(crate) fn status_text(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "running",
        RunStatus::Passed => "passed",
        RunStatus::Failed => "failed",
        RunStatus::Skipped => "skipped",
    }
}

pub(crate) fn prepared_output_consumer_label(
    kind: jerichoci::PreparedOutputConsumerKind,
) -> &'static str {
    match kind {
        jerichoci::PreparedOutputConsumerKind::HostLocalSymlinkMountsV1 => {
            "host_local_symlink_mounts_v1"
        }
        jerichoci::PreparedOutputConsumerKind::RemoteExposureRequestV1 => {
            "remote_exposure_request_v1"
        }
        jerichoci::PreparedOutputConsumerKind::FulfillRequestCliV1 => "fulfill_request_cli_v1",
    }
}

fn prepared_output_invocation_mode_label(
    mode: jerichoci::PreparedOutputInvocationMode,
) -> &'static str {
    match mode {
        jerichoci::PreparedOutputInvocationMode::DirectHelperExecV1 => "direct_helper_exec_v1",
        jerichoci::PreparedOutputInvocationMode::ExternalWrapperCommandV1 => {
            "external_wrapper_command_v1"
        }
    }
}

fn prepared_output_launcher_transport_mode_label(
    mode: jerichoci::PreparedOutputLauncherTransportMode,
) -> &'static str {
    match mode {
        jerichoci::PreparedOutputLauncherTransportMode::DirectLauncherExecV1 => {
            "direct_launcher_exec_v1"
        }
        jerichoci::PreparedOutputLauncherTransportMode::CommandTransportV1 => {
            "command_transport_v1"
        }
        jerichoci::PreparedOutputLauncherTransportMode::SshLauncherTransportV1 => {
            "ssh_launcher_transport_v1"
        }
    }
}
