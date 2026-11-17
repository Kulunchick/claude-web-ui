pub mod accounts;
pub mod agents_skills_mcp;
pub mod auth;
pub mod chat;
pub mod cron;
pub mod db;
pub mod fakes;
pub mod fs_link;
pub mod kanban;
pub mod messenger;
pub mod traits;
pub mod workspace_files;

use crate::traits::{CliRunner, MessengerTransport};
use axum::Router;
use axum::body::Body;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rust_embed::Embed;
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tower_http::cors::CorsLayer;

#[derive(Embed)]
#[folder = "../ui/dist"]
struct EmbeddedUiAssets;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(String),
}

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub jwt_secret: String,
    pub kanban_mcp_url: String,
    pub access_token_ttl_secs: u64,
    pub refresh_token_ttl_secs: u64,
}

impl GatewayConfig {
    pub fn for_data_dir(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        let db_path = data_dir.join("gateway.sqlite");
        Self {
            data_dir,
            db_path,
            kanban_mcp_url: "http://127.0.0.1:8080".into(),
            jwt_secret: "dev-insecure-default-secret-change-me".to_string(),
            access_token_ttl_secs: 900,
            refresh_token_ttl_secs: 60 * 60 * 24 * 30,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub config: Arc<GatewayConfig>,
    pub cli_runner: Arc<dyn CliRunner>,
    pub messenger: Arc<dyn MessengerTransport>,
    pub scheduler: Arc<chat::ExecutionScheduler>,
    pub kanban_events: tokio::sync::broadcast::Sender<claude_web_ui_shared::kanban::KanbanCardDto>,
}

#[derive(Clone)]
pub struct GatewayServer {
    state: AppState,
    _kanban_worker: Arc<KanbanWorkerHandle>,
}

struct KanbanWorkerHandle(tokio::task::JoinHandle<()>);

impl Drop for KanbanWorkerHandle {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl GatewayServer {
    pub async fn new(
        config: GatewayConfig,
        cli_runner: Arc<dyn CliRunner>,
        messenger: Arc<dyn MessengerTransport>,
    ) -> Result<Self, ServerError> {
        std::fs::create_dir_all(&config.data_dir)?;
        let db = db::connect_sqlite_wal(&config.db_path).await?;
        sqlx::query(
            "UPDATE chat_turns SET status = 'error', error_message = 'Gateway restarted during the Kanban turn'
             WHERE id IN (SELECT turn_id FROM kanban_cards WHERE active_run = 1)
             AND status IN ('queued','running','waiting_approval')"
        ).execute(&db).await?;
        let config_arc = Arc::new(config);
        let scheduler = Arc::new(chat::ExecutionScheduler::new(
            db.clone(),
            config_arc.clone(),
            cli_runner.clone(),
        ));
        let state = AppState {
            db,
            config: config_arc,
            cli_runner,
            messenger,
            scheduler,
            kanban_events: tokio::sync::broadcast::channel(128).0,
        };
        let worker = kanban::start(state.clone());
        Ok(Self {
            state,
            _kanban_worker: Arc::new(KanbanWorkerHandle(worker)),
        })
    }

    pub fn router(&self) -> Router {
        Router::new()
            .route("/api/health", get(|| async { "ok" }))
            .merge(auth::router(self.state.clone()))
            .merge(accounts::router(self.state.clone()))
            .merge(workspace_files::router(self.state.clone()))
            .merge(chat::router(self.state.clone()))
            .merge(kanban::router(self.state.clone()))
            .merge(agents_skills_mcp::router(self.state.clone()))
            .fallback(serve_embedded_ui)
            .layer(CorsLayer::permissive())
            .with_state(self.state.clone())
    }

    pub fn db(&self) -> &SqlitePool {
        &self.state.db
    }

    pub fn config(&self) -> &GatewayConfig {
        &self.state.config
    }

    pub fn cli_runner(&self) -> &Arc<dyn CliRunner> {
        &self.state.cli_runner
    }

    pub fn messenger(&self) -> &Arc<dyn MessengerTransport> {
        &self.state.messenger
    }

    pub fn subscribe_kanban(
        &self,
    ) -> tokio::sync::broadcast::Receiver<claude_web_ui_shared::kanban::KanbanCardDto> {
        self.state.kanban_events.subscribe()
    }

    pub fn scheduler(&self) -> &Arc<chat::ExecutionScheduler> {
        &self.state.scheduler
    }
}

async fn serve_embedded_ui(uri: Uri) -> Response {
    let raw_path = uri.path().trim_start_matches('/');
    if raw_path.starts_with("api/") || raw_path == "api" {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    let candidate = if raw_path.is_empty() {
        "index.html"
    } else {
        raw_path
    };

    let (asset_path, asset) = match EmbeddedUiAssets::get(candidate) {
        Some(file) => (candidate, file),
        None => match EmbeddedUiAssets::get("index.html") {
            Some(index) => ("index.html", index),
            None => return (StatusCode::NOT_FOUND, "UI assets missing").into_response(),
        },
    };

    let mime = mime_guess::from_path(asset_path).first_or_octet_stream();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(Body::from(asset.data.into_owned()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
