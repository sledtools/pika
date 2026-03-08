use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, exit};

use anyhow::{Context, bail};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os();
    let _program = args.next();
    let request_path = args
        .next()
        .context("usage: pikaci-launch-fulfill-prepared-output <launch-request-path>")?;
    if args.next().is_some() {
        bail!("usage: pikaci-launch-fulfill-prepared-output <launch-request-path>");
    }

    let launch_request =
        pikaci::load_prepared_output_fulfillment_launch_request(Path::new(&request_path))
            .with_context(|| format!("load {}", Path::new(&request_path).display()))?;
    if launch_request.schema_version != 1 {
        bail!(
            "unsupported launch request schema_version {}",
            launch_request.schema_version
        );
    }

    let output = Command::new(&launch_request.helper_program)
        .arg("--result-path")
        .arg(&launch_request.helper_result_path)
        .arg(&launch_request.helper_request_path)
        .output()
        .with_context(|| {
            format!(
                "run helper `{}` for {}",
                launch_request.helper_program, launch_request.helper_request_path
            )
        })?;

    io::stdout()
        .write_all(&output.stdout)
        .context("write helper stdout")?;
    io::stderr()
        .write_all(&output.stderr)
        .context("write helper stderr")?;

    if output.status.success() {
        return Ok(());
    }

    exit(output.status.code().unwrap_or(1));
}
