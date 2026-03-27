mod executor;
mod model;
mod run;
mod snapshot;

pub use executor::{StagedLinuxRemoteDefaults, staged_linux_remote_defaults};
pub use model::{
    ExecuteNode, HostProcessRuntimeConfig, IncusRuntimeConfig, JobExecutionConfig,
    JobPlacementKind, JobRuntimeConfig, JobRuntimeKind, PlanExecutorKind, PlanNodeRecord,
    PlanScope, PrepareNode, PreparedOutputConsumerKind, PreparedOutputFulfillmentLaunchRequest,
    PreparedOutputFulfillmentResult, PreparedOutputFulfillmentStatus,
    PreparedOutputFulfillmentTransportPathContract, PreparedOutputFulfillmentTransportRequest,
    PreparedOutputInvocationMode, PreparedOutputLauncherTransportMode,
    PreparedOutputPayloadManifestRecord, PreparedOutputPayloadPathRecord,
    PreparedOutputRemoteExposureRequest, PreparedOutputsRecord, RealizedPreparedOutputRecord,
    RemoteLinuxVmBackend, RemoteLinuxVmExecutionRecord, RemoteLinuxVmImageRecord,
    RemoteLinuxVmPhase, RemoteLinuxVmPhaseRecord, RunBundle, RunLifecycleEvent, RunLogsMetadata,
    RunPlanRecord, StagedLinuxCommandConfig, StagedLinuxRustPayloadRole,
    StagedLinuxRustTargetPayloadSpec, StagedLinuxSnapshotProfile, TartRuntimeConfig,
};
pub use model::{
    GuestCommand, JobLogMetadata, JobOutcome, JobRecord, JobSpec, RunRecord, RunStatus, RunnerKind,
};
pub use run::{
    LogKind, Logs, RunMetadata, RunOptions, fulfill_prepared_output_request,
    fulfill_prepared_output_request_result, gc_runs, list_runs, load_logs, load_logs_metadata,
    load_prepared_output_fulfillment_launch_request,
    load_prepared_output_fulfillment_transport_request, load_prepared_outputs,
    load_prepared_outputs_record, load_run_bundle, load_run_record, record_skipped_run,
    record_skipped_run_with_reporter, rerun_jobs_with_metadata,
    rerun_jobs_with_metadata_and_reporter, run_job, run_jobs, run_jobs_with_metadata,
    run_jobs_with_metadata_and_reporter, write_prepared_output_fulfillment_launch_request,
    write_prepared_output_fulfillment_result, write_prepared_output_fulfillment_transport_request,
};
pub use snapshot::git_changed_files;

#[cfg(test)]
pub(crate) mod test_support {
    use std::cell::Cell;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    thread_local! {
        static ENV_LOCK_DEPTH: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) struct EnvLockGuard {
        _guard: Option<MutexGuard<'static, ()>>,
    }

    pub(crate) fn env_lock() -> EnvLockGuard {
        let guard = ENV_LOCK_DEPTH.with(|depth| {
            let current = depth.get();
            depth.set(current + 1);
            if current == 0 {
                Some(
                    ENV_LOCK
                        .get_or_init(|| Mutex::new(()))
                        .lock()
                        .unwrap_or_else(|err| err.into_inner()),
                )
            } else {
                None
            }
        });
        EnvLockGuard { _guard: guard }
    }

    impl Drop for EnvLockGuard {
        fn drop(&mut self) {
            ENV_LOCK_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
        }
    }
}
