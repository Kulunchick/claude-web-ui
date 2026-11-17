use claude_web_ui_server::fakes::{FakeCliRunner, FakeMessengerTransport};
use claude_web_ui_server::{GatewayConfig, GatewayServer};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let data_dir = std::env::var("CLAUDE_WEB_UI_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./data"));
    let bind_addr: SocketAddr = std::env::var("CLAUDE_WEB_UI_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
        .parse()?;

    let mut config = GatewayConfig::for_data_dir(&data_dir);
    if let Ok(secret) = std::env::var("CLAUDE_WEB_UI_JWT_SECRET") {
        config.jwt_secret = secret;
    } else {
        let secret_file = data_dir.join("jwt_secret");
        std::fs::create_dir_all(&data_dir)?;
        config.jwt_secret = if secret_file.exists() {
            std::fs::read_to_string(&secret_file)?.trim().to_string()
        } else {
            let generated = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
            std::fs::write(&secret_file, &generated)?;
            generated
        };
    }

    let server = GatewayServer::new(
        config,
        Arc::new(FakeCliRunner::new()),
        Arc::new(FakeMessengerTransport::new()),
    )
    .await?;

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!("claude-web-ui gateway listening on http://{}", bind_addr);
    axum::serve(listener, server.router()).await?;
    Ok(())
}
