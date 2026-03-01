use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, bail};

use crate::config;
use crate::testing::{ArtifactPolicy, CommandRunner, CommandSpec, TestContext};

use super::artifacts::{self, CommandOutcomeRecord};
use super::types::ScenarioRunOutput;

const ENV_PIKA_TEST_NSEC: &str = "PIKA_TEST_NSEC";
const ENV_PIKA_UI_E2E_BOT_NPUB: &str = "PIKA_UI_E2E_BOT_NPUB";
const ENV_PIKA_UI_E2E_RELAYS: &str = "PIKA_UI_E2E_RELAYS";
const ENV_PIKA_UI_E2E_KP_RELAYS: &str = "PIKA_UI_E2E_KP_RELAYS";
const ENV_PIKA_UI_E2E_NSEC: &str = "PIKA_UI_E2E_NSEC";

static DOTENV_DEFAULTS: OnceLock<HashMap<String, String>> = OnceLock::new();

#[derive(Debug, Clone, Copy)]
pub enum PublicUiPlatform {
    Android,
    Ios,
    All,
}

pub fn optional_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| dotenv_defaults().get(name).cloned())
}

pub fn run_public_ui_e2e(platform: PublicUiPlatform) -> Result<ScenarioRunOutput> {
    let run_name = match platform {
        PublicUiPlatform::Android => "ui-e2e-public-android",
        PublicUiPlatform::Ios => "ui-e2e-public-ios",
        PublicUiPlatform::All => "ui-e2e-public-all",
    };

    let mut context = TestContext::builder(run_name)
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    let root = config::find_workspace_root()?;
    let runner = CommandRunner::new(&context);
    let mut outcomes = Vec::new();

    match platform {
        PublicUiPlatform::Android => {
            outcomes.extend(run_public_android(&runner, &root)?);
        }
        PublicUiPlatform::Ios => {
            outcomes.extend(run_public_ios(&runner, &root)?);
        }
        PublicUiPlatform::All => {
            outcomes.extend(run_public_ios(&runner, &root)?);
            outcomes.extend(run_public_android(&runner, &root)?);
        }
    }

    let mut result = ScenarioRunOutput::completed(context.state_dir().to_path_buf())
        .with_metadata("platform", format!("{:?}", platform));
    let summary = artifacts::write_standard_summary(
        &context,
        "public::ui_e2e",
        &result,
        outcomes,
        BTreeMap::new(),
    )?;
    result = result.with_summary(summary);

    context.mark_success();
    Ok(result)
}

pub fn run_deployed_bot_call_flow() -> Result<ScenarioRunOutput> {
    let mut context = TestContext::builder("deployed-bot-call-flow")
        .artifact_policy(ArtifactPolicy::PreserveOnFailure)
        .build()?;
    let root = config::find_workspace_root()?;
    let runner = CommandRunner::new(&context);

    let command = runner.run(
        &CommandSpec::cargo()
            .cwd(&root)
            .args([
                "test",
                "-p",
                "pika_core",
                "--test",
                "e2e_calls",
                "call_deployed_bot",
                "--",
                "--ignored",
                "--nocapture",
            ])
            .capture_name("deployed-bot-call-flow"),
    )?;

    let mut result = ScenarioRunOutput::completed(context.state_dir().to_path_buf())
        .with_artifact(command.stdout_path.clone())
        .with_artifact(command.stderr_path.clone());
    let summary = artifacts::write_standard_summary(
        &context,
        "public::deployed_bot_call",
        &result,
        vec![CommandOutcomeRecord::from_output(
            "deployed-bot-call-flow",
            &command,
        )],
        BTreeMap::new(),
    )?;
    result = result.with_summary(summary);

    context.mark_success();
    Ok(result)
}

fn run_public_android(
    runner: &CommandRunner<'_>,
    root: &Path,
) -> Result<Vec<CommandOutcomeRecord>> {
    let peer = required_env(ENV_PIKA_UI_E2E_BOT_NPUB)?;
    let relays = required_env(ENV_PIKA_UI_E2E_RELAYS)?;
    let kp_relays = required_env(ENV_PIKA_UI_E2E_KP_RELAYS)?;
    let nsec = optional_env(ENV_PIKA_UI_E2E_NSEC)
        .or_else(|| optional_env(ENV_PIKA_TEST_NSEC))
        .ok_or_else(|| anyhow!("missing {ENV_PIKA_UI_E2E_NSEC} and {ENV_PIKA_TEST_NSEC}"))?;

    let test_suffix = optional_env("PIKA_ANDROID_TEST_APPLICATION_ID_SUFFIX")
        .unwrap_or_else(|| ".test".to_string());
    let test_app_id = format!("org.pikachat.pika{test_suffix}");

    let mut outcomes = Vec::new();

    if optional_env("PIKA_ANDROID_SERIAL").is_none() {
        let emulator = runner.run(
            &CommandSpec::new("./tools/android-emulator-ensure")
                .cwd(root)
                .capture_name("android-emulator-ensure"),
        )?;
        outcomes.push(CommandOutcomeRecord::from_output(
            "android-emulator-ensure",
            &emulator,
        ));
    }

    let prepare = runner.run(
        &CommandSpec::new("just")
            .cwd(root)
            .args(["gen-kotlin", "android-rust", "android-local-properties"])
            .capture_name("android-prepare-build"),
    )?;
    outcomes.push(CommandOutcomeRecord::from_output(
        "android-prepare-build",
        &prepare,
    ));

    let installable = runner.run(
        &CommandSpec::new("./tools/android-ensure-debug-installable")
            .cwd(root)
            .env("PIKA_ANDROID_APP_ID", &test_app_id)
            .capture_name("android-ensure-installable"),
    )?;
    outcomes.push(CommandOutcomeRecord::from_output(
        "android-ensure-installable",
        &installable,
    ));

    let serial_output = runner.run(
        &CommandSpec::new("./tools/android-pick-serial")
            .cwd(root)
            .capture_name("android-pick-serial"),
    )?;
    outcomes.push(CommandOutcomeRecord::from_output(
        "android-pick-serial",
        &serial_output,
    ));

    let serial = String::from_utf8_lossy(&serial_output.stdout)
        .trim()
        .to_string();
    if serial.is_empty() {
        bail!("android serial output was empty");
    }

    if !serial.starts_with("emulator-") {
        let unlock = runner.run(
            &CommandSpec::new("./tools/android-ensure-unlocked")
                .cwd(root)
                .arg(serial.clone())
                .capture_name("android-ensure-unlocked"),
        )?;
        outcomes.push(CommandOutcomeRecord::from_output(
            "android-ensure-unlocked",
            &unlock,
        ));
    }

    let ui = runner.run(
        &CommandSpec::gradlew()
            .cwd(root.join("android"))
            .env("ANDROID_SERIAL", serial)
            .arg(":app:connectedDebugAndroidTest")
            .arg(format!("-PPIKA_ANDROID_APPLICATION_ID_SUFFIX={test_suffix}"))
            .arg("-Pandroid.testInstrumentationRunnerArguments.class=com.pika.app.PikaE2eUiTest")
            .arg("-Pandroid.testInstrumentationRunnerArguments.pika_e2e=1")
            .arg("-Pandroid.testInstrumentationRunnerArguments.pika_disable_network=false")
            .arg("-Pandroid.testInstrumentationRunnerArguments.pika_reset=1")
            .arg(format!("-Pandroid.testInstrumentationRunnerArguments.pika_peer_npub={peer}"))
            .arg(format!("-Pandroid.testInstrumentationRunnerArguments.pika_relay_urls={relays}"))
            .arg(format!("-Pandroid.testInstrumentationRunnerArguments.pika_key_package_relay_urls={kp_relays}"))
            .arg(format!("-Pandroid.testInstrumentationRunnerArguments.pika_nsec={nsec}"))
            .capture_name("android-ui-e2e-public"),
    )?;
    outcomes.push(CommandOutcomeRecord::from_output(
        "android-ui-e2e-public",
        &ui,
    ));

    Ok(outcomes)
}

fn run_public_ios(runner: &CommandRunner<'_>, root: &Path) -> Result<Vec<CommandOutcomeRecord>> {
    let peer = required_env(ENV_PIKA_UI_E2E_BOT_NPUB)?;
    let relays = required_env(ENV_PIKA_UI_E2E_RELAYS)?;
    let kp_relays = required_env(ENV_PIKA_UI_E2E_KP_RELAYS)?;
    let nsec = optional_env(ENV_PIKA_UI_E2E_NSEC)
        .or_else(|| optional_env(ENV_PIKA_TEST_NSEC))
        .ok_or_else(|| anyhow!("missing {ENV_PIKA_UI_E2E_NSEC} and {ENV_PIKA_TEST_NSEC}"))?;

    let mut outcomes = Vec::new();

    let prepare = runner.run(
        &CommandSpec::new("just")
            .cwd(root)
            .args(["ios-xcframework", "ios-xcodeproj"])
            .capture_name("ios-prepare-build"),
    )?;
    outcomes.push(CommandOutcomeRecord::from_output(
        "ios-prepare-build",
        &prepare,
    ));

    let sim_output = runner.run(
        &CommandSpec::new("./tools/ios-sim-ensure")
            .cwd(root)
            .env(ENV_PIKA_UI_E2E_NSEC, &nsec)
            .capture_name("ios-sim-ensure-public"),
    )?;
    outcomes.push(CommandOutcomeRecord::from_output(
        "ios-sim-ensure-public",
        &sim_output,
    ));
    let sim_stdout = String::from_utf8_lossy(&sim_output.stdout);
    let udid = parse_udid(&sim_stdout)
        .ok_or_else(|| anyhow!("could not determine simulator udid from ios-sim-ensure"))?;

    let ios_ui = runner.run(
        &CommandSpec::new("./tools/xcode-run")
            .cwd(root)
            .env("PIKA_UI_E2E", "1")
            .arg("xcodebuild")
            .args(["-project", "ios/Pika.xcodeproj", "-scheme", "Pika"])
            .arg("-destination")
            .arg(format!("id={udid}"))
            .arg("test")
            .arg("CODE_SIGNING_ALLOWED=NO")
            .arg(format!("PIKA_UI_E2E_BOT_NPUB={peer}"))
            .arg(format!("PIKA_UI_E2E_RELAYS={relays}"))
            .arg(format!("PIKA_UI_E2E_KP_RELAYS={kp_relays}"))
            .arg(format!("PIKA_UI_E2E_NSEC={nsec}"))
            .arg(format!(
                "PIKA_APP_BUNDLE_ID={}",
                optional_env("PIKA_IOS_BUNDLE_ID")
                    .unwrap_or_else(|| "org.pikachat.pika.dev".to_string())
            ))
            .arg("-only-testing:PikaUITests/PikaUITests/testE2E_deployedRustBot_pingPong")
            .capture_name("ios-ui-e2e-public"),
    )?;
    outcomes.push(CommandOutcomeRecord::from_output(
        "ios-ui-e2e-public",
        &ios_ui,
    ));

    Ok(outcomes)
}

fn required_env(name: &str) -> Result<String> {
    if let Some(value) = optional_env(name) {
        return Ok(value);
    }
    bail!("missing required env: {name}");
}

fn dotenv_defaults() -> &'static HashMap<String, String> {
    DOTENV_DEFAULTS.get_or_init(|| {
        load_dotenv_defaults(&config::find_workspace_root().unwrap_or_else(|_| PathBuf::from(".")))
            .unwrap_or_default()
    })
}

fn load_dotenv_defaults(root: &Path) -> Result<HashMap<String, String>> {
    let mut defaults = HashMap::new();

    for file_name in [".env", ".env.local"] {
        let path = root.join(file_name);
        if !path.is_file() {
            continue;
        }
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some((raw_key, raw_value)) = trimmed.split_once('=') else {
                continue;
            };

            let raw_key = raw_key.trim();
            let key = raw_key
                .strip_prefix("export")
                .map(str::trim_start)
                .unwrap_or(raw_key)
                .trim();
            if key.is_empty() || std::env::var_os(key).is_some() {
                continue;
            }

            let value = parse_dotenv_value(raw_value.trim());
            if value.is_empty() {
                continue;
            }

            defaults.insert(key.to_string(), value);
        }
    }

    Ok(defaults)
}

fn parse_dotenv_value(raw: &str) -> String {
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        return raw[1..raw.len() - 1].to_string();
    }
    if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
        return raw[1..raw.len() - 1].to_string();
    }
    raw.to_string()
}

fn parse_udid(output: &str) -> Option<String> {
    for line in output.lines() {
        let prefix = "ok: ios simulator ready (udid=";
        if let Some(rest) = line.strip_prefix(prefix)
            && let Some(udid) = rest.strip_suffix(')')
        {
            return Some(udid.to_string());
        }
    }
    None
}
