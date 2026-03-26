use tokio::sync::broadcast;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiLiveUpdate {
    BranchChanged {
        branch_id: i64,
        reason: &'static str,
    },
    NightlyChanged {
        nightly_run_id: i64,
        reason: &'static str,
    },
}

#[derive(Clone, Debug)]
pub struct CiLiveUpdates {
    sender: broadcast::Sender<CiLiveUpdate>,
}

impl CiLiveUpdates {
    pub fn new(buffer: usize) -> Self {
        let (sender, _) = broadcast::channel(buffer);
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CiLiveUpdate> {
        self.sender.subscribe()
    }

    pub fn branch_changed(&self, branch_id: i64, reason: &'static str) {
        let _ = self
            .sender
            .send(CiLiveUpdate::BranchChanged { branch_id, reason });
    }

    pub fn nightly_changed(&self, nightly_run_id: i64, reason: &'static str) {
        let _ = self.sender.send(CiLiveUpdate::NightlyChanged {
            nightly_run_id,
            reason,
        });
    }
}
