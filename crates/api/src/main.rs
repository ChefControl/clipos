use clap::Parser;
use clipos_api::config::Config;
use clipos_core::telemetry;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Local convenience only; in Azure everything comes from app settings.
    let _ = dotenvy::dotenv();
    let config = Config::parse();
    telemetry::init(config.log_format);
    clipos_api::serve(config, shutdown_signal()).await
}

async fn shutdown_signal() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.ok() };
    #[cfg(unix)]
    let term = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("installing SIGTERM handler")
            .recv()
            .await
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<Option<()>>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}
