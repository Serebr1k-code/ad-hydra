use clap::Parser;
use std::path::PathBuf;
#[derive(Parser)]
#[command(version, about = "Hydra independent TCP/UDP data plane")]
struct Args {
    #[arg(long)]
    config: PathBuf,
}
#[tokio::main(worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    hydra_proxy::app::run(hydra_proxy::Config::load(&Args::parse().config).await?).await
}
