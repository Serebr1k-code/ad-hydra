use crate::{Config, grpc::Rpc, security, state::State, telemetry::Telemetry, transport::Manager};
use hydra_wire::v1::{
    proxy_control_server::ProxyControlServer, proxy_diagnostics_server::ProxyDiagnosticsServer,
    proxy_telemetry_server::ProxyTelemetryServer,
};
use tokio_util::sync::CancellationToken;

pub async fn run(config: Config) -> anyhow::Result<()> {
    let shutdown = CancellationToken::new();
    // Acquire the exclusive management socket before opening shared state stores.
    let listener = security::bind(&config.socket, config.socket_mode).await?;
    let telemetry = Telemetry::new(config.limits.max_flows);
    if let Some(pattern) = config.flag_pattern.as_ref() {
        *telemetry.scanner.write().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(
            crate::inspection::flags::Scanner::new(pattern)?,
        ));
    }
    let spool = crate::telemetry::critical::CriticalSpool::open(
        config
            .state_file
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("critical"),
    )?;
    tracing::info!(critical_epoch=%spool.epoch(),"critical spool loaded");
    *telemetry
        .critical
        .write()
        .unwrap_or_else(|e| e.into_inner()) = Some(spool);
    let sample_directory = config
        .state_file
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("samples");
    if !sample_directory.exists() {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&sample_directory)?;
    }
    *telemetry.samples.write().unwrap_or_else(|e| e.into_inner()) =
        Some(crate::samples::SampleRing::open(sample_directory).await?);
    let history_directory = config
        .state_file
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("history");
    if !history_directory.exists() {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&history_directory)?;
    }
    *telemetry.history.write().unwrap_or_else(|e| e.into_inner()) =
        Some(crate::history::HistoryStore::open(history_directory).await?);
    let plugin_directory = config
        .state_file
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("protocols");
    let protocols = crate::inspection::protocols::Protocols::open(plugin_directory).await?;
    *telemetry
        .protocols
        .write()
        .unwrap_or_else(|e| e.into_inner()) = Some(protocols.clone());
    *telemetry
        .forwarding
        .write()
        .unwrap_or_else(|e| e.into_inner()) = Some(
        crate::forwarding::Store::open(
            config
                .state_file
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join("forwarding-mappings.jsonl"),
        )
        .await?,
    );
    let manager = Manager::new(config.limits, telemetry.clone(), shutdown.clone());

    let state = State::start(config.state_file, config.snapshot, manager, telemetry).await?;
    let rpc = Rpc {
        diagnostics: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        state: state.clone(),
        protocols,
        shutdown: shutdown.clone(),
    };
    let uid = config
        .allowed_core_uid
        .unwrap_or(nix::unistd::geteuid().as_raw());
    tracing::info!(socket = %config.socket.display(), allowed_core_uid = uid, "proxy ready");
    let signal = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal.cancel();
    });
    let term = shutdown.clone();
    tokio::spawn(async move {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
            term.cancel();
        }
    });
    let result = tonic::transport::Server::builder()
        .concurrency_limit_per_connection(16)
        .max_concurrent_streams(16)
        .add_service(
            ProxyControlServer::new(rpc.clone()).max_decoding_message_size(4 * 1024 * 1024 + 65536),
        )
        .add_service(ProxyTelemetryServer::new(rpc.clone()).max_decoding_message_size(4096))
        .add_service(
            ProxyDiagnosticsServer::new(rpc)
                .max_decoding_message_size(16384)
                .max_encoding_message_size(256 * 1024),
        )
        .serve_with_incoming_shutdown(security::incoming(listener, uid), shutdown.cancelled())
        .await;
    shutdown.cancel();
    state.stop().await;
    let _ = tokio::fs::remove_file(&config.socket).await;
    result?;
    Ok(())
}
