use anyhow::Context;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os();
    let _program = args.next();
    let request_path = args
        .next()
        .context("usage: pikaci-fulfill-prepared-output <request-path>")?;
    if args.next().is_some() {
        anyhow::bail!("usage: pikaci-fulfill-prepared-output <request-path>");
    }

    let request = pikaci::fulfill_prepared_output_request(std::path::Path::new(&request_path))?;
    println!("request={}", std::path::Path::new(&request_path).display());
    println!("output={}", request.output_name);
    println!("requested_exposures={}", request.requested_exposures.len());
    Ok(())
}
