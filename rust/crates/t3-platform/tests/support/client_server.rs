#[tokio::main]
async fn main() -> std::process::ExitCode {
    let result = match t3_server::cli::parse(std::env::args_os().skip(1)) {
        Ok(invocation) => t3_server::cli::run(invocation).await,
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
