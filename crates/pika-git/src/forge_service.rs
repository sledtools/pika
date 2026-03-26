use std::sync::Arc;

use pika_forge_model::BranchState;

use crate::branch_store::{BranchActionTarget, BranchDetailRecord, BranchLookupRecord};
use crate::ci_state::CiLaneStatus;
use crate::ci_store::{BranchCiRunRecord, NightlyRunRecord};
use crate::config::Config;
use crate::forge;
use crate::forge_runtime::ForgeRuntime;
use crate::live::CiLiveUpdates;
use crate::storage::Store;

#[derive(Clone)]
pub(crate) struct ForgeService {
    store: Store,
    config: Config,
    live_updates: CiLiveUpdates,
    forge_runtime: Arc<ForgeRuntime>,
}

#[derive(Debug)]
pub(crate) enum ForgeServiceError {
    NotFound(String),
    Conflict(String),
    Internal(String),
}

#[derive(Clone)]
pub(crate) struct BranchDetailAndRuns {
    pub(crate) detail: BranchDetailRecord,
    pub(crate) ci_runs: Vec<BranchCiRunRecord>,
}

pub(crate) struct MergeBranchResult {
    pub(crate) branch_id: i64,
    pub(crate) merge_commit_sha: String,
}

pub(crate) struct CloseBranchResult {
    pub(crate) branch_id: i64,
    pub(crate) deleted: bool,
}

pub(crate) struct BranchLaneRerunResult {
    pub(crate) branch_id: i64,
    pub(crate) rerun_suite_id: i64,
}

pub(crate) struct NightlyLaneRerunResult {
    pub(crate) nightly_run_id: i64,
    pub(crate) rerun_run_id: i64,
}

pub(crate) struct BranchLaneMutationResult {
    pub(crate) branch_id: i64,
    pub(crate) lane_run_id: i64,
    pub(crate) lane_status: CiLaneStatus,
}

pub(crate) struct NightlyLaneMutationResult {
    pub(crate) nightly_run_id: i64,
    pub(crate) lane_run_id: i64,
    pub(crate) lane_status: CiLaneStatus,
}

pub(crate) struct BranchRunRecoveryResult {
    pub(crate) branch_id: i64,
    pub(crate) run_id: i64,
    pub(crate) recovered_lane_count: usize,
}

pub(crate) struct NightlyRunRecoveryResult {
    pub(crate) nightly_run_id: i64,
    pub(crate) recovered_lane_count: usize,
}

impl ForgeService {
    pub(crate) fn new(
        store: Store,
        config: Config,
        live_updates: CiLiveUpdates,
        forge_runtime: Arc<ForgeRuntime>,
    ) -> Self {
        Self {
            store,
            config,
            live_updates,
            forge_runtime,
        }
    }

    pub(crate) async fn branch_detail_and_runs(
        &self,
        branch_id: i64,
        run_limit: usize,
    ) -> Result<Option<BranchDetailAndRuns>, ForgeServiceError> {
        let detail_store = self.store.clone();
        let runs_store = self.store.clone();
        let detail =
            match tokio::task::spawn_blocking(move || detail_store.get_branch_detail(branch_id))
                .await
            {
                Ok(Ok(Some(record))) => record,
                Ok(Ok(None)) => return Ok(None),
                Ok(Err(err)) => {
                    return Err(ForgeServiceError::Internal(format!(
                        "failed to query branch detail: {}",
                        err
                    )));
                }
                Err(err) => {
                    return Err(ForgeServiceError::Internal(format!(
                        "detail worker task failed: {}",
                        err
                    )));
                }
            };
        let ci_runs = match tokio::task::spawn_blocking(move || {
            runs_store.list_branch_ci_runs(branch_id, run_limit)
        })
        .await
        {
            Ok(Ok(runs)) => runs,
            Ok(Err(err)) => {
                return Err(ForgeServiceError::Internal(format!(
                    "failed to query branch ci runs: {}",
                    err
                )));
            }
            Err(err) => {
                return Err(ForgeServiceError::Internal(format!(
                    "ci worker task failed: {}",
                    err
                )));
            }
        };
        Ok(Some(BranchDetailAndRuns { detail, ci_runs }))
    }

    pub(crate) async fn resolve_branch_by_name(
        &self,
        branch_name: &str,
    ) -> Result<Option<BranchLookupRecord>, ForgeServiceError> {
        let repo = self
            .config
            .effective_forge_repo()
            .map(|repo| repo.repo)
            .unwrap_or_else(|| "sledtools/pika".to_string());
        let branch_name = branch_name.to_string();
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || store.find_branch_by_name(&repo, &branch_name))
            .await
        {
            Ok(Ok(branch)) => Ok(branch),
            Ok(Err(err)) => Err(ForgeServiceError::Internal(err.to_string())),
            Err(err) => Err(ForgeServiceError::Internal(err.to_string())),
        }
    }

    pub(crate) async fn nightly_run(
        &self,
        nightly_run_id: i64,
    ) -> Result<Option<NightlyRunRecord>, ForgeServiceError> {
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || store.get_nightly_run(nightly_run_id)).await {
            Ok(Ok(run)) => Ok(run),
            Ok(Err(err)) => Err(ForgeServiceError::Internal(err.to_string())),
            Err(err) => Err(ForgeServiceError::Internal(err.to_string())),
        }
    }

    pub(crate) async fn merge_branch(
        &self,
        branch_id: i64,
        merged_by: &str,
    ) -> Result<MergeBranchResult, ForgeServiceError> {
        let Some(forge_repo) = self.config.effective_forge_repo() else {
            return Err(ForgeServiceError::Internal(
                "forge repo is not configured".to_string(),
            ));
        };

        let target = self.branch_action_target(branch_id).await?;
        if target.branch_state != BranchState::Open {
            return Err(ForgeServiceError::Conflict(
                "only open branches can be merged".to_string(),
            ));
        }

        let current_head = match forge::current_branch_head(&forge_repo, &target.branch_name) {
            Ok(Some(head)) => head,
            Ok(None) => {
                return Err(ForgeServiceError::Conflict(
                    "branch ref no longer exists".to_string(),
                ));
            }
            Err(err) => return Err(ForgeServiceError::Internal(err.to_string())),
        };
        if current_head != target.head_sha {
            return Err(ForgeServiceError::Conflict(
                "branch head changed; refresh before merging".to_string(),
            ));
        }

        let merge_outcome = match tokio::task::spawn_blocking({
            let forge_repo = forge_repo.clone();
            let branch_name = target.branch_name.clone();
            let expected_head = target.head_sha.clone();
            move || forge::merge_branch(&forge_repo, &branch_name, &expected_head)
        })
        .await
        {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(err)) => return Err(ForgeServiceError::Conflict(err.to_string())),
            Err(err) => return Err(ForgeServiceError::Internal(err.to_string())),
        };

        let merge_commit_sha = merge_outcome.merge_commit_sha.clone();
        let merged_by = merged_by.to_string();
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || {
            store.mark_branch_merged(branch_id, &merged_by, &merge_commit_sha)
        })
        .await
        {
            Ok(Ok(())) => {
                self.forge_runtime.request_mirror();
                Ok(MergeBranchResult {
                    branch_id,
                    merge_commit_sha: merge_outcome.merge_commit_sha,
                })
            }
            Ok(Err(err)) => Err(ForgeServiceError::Internal(err.to_string())),
            Err(err) => Err(ForgeServiceError::Internal(err.to_string())),
        }
    }

    pub(crate) async fn close_branch(
        &self,
        branch_id: i64,
        closed_by: &str,
    ) -> Result<CloseBranchResult, ForgeServiceError> {
        let Some(forge_repo) = self.config.effective_forge_repo() else {
            return Err(ForgeServiceError::Internal(
                "forge repo is not configured".to_string(),
            ));
        };

        let target = self.branch_action_target(branch_id).await?;
        if target.branch_state != BranchState::Open {
            return Err(ForgeServiceError::Conflict(
                "only open branches can be closed".to_string(),
            ));
        }

        let current_head = match forge::current_branch_head(&forge_repo, &target.branch_name) {
            Ok(Some(head)) => head,
            Ok(None) => {
                return Err(ForgeServiceError::Conflict(
                    "branch ref no longer exists".to_string(),
                ));
            }
            Err(err) => return Err(ForgeServiceError::Internal(err.to_string())),
        };
        if current_head != target.head_sha {
            return Err(ForgeServiceError::Conflict(
                "branch head changed; refresh before closing".to_string(),
            ));
        }

        let close_outcome = match tokio::task::spawn_blocking({
            let forge_repo = forge_repo.clone();
            let branch_name = target.branch_name.clone();
            let expected_head = target.head_sha.clone();
            move || forge::close_branch(&forge_repo, &branch_name, &expected_head)
        })
        .await
        {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(err)) => return Err(ForgeServiceError::Conflict(err.to_string())),
            Err(err) => return Err(ForgeServiceError::Internal(err.to_string())),
        };

        let closed_by = closed_by.to_string();
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || store.mark_branch_closed(branch_id, &closed_by))
            .await
        {
            Ok(Ok(())) => {
                self.forge_runtime.request_mirror();
                Ok(CloseBranchResult {
                    branch_id,
                    deleted: close_outcome.deleted,
                })
            }
            Ok(Err(err)) => Err(ForgeServiceError::Internal(err.to_string())),
            Err(err) => Err(ForgeServiceError::Internal(err.to_string())),
        }
    }

    pub(crate) async fn rerun_branch_ci_lane(
        &self,
        branch_id: i64,
        lane_run_id: i64,
    ) -> Result<Option<BranchLaneRerunResult>, ForgeServiceError> {
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || {
            store.rerun_branch_ci_lane(branch_id, lane_run_id)
        })
        .await
        {
            Ok(Ok(Some(rerun_suite_id))) => {
                self.live_updates.branch_changed(branch_id, "rerun_queued");
                self.forge_runtime.wake_ci();
                Ok(Some(BranchLaneRerunResult {
                    branch_id,
                    rerun_suite_id,
                }))
            }
            Ok(Ok(None)) => Ok(None),
            Ok(Err(err)) => Err(ForgeServiceError::Conflict(err.to_string())),
            Err(err) => Err(ForgeServiceError::Internal(err.to_string())),
        }
    }

    pub(crate) async fn rerun_nightly_lane(
        &self,
        nightly_run_id: i64,
        lane_run_id: i64,
    ) -> Result<Option<NightlyLaneRerunResult>, ForgeServiceError> {
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || {
            store.rerun_nightly_lane(nightly_run_id, lane_run_id)
        })
        .await
        {
            Ok(Ok(Some(rerun_run_id))) => {
                self.live_updates
                    .nightly_changed(nightly_run_id, "rerun_queued");
                self.forge_runtime.wake_ci();
                Ok(Some(NightlyLaneRerunResult {
                    nightly_run_id,
                    rerun_run_id,
                }))
            }
            Ok(Ok(None)) => Ok(None),
            Ok(Err(err)) => Err(ForgeServiceError::Conflict(err.to_string())),
            Err(err) => Err(ForgeServiceError::Internal(err.to_string())),
        }
    }

    pub(crate) async fn fail_branch_ci_lane(
        &self,
        branch_id: i64,
        lane_run_id: i64,
        failed_by: &str,
    ) -> Result<Option<BranchLaneMutationResult>, ForgeServiceError> {
        let failed_by = failed_by.to_string();
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || {
            store.fail_branch_ci_lane(branch_id, lane_run_id, &failed_by)
        })
        .await
        {
            Ok(Ok(Some(()))) => {
                self.live_updates.branch_changed(branch_id, "lane_failed");
                self.forge_runtime.wake_ci();
                Ok(Some(BranchLaneMutationResult {
                    branch_id,
                    lane_run_id,
                    lane_status: CiLaneStatus::Failed,
                }))
            }
            Ok(Ok(None)) => Ok(None),
            Ok(Err(err)) => Err(ForgeServiceError::Conflict(err.to_string())),
            Err(err) => Err(ForgeServiceError::Internal(err.to_string())),
        }
    }

    pub(crate) async fn requeue_branch_ci_lane(
        &self,
        branch_id: i64,
        lane_run_id: i64,
    ) -> Result<Option<BranchLaneMutationResult>, ForgeServiceError> {
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || {
            store.requeue_branch_ci_lane(branch_id, lane_run_id)
        })
        .await
        {
            Ok(Ok(Some(()))) => {
                self.live_updates.branch_changed(branch_id, "lane_requeued");
                self.forge_runtime.wake_ci();
                Ok(Some(BranchLaneMutationResult {
                    branch_id,
                    lane_run_id,
                    lane_status: CiLaneStatus::Queued,
                }))
            }
            Ok(Ok(None)) => Ok(None),
            Ok(Err(err)) => Err(ForgeServiceError::Conflict(err.to_string())),
            Err(err) => Err(ForgeServiceError::Internal(err.to_string())),
        }
    }

    pub(crate) async fn recover_branch_ci_run(
        &self,
        branch_id: i64,
        run_id: i64,
    ) -> Result<Option<BranchRunRecoveryResult>, ForgeServiceError> {
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || store.recover_branch_ci_run(branch_id, run_id))
            .await
        {
            Ok(Ok(Some(recovered_lane_count))) => {
                self.live_updates.branch_changed(branch_id, "run_recovered");
                self.forge_runtime.wake_ci();
                Ok(Some(BranchRunRecoveryResult {
                    branch_id,
                    run_id,
                    recovered_lane_count,
                }))
            }
            Ok(Ok(None)) => Ok(None),
            Ok(Err(err)) => Err(ForgeServiceError::Conflict(err.to_string())),
            Err(err) => Err(ForgeServiceError::Internal(err.to_string())),
        }
    }

    pub(crate) async fn fail_nightly_lane(
        &self,
        nightly_run_id: i64,
        lane_run_id: i64,
        failed_by: &str,
    ) -> Result<Option<NightlyLaneMutationResult>, ForgeServiceError> {
        let failed_by = failed_by.to_string();
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || {
            store.fail_nightly_lane(nightly_run_id, lane_run_id, &failed_by)
        })
        .await
        {
            Ok(Ok(Some(()))) => {
                self.live_updates
                    .nightly_changed(nightly_run_id, "lane_failed");
                self.forge_runtime.wake_ci();
                Ok(Some(NightlyLaneMutationResult {
                    nightly_run_id,
                    lane_run_id,
                    lane_status: CiLaneStatus::Failed,
                }))
            }
            Ok(Ok(None)) => Ok(None),
            Ok(Err(err)) => Err(ForgeServiceError::Conflict(err.to_string())),
            Err(err) => Err(ForgeServiceError::Internal(err.to_string())),
        }
    }

    pub(crate) async fn requeue_nightly_lane(
        &self,
        nightly_run_id: i64,
        lane_run_id: i64,
    ) -> Result<Option<NightlyLaneMutationResult>, ForgeServiceError> {
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || {
            store.requeue_nightly_lane(nightly_run_id, lane_run_id)
        })
        .await
        {
            Ok(Ok(Some(()))) => {
                self.live_updates
                    .nightly_changed(nightly_run_id, "lane_requeued");
                self.forge_runtime.wake_ci();
                Ok(Some(NightlyLaneMutationResult {
                    nightly_run_id,
                    lane_run_id,
                    lane_status: CiLaneStatus::Queued,
                }))
            }
            Ok(Ok(None)) => Ok(None),
            Ok(Err(err)) => Err(ForgeServiceError::Conflict(err.to_string())),
            Err(err) => Err(ForgeServiceError::Internal(err.to_string())),
        }
    }

    pub(crate) async fn recover_nightly_run(
        &self,
        nightly_run_id: i64,
    ) -> Result<Option<NightlyRunRecoveryResult>, ForgeServiceError> {
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || store.recover_nightly_run(nightly_run_id)).await {
            Ok(Ok(Some(recovered_lane_count))) => {
                self.live_updates
                    .nightly_changed(nightly_run_id, "run_recovered");
                self.forge_runtime.wake_ci();
                Ok(Some(NightlyRunRecoveryResult {
                    nightly_run_id,
                    recovered_lane_count,
                }))
            }
            Ok(Ok(None)) => Ok(None),
            Ok(Err(err)) => Err(ForgeServiceError::Conflict(err.to_string())),
            Err(err) => Err(ForgeServiceError::Internal(err.to_string())),
        }
    }

    pub(crate) fn wake_ci(&self) {
        self.forge_runtime.wake_ci();
    }

    async fn branch_action_target(
        &self,
        branch_id: i64,
    ) -> Result<BranchActionTarget, ForgeServiceError> {
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || store.get_branch_action_target(branch_id)).await {
            Ok(Ok(Some(target))) => Ok(target),
            Ok(Ok(None)) => Err(ForgeServiceError::NotFound("branch not found".to_string())),
            Ok(Err(err)) => Err(ForgeServiceError::Internal(err.to_string())),
            Err(err) => Err(ForgeServiceError::Internal(err.to_string())),
        }
    }
}
