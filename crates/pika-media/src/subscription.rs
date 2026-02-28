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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn wait_ready_times_out_when_ready_signal_is_delayed() {
        let (_frame_tx, frame_rx) = mpsc::channel::<MediaFrame>();
        let (_ready_tx, ready_rx) = mpsc::channel::<Result<(), MediaSessionError>>();
        let subscription = MediaFrameSubscription::new(frame_rx, ready_rx, None);
        let err = subscription
            .wait_ready(Duration::from_millis(10))
            .expect_err("wait_ready should time out");
        match err {
            MediaSessionError::Timeout(message) => {
                assert!(
                    message.contains("media subscription ready"),
                    "timeout should mention subscription readiness, got: {message}"
                );
            }
            other => panic!("expected timeout error, got: {other:?}"),
        }
    }

    #[test]
    fn wait_ready_returns_not_connected_when_ready_sender_drops() {
        let (_frame_tx, frame_rx) = mpsc::channel::<MediaFrame>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), MediaSessionError>>();
        drop(ready_tx);
        let subscription = MediaFrameSubscription::new(frame_rx, ready_rx, None);
        let err = subscription
            .wait_ready(Duration::from_millis(10))
            .expect_err("wait_ready should report disconnected sender");
        assert!(matches!(err, MediaSessionError::NotConnected));
    }
}
