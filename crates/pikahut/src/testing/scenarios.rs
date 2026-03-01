//! Scenario orchestration entrypoints for `pikahut::testing`.
//!
//! Domain-specific implementations live under `testing/scenarios/*`.

mod artifacts;
mod common;
mod deterministic;
mod interop;
mod openclaw;
pub mod primal;
pub mod public;
pub mod types;

pub use deterministic::{run_cli_smoke, run_scenario, run_ui_e2e_local};
pub use interop::run_interop_rust_baseline;
pub use openclaw::run_openclaw_e2e;
pub use types::{
    CliSmokeRequest, InteropRustBaselineRequest, OpenclawE2eRequest, ScenarioRequest,
    ScenarioRunOutput, UiE2eLocalRequest, UiPlatform,
};
