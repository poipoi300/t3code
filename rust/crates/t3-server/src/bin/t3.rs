#[tokio::main]
async fn main() -> std::process::ExitCode {
    let result = match t3_server::cli::parse(std::env::args_os().skip(1)) {
        Ok(invocation) => {
            if !matches!(&invocation, t3_server::cli::Invocation::Serve(_)) {
                let filter = tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
                let _ = tracing_subscriber::fmt().with_writer(std::io::stderr)
                    .with_env_filter(filter).try_init();
            }
            t3_server::cli::run(invocation).await
        }
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::from(error.exit_code())
        }
    }
}
