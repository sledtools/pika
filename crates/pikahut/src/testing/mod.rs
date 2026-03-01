//! Library-first integration testing primitives for `pikahut`.
//!
//! This module is the canonical API surface for test fixture lifecycle,
//! command orchestration, capability gating, and artifact capture.
//! Compatibility CLI entrypoints (`pikahut test ...`) are expected to call
//! these same primitives instead of owning bespoke orchestration logic.

pub mod capabilities;
pub mod command;
pub mod context;
pub mod fixture;
pub mod scenarios;
pub mod selector;

pub use capabilities::{Capabilities, RequireOutcome, Requirement, SkipReason};
pub use command::{CommandOutput, CommandRunner, CommandSpec};
pub use context::{ArtifactPolicy, TestContext, TestContextBuilder};
pub use fixture::{FixtureBuilder, FixtureHandle, FixtureSpec, start_fixture};
pub use scenarios::{
    CliSmokeRequest, InteropRustBaselineRequest, OpenclawE2eRequest, ScenarioRequest,
    ScenarioRunOutput, UiE2eLocalRequest, UiPlatform,
};
pub use selector::{emit_skip, skip_if_missing_env, skip_if_missing_requirements};
