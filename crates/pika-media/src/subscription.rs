use std::any::Any;
use std::sync::mpsc::{Receiver, RecvError, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use crate::session::{MediaFrame, MediaSessionError};

// Sync receiver wrapper + keepalive guard.
//
// For the network transport, the subscriber task runs on a tokio runtime owned by a relay
// worker thread. If all relay handles are dropped while a subscription is still in use, the
// worker thread must stay alive; otherwise the subscriber task can fail mid-setup and surface
// misleading errors (e.g. "failed DNS lookup").
pub struct MediaFrameSubscription {
    rx: Receiver<MediaFrame>,
    ready: Receiver<Result<(), MediaSessionError>>,
    _keepalive: Option<Arc<dyn Any + Send + Sync>>,
}

impl MediaFrameSubscription {
    pub(crate) fn new(
        rx: Receiver<MediaFrame>,
        ready: Receiver<Result<(), MediaSessionError>>,
        keepalive: Option<Arc<dyn Any + Send + Sync>>,
    ) -> Self {
        Self {
            rx,
            ready,
            _keepalive: keepalive,
        }
    }

    pub fn try_recv(&self) -> Result<MediaFrame, TryRecvError> {
        self.rx.try_recv()
    }

    pub fn recv(&self) -> Result<MediaFrame, RecvError> {
        self.rx.recv()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<MediaFrame, RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }

    pub fn wait_ready(&self, timeout: Duration) -> Result<(), MediaSessionError> {
        match self.ready.recv_timeout(timeout) {
            Ok(res) => res,
            Err(RecvTimeoutError::Timeout) => Err(MediaSessionError::Timeout(
                "timed out waiting for media subscription ready".to_string(),
            )),
            Err(RecvTimeoutError::Disconnected) => Err(MediaSessionError::NotConnected),
        }
    }
}
