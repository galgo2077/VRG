#[tokio::main]
async fn main() {
    if let Err(error) = vrg::cli::run().await {
        eprintln!("vrg: {error}");
        std::process::exit(2);
    }
}
