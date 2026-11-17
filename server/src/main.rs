use claude_web_ui_server::accounts::PortablePtyCliRunner;
use claude_web_ui_server::messenger::transport::BotTransport;
use claude_web_ui_server::{GatewayConfig, GatewayServer};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("mcp-kanban") {
        return claude_web_ui_server::kanban::stdio_bridge(args).await;
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let data_dir = std::env::var("CLAUDE_WEB_UI_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./data"));
    let bind_addr: SocketAddr = std::env::var("CLAUDE_WEB_UI_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
        .parse()?;

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    let local_addr = listener.local_addr()?;
    let loopback = if local_addr.is_ipv4() {
        "127.0.0.1"
    } else {
        "[::1]"
    };
    let mut config = GatewayConfig::for_data_dir(&data_dir);
    let bridge_listener = if local_addr.ip().is_loopback() || local_addr.ip().is_unspecified() {
        None
    } else {
        Some(tokio::net::TcpListener::bind(format!("{loopback}:0")).await?)
    };
    let bridge_port = bridge_listener
        .as_ref()
        .map(|listener| listener.local_addr())
        .transpose()?
        .unwrap_or(local_addr)
        .port();
    config.kanban_mcp_url = std::env::var("CLAUDE_WEB_UI_MCP_URL")
        .unwrap_or_else(|_| format!("http://{loopback}:{bridge_port}"));
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

    let messenger = Arc::new(BotTransport::new(
        std::env::var("CLAUDE_WEB_UI_TELEGRAM_TOKEN").ok(),
        std::env::var("CLAUDE_WEB_UI_DISCORD_TOKEN").ok(),
    ));
    let server = GatewayServer::new(
        config,
        Arc::new(PortablePtyCliRunner::new()),
        messenger.clone(),
    )
    .await?;

    messenger.start(&server);

    tracing::info!("claude-web-ui gateway listening on http://{}", bind_addr);
    if let Some(bridge_listener) = bridge_listener {
        tokio::try_join!(
            async { axum::serve(listener, server.router()).await },
            async { axum::serve(bridge_listener, server.kanban_mcp_router()).await },
        )?;
    } else {
        axum::serve(listener, server.router()).await?;
    }
    Ok(())
}
