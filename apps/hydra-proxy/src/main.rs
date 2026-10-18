use clap::Parser;
use std::path::PathBuf;
#[derive(Parser)]
#[command(version, about = "Hydra independent TCP/UDP data plane")]
struct Args {
    #[arg(long)]
    config: PathBuf,
}
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let workers = std::thread::available_parallelism()?.get();
    let blocking_workers_limit = workers.min(4);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .max_blocking_threads(blocking_workers_limit)
        .enable_all()
        .build()?;
    tracing::info!(
        async_workers = runtime.metrics().num_workers(),
        blocking_workers_limit,
        "proxy runtime configured"
    );
    runtime.block_on(async {
        hydra_proxy::app::run(hydra_proxy::Config::load(&args.config).await?).await
    })
}
