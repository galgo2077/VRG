#[tokio::main]
async fn main() {
    if let Err(error) = vrg::cli::run().await {
        eprintln!("vrg: {error}");
        std::process::exit(vrg::view::exit_code(&error).unwrap_or(2));
    }
}
