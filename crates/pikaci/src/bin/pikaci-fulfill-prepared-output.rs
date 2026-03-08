use anyhow::Context;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os();
    let _program = args.next();
    let mut result_path = None;
    let mut request_path = None;
    while let Some(arg) = args.next() {
        if arg == "--result-path" {
            result_path = Some(args.next().context(
                "usage: pikaci-fulfill-prepared-output [--result-path <path>] <request-path>",
            )?);
            continue;
        }
        if request_path.is_none() {
            request_path = Some(arg);
            continue;
        }
        anyhow::bail!(
            "usage: pikaci-fulfill-prepared-output [--result-path <path>] <request-path>"
        );
    }
    let request_path = request_path
        .context("usage: pikaci-fulfill-prepared-output [--result-path <path>] <request-path>")?;

    let result =
        pikaci::fulfill_prepared_output_request_result(std::path::Path::new(&request_path));
    if let Some(result_path) = result_path {
        let result_path = std::path::Path::new(&result_path);
        pikaci::write_prepared_output_fulfillment_result(result_path, &result)
            .with_context(|| format!("write {}", result_path.display()))?;
    }
    println!("{}", serde_json::to_string(&result)?);
    if result.status == pikaci::PreparedOutputFulfillmentStatus::Succeeded {
        return Ok(());
    }
    anyhow::bail!(
        "{}",
        result
            .error
            .unwrap_or_else(|| "prepared-output fulfillment failed".to_string())
    )
}
