#[derive(Debug, Default)]
pub(super) struct CallRuntime;

impl CallRuntime {
    pub(super) fn on_call_connecting(&mut self, _call_id: &str) {
        // Phase-0 scaffold only. Media worker lifecycle gets wired in later phases.
    }

    pub(super) fn on_call_ended(&mut self, _call_id: &str) {
        // Phase-0 scaffold only. Media worker lifecycle gets wired in later phases.
    }
}
