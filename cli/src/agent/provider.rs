use std::time::Duration;

use pika_agent_protocol::projection::ProjectionMode;

#[derive(Clone, Copy, Debug)]
pub struct KeyPackageWaitPlan {
    pub progress_message: &'static str,
    pub timeout: Duration,
    pub fetch_timeout: Duration,
    pub retry_delay: Duration,
}

#[derive(Clone, Copy, Debug)]
pub struct GroupCreatePlan {
    pub progress_message: &'static str,
    pub create_group_context: &'static str,
    pub build_welcome_context: &'static str,
    pub welcome_publish_label: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub struct ChatLoopPlan {
    pub outbound_publish_label: &'static str,
    pub wait_for_pending_replies_on_eof: bool,
    pub eof_reply_timeout: Duration,
    pub projection_mode: ProjectionMode,
}
