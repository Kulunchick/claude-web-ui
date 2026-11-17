use crate::auth::{
    project_rbac_middleware, record_audit_log, slugify, AdminUser, AppError, ProjectContext,
};
use crate::traits::{CliInvocation, CliRunner, PtyInvocation, PtySession, SpawnedPty, TurnEvent};
use crate::{fs_link, AppState};
use async_trait::async_trait;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::Response;
use axum::routing::{delete, get};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use claude_web_ui_shared::accounts::{
    AccountAclDto, AccountSummaryDto, CliAccountDto, CliProvider, CreateAccountRequest,
    CredentialFileDto, ProjectCliStateDto, SwitchProjectCliRequest, UpdateAccountAclRequest,
    UpdateAccountRequest, UploadCredentialRequest,
};
use claude_web_ui_shared::auth::Role;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Deserialize;
use serde_json::json;
use sqlx::{Row, SqlitePool};
use std::collections::{BTreeMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path as StdPath, PathBuf};
use std::sync::Mutex;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct ListAccountsQuery {
    pub provider: Option<CliProvider>,
}

#[derive(Debug, Deserialize)]
pub struct PtyWsQuery {
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}

pub fn router(state: AppState) -> Router<AppState> {
    let project_scoped = Router::new()
        .route(
            "/api/projects/{project_id}/cli",
            get(get_project_cli_state).put(switch_project_cli),
        )
        .layer(middleware::from_fn_with_state(
            state,
            project_rbac_middleware,
        ));

    Router::new()
        .route("/api/admin/accounts", get(list_accounts).post(create_account))
        .route(
            "/api/admin/accounts/{account_id}",
            get(get_account).patch(update_account).delete(delete_account),
        )
        .route(
            "/api/admin/accounts/{account_id}/credentials",
            get(list_credentials).post(upload_credential),
        )
        .route(
            "/api/admin/accounts/{account_id}/credentials/{filename}",
            delete(delete_credential),
        )
        .route(
            "/api/admin/accounts/{account_id}/login-pty",
            get(login_pty_ws),
        )
        .route(
            "/api/admin/projects/{project_id}/members/{user_id}/accounts",
            get(get_member_account_acl).put(update_member_account_acl),
        )
        .route(
            "/api/admin/projects/{project_id}/account-acls",
            get(list_project_account_acls),
        )
        .merge(project_scoped)
}

/// Returns the isolated environment variables for spawning a CLI process or login PTY
/// under the given account profile directory.
pub fn account_env_vars(provider: CliProvider, profile_dir: &StdPath) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert(
        provider.config_env_var().to_string(),
        profile_dir.to_string_lossy().into_owned(),
    );
    env
}

fn ensure_account_dirs(
    data_dir: &StdPath,
    provider: CliProvider,
    slug: &str,
) -> Result<PathBuf, AppError> {
    let shared_root = data_dir.join("shared").join(provider.as_str());
    for sub in CliProvider::SHARED_SUBDIRS {
        std::fs::create_dir_all(shared_root.join(sub))
            .map_err(|e| AppError::internal(format!("failed to create shared dir: {e}")))?;
    }

    let profile_dir = data_dir.join("accounts").join(provider.as_str()).join(slug);
    std::fs::create_dir_all(&profile_dir)
        .map_err(|e| AppError::internal(format!("failed to create profile dir: {e}")))?;

    for sub in CliProvider::SHARED_SUBDIRS {
        let link = profile_dir.join(sub);
        if std::fs::symlink_metadata(&link).is_err() {
            fs_link::link_dir(&shared_root.join(sub), &link)
                .map_err(|e| AppError::internal(format!("failed to link shared {sub}: {e}")))?;
        }
    }

    Ok(profile_dir)
}

fn remove_account_dirs(profile_dir: &StdPath) -> Result<(), AppError> {
    if !profile_dir.exists() {
        return Ok(());
    }
    for sub in CliProvider::SHARED_SUBDIRS {
        fs_link::unlink_dir(&profile_dir.join(sub)).map_err(|e| {
            AppError::internal(format!("failed to unlink shared dir {sub}: {e}"))
        })?;
    }
    std::fs::remove_dir_all(profile_dir)
        .map_err(|e| AppError::internal(format!("failed to remove profile dir: {e}")))?;
    Ok(())
}

fn list_credential_filenames(profile_dir: &StdPath) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(profile_dir) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if meta.is_file() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    names
}

fn row_to_account(row: &sqlx::sqlite::SqliteRow) -> Result<CliAccountDto, AppError> {
    let provider_str: String = row.try_get("provider")?;
    let provider: CliProvider = provider_str
        .parse()
        .map_err(|e: String| AppError::internal(format!("corrupt provider in db: {e}")))?;
    let profile_dir: String = row.try_get("profile_dir")?;
    let credential_files = list_credential_filenames(StdPath::new(&profile_dir));
    let has_credentials = !credential_files.is_empty();

    Ok(CliAccountDto {
        id: row.try_get("id")?,
        provider,
        name: row.try_get("name")?,
        slug: row.try_get("slug")?,
        profile_dir,
        credential_files,
        has_credentials,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn load_account(state: &AppState, account_id: &str) -> Result<CliAccountDto, AppError> {
    let row = sqlx::query(
        "SELECT id, provider, name, slug, profile_dir, created_at, updated_at FROM cli_accounts WHERE id = ?",
    )
    .bind(account_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::not_found("account not found"))?;
    row_to_account(&row)
}

async fn list_accounts(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(query): Query<ListAccountsQuery>,
) -> Result<Json<Vec<CliAccountDto>>, AppError> {
    let rows = match query.provider {
        Some(provider) => {
            sqlx::query(
                "SELECT id, provider, name, slug, profile_dir, created_at, updated_at FROM cli_accounts WHERE provider = ? ORDER BY created_at ASC",
            )
            .bind(provider.as_str())
            .fetch_all(&state.db)
            .await?
        }
        None => {
            sqlx::query(
                "SELECT id, provider, name, slug, profile_dir, created_at, updated_at FROM cli_accounts ORDER BY created_at ASC",
            )
            .fetch_all(&state.db)
            .await?
        }
    };

    let accounts: Result<Vec<CliAccountDto>, AppError> = rows.iter().map(row_to_account).collect();
    Ok(Json(accounts?))
}

async fn get_account(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(account_id): Path<String>,
) -> Result<Json<CliAccountDto>, AppError> {
    Ok(Json(load_account(&state, &account_id).await?))
}

async fn create_account(
    State(state): State<AppState>,
    admin: AdminUser,
    Json(req): Json<CreateAccountRequest>,
) -> Result<(StatusCode, Json<CliAccountDto>), AppError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("account name is required"));
    }

    let slug = slugify(name);
    let existing: Option<String> =
        sqlx::query_scalar("SELECT id FROM cli_accounts WHERE provider = ? AND slug = ?")
            .bind(req.provider.as_str())
            .bind(&slug)
            .fetch_optional(&state.db)
            .await?;
    if existing.is_some() {
        return Err(AppError::conflict(format!(
            "account with slug '{slug}' already exists for provider {}",
            req.provider.display_name()
        )));
    }

    let profile_dir = ensure_account_dirs(&state.config.data_dir, req.provider, &slug)?;
    let profile_dir_str = profile_dir.to_string_lossy().into_owned();
    let account_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();

    if let Err(err) = sqlx::query(
        "INSERT INTO cli_accounts (id, provider, name, slug, profile_dir, created_by, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&account_id)
    .bind(req.provider.as_str())
    .bind(name)
    .bind(&slug)
    .bind(&profile_dir_str)
    .bind(&admin.0.user.id)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await
    {
        let _ = remove_account_dirs(&profile_dir);
        return Err(err.into());
    }

    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "create_cli_account",
        Some("cli_account"),
        Some(&account_id),
        Some(req.provider.as_str()),
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(CliAccountDto {
            id: account_id,
            provider: req.provider,
            name: name.to_string(),
            slug,
            profile_dir: profile_dir_str,
            credential_files: Vec::new(),
            has_credentials: false,
            created_at: now.clone(),
            updated_at: now,
        }),
    ))
}

async fn update_account(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(account_id): Path<String>,
    Json(req): Json<UpdateAccountRequest>,
) -> Result<Json<CliAccountDto>, AppError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("account name cannot be empty"));
    }

    let _existing = load_account(&state, &account_id).await?;
    let now = Utc::now().to_rfc3339();

    // Keep slug and profile_dir stable on rename so existing credentials and active env paths do not break.
    sqlx::query("UPDATE cli_accounts SET name = ?, updated_at = ? WHERE id = ?")
        .bind(name)
        .bind(&now)
        .bind(&account_id)
        .execute(&state.db)
        .await?;

    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "rename_cli_account",
        Some("cli_account"),
        Some(&account_id),
        Some(name),
    )
    .await?;

    Ok(Json(load_account(&state, &account_id).await?))
}

async fn delete_account(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(account_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let existing = load_account(&state, &account_id).await?;
    remove_account_dirs(StdPath::new(&existing.profile_dir))?;

    sqlx::query("DELETE FROM cli_accounts WHERE id = ?")
        .bind(&account_id)
        .execute(&state.db)
        .await?;

    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "delete_cli_account",
        Some("cli_account"),
        Some(&account_id),
        Some(&existing.slug),
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

fn validate_credential_filename(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed != raw {
        return Err(AppError::bad_request("invalid credential filename"));
    }
    if trimmed == "." || trimmed == ".." {
        return Err(AppError::bad_request("invalid credential filename"));
    }
    if trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains(':')
        || trimmed.contains('\0')
    {
        return Err(AppError::bad_request(
            "credential filename must be a plain file name without path separators",
        ));
    }
    if CliProvider::SHARED_SUBDIRS.contains(&trimmed) {
        return Err(AppError::bad_request(
            "credential filename cannot collide with shared provider directories",
        ));
    }
    Ok(trimmed.to_string())
}

fn resolve_credential_path(profile_dir: &StdPath, filename: &str) -> Result<PathBuf, AppError> {
    let safe_name = validate_credential_filename(filename)?;
    let canonical_root = std::fs::canonicalize(profile_dir)
        .map_err(|e| AppError::internal(format!("account profile dir unavailable: {e}")))?;
    let target = canonical_root.join(&safe_name);
    if target.parent() != Some(canonical_root.as_path()) {
        return Err(AppError::bad_request("invalid credential path"));
    }
    if let Ok(meta) = std::fs::symlink_metadata(&target)
        && (meta.file_type().is_symlink() || meta.is_dir())
    {
        return Err(AppError::bad_request(
            "target path is a directory or symlink",
        ));
    }
    Ok(target)
}

fn file_metadata_to_dto(filename: String, meta: &std::fs::Metadata) -> CredentialFileDto {
    let updated_at = meta
        .modified()
        .map(|t| DateTime::<Utc>::from(t).to_rfc3339())
        .unwrap_or_else(|_| Utc::now().to_rfc3339());
    CredentialFileDto {
        filename,
        size_bytes: meta.len(),
        updated_at,
    }
}

async fn list_credentials(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(account_id): Path<String>,
) -> Result<Json<Vec<CredentialFileDto>>, AppError> {
    let account = load_account(&state, &account_id).await?;
    let profile_dir = StdPath::new(&account.profile_dir);
    let mut files = Vec::new();
    for name in list_credential_filenames(profile_dir) {
        if let Ok(meta) = std::fs::symlink_metadata(profile_dir.join(&name))
            && meta.is_file()
        {
            files.push(file_metadata_to_dto(name, &meta));
        }
    }
    Ok(Json(files))
}

async fn upload_credential(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(account_id): Path<String>,
    Json(req): Json<UploadCredentialRequest>,
) -> Result<Json<CredentialFileDto>, AppError> {
    let account = load_account(&state, &account_id).await?;
    let target = resolve_credential_path(StdPath::new(&account.profile_dir), &req.filename)?;
    let filename = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| AppError::bad_request("invalid credential filename"))?;

    std::fs::write(&target, req.content.as_bytes())
        .map_err(|e| AppError::internal(format!("failed to write credential file: {e}")))?;

    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE cli_accounts SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&account_id)
        .execute(&state.db)
        .await?;

    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "upload_cli_credential",
        Some("cli_account"),
        Some(&account_id),
        Some(&filename),
    )
    .await?;

    let meta = std::fs::symlink_metadata(&target)
        .map_err(|e| AppError::internal(format!("failed to stat credential file: {e}")))?;
    Ok(Json(file_metadata_to_dto(filename, &meta)))
}

async fn delete_credential(
    State(state): State<AppState>,
    admin: AdminUser,
    Path((account_id, filename)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let account = load_account(&state, &account_id).await?;
    let target = resolve_credential_path(StdPath::new(&account.profile_dir), &filename)?;
    if !target.exists() {
        return Err(AppError::not_found("credential file not found"));
    }
    std::fs::remove_file(&target)
        .map_err(|e| AppError::internal(format!("failed to delete credential file: {e}")))?;

    let now = Utc::now().to_rfc3339();
    sqlx::query("UPDATE cli_accounts SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&account_id)
        .execute(&state.db)
        .await?;

    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "delete_cli_credential",
        Some("cli_account"),
        Some(&account_id),
        Some(&filename),
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PtyClientFrame {
    Input { data: String },
    Resize { cols: u16, rows: u16 },
}

async fn login_pty_ws(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(account_id): Path<String>,
    Query(query): Query<PtyWsQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let account = load_account(&state, &account_id).await?;
    let cols = query.cols.unwrap_or(80).clamp(20, 400);
    let rows = query.rows.unwrap_or(24).clamp(8, 200);

    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "launch_oauth_login_pty",
        Some("cli_account"),
        Some(&account_id),
        Some(account.provider.as_str()),
    )
    .await?;

    Ok(ws.on_upgrade(move |socket| handle_pty_socket(socket, state, account, cols, rows)))
}

async fn handle_pty_socket(
    socket: WebSocket,
    state: AppState,
    account: CliAccountDto,
    cols: u16,
    rows: u16,
) {
    let (program, args) = account.provider.login_command();
    let working_dir = PathBuf::from(&account.profile_dir);
    let env = account_env_vars(account.provider, &working_dir);
    let invocation = PtyInvocation {
        provider: account.provider,
        program: program.to_string(),
        args: args.iter().map(|s| (*s).to_string()).collect(),
        working_dir,
        env,
        cols,
        rows,
    };

    bridge_pty_websocket(socket, state, invocation).await;
}

/// Bridges an authenticated WebSocket upgrade to `state.cli_runner.spawn_pty`.
pub(crate) async fn bridge_pty_websocket(
    mut socket: WebSocket,
    state: AppState,
    invocation: PtyInvocation,
) {
    let mut spawned = match state.cli_runner.spawn_pty(invocation).await {
        Ok(s) => s,
        Err(err) => {
            let _ = socket
                .send(WsMessage::Text(
                    json!({ "type": "error", "message": err }).to_string().into(),
                ))
                .await;
            let _ = socket.send(WsMessage::Close(None)).await;
            return;
        }
    };

    let mut exit_rx = spawned.exit_rx;

    loop {
        tokio::select! {
            maybe_bytes = spawned.output_rx.recv() => {
                match maybe_bytes {
                    Some(bytes) => {
                        let data = String::from_utf8_lossy(&bytes).into_owned();
                        if socket
                            .send(WsMessage::Text(json!({ "type": "output", "data": data }).to_string().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    None => break,
                }
            }
            exit_res = &mut exit_rx => {
                let code = exit_res.unwrap_or(-1);
                while let Ok(bytes) = spawned.output_rx.try_recv() {
                    let data = String::from_utf8_lossy(&bytes).into_owned();
                    let _ = socket
                        .send(WsMessage::Text(json!({ "type": "output", "data": data }).to_string().into()))
                        .await;
                }
                let _ = socket
                    .send(WsMessage::Text(json!({ "type": "exit", "code": code }).to_string().into()))
                    .await;
                break;
            }
            maybe_msg = socket.recv() => {
                match maybe_msg {
                    Some(Ok(WsMessage::Text(text))) => {
                        if let Ok(frame) = serde_json::from_str::<PtyClientFrame>(&text) {
                            match frame {
                                PtyClientFrame::Input { data } => {
                                    if spawned.session.write_input(data.as_bytes()).await.is_err() {
                                        break;
                                    }
                                }
                                PtyClientFrame::Resize { cols, rows } => {
                                    let _ = spawned.session.resize(cols.clamp(20, 400), rows.clamp(8, 200)).await;
                                }
                            }
                        } else if spawned.session.write_input(text.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Binary(bin))) => {
                        if spawned.session.write_input(&bin).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                }
            }
        }
    }

    let _ = spawned.session.close().await;
    let _ = socket.send(WsMessage::Close(None)).await;
}

/// Production `CliRunner` PTY implementation backed by `portable-pty`.
#[derive(Debug, Default)]
pub struct PortablePtyCliRunner;

impl PortablePtyCliRunner {
    pub fn new() -> Self {
        Self
    }
}

struct PortablePtySession {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

#[async_trait]
impl PtySession for PortablePtySession {
    async fn write_input(&self, data: &[u8]) -> Result<(), String> {
        let mut guard = self
            .writer
            .lock()
            .map_err(|_| "pty writer lock poisoned".to_string())?;
        guard
            .write_all(data)
            .and_then(|_| guard.flush())
            .map_err(|e| format!("pty write failed: {e}"))
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        let guard = self
            .master
            .lock()
            .map_err(|_| "pty master lock poisoned".to_string())?;
        guard
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("pty resize failed: {e}"))
    }

    async fn close(&self) -> Result<(), String> {
        let mut guard = self
            .killer
            .lock()
            .map_err(|_| "pty killer lock poisoned".to_string())?;
        let _ = guard.kill();
        Ok(())
    }
}

pub fn spawn_portable_pty(invocation: PtyInvocation) -> Result<SpawnedPty, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: invocation.rows,
            cols: invocation.cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty failed: {e}"))?;

    let mut cmd = CommandBuilder::new(&invocation.program);
    cmd.args(&invocation.args);
    cmd.cwd(&invocation.working_dir);
    for (key, val) in &invocation.env {
        cmd.env(key, val);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn {} failed: {e}", invocation.program))?;
    drop(pair.slave);

    let killer = child.clone_killer();
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("pty clone reader failed: {e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("pty take writer failed: {e}"))?;

    let (output_tx, output_rx) = mpsc::unbounded_channel();
    let (exit_tx, exit_rx) = oneshot::channel();

    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if output_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
        let code = child
            .wait()
            .map(|status| status.exit_code() as i32)
            .unwrap_or(-1);
        let _ = exit_tx.send(code);
    });

    Ok(SpawnedPty {
        session: Box::new(PortablePtySession {
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
        }),
        output_rx,
        exit_rx,
    })
}

#[async_trait]
impl CliRunner for PortablePtyCliRunner {
    async fn run_turn(
        &self,
        invocation: CliInvocation,
        event_tx: mpsc::UnboundedSender<TurnEvent>,
        approval_rx: mpsc::UnboundedReceiver<bool>,
    ) -> Result<(), String> {
        crate::chat::run_native_cli_turn(invocation, event_tx, approval_rx).await
    }

    async fn spawn_pty(&self, invocation: PtyInvocation) -> Result<SpawnedPty, String> {
        spawn_portable_pty(invocation)
    }
}

pub(crate) async fn authorize_turn_account(
    db: &SqlitePool,
    project_id: &str,
    user_id: &str,
    account_id: Option<&str>,
) -> Result<(), AppError> {
    let role: Option<String> = sqlx::query_scalar("SELECT role FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(db)
        .await?;
    let role: Role = role.ok_or_else(|| AppError::forbidden("user no longer exists"))?
        .parse().map_err(|error: String| AppError::internal(error))?;
    let effective_role = crate::auth::project_role(db, user_id, role, project_id, true).await?;
    if effective_role != Role::Admin {
        let (restricted, allowed) = load_acl_state(db, user_id, project_id).await?;
        if restricted && !account_id.is_some_and(|id| allowed.iter().any(|candidate| candidate == id)) {
            return Err(AppError::forbidden("you are not permitted to use this CLI account"));
        }
    }
    Ok(())
}

pub(crate) async fn load_acl_state(
    db: &SqlitePool,
    user_id: &str,
    project_id: &str,
) -> Result<(bool, Vec<String>), AppError> {
    let restrict: Option<i64> = sqlx::query_scalar(
        "SELECT restrict_accounts FROM project_member_acl_settings WHERE user_id = ? AND project_id = ?",
    )
    .bind(user_id)
    .bind(project_id)
    .fetch_optional(db)
    .await?;

    let restricted = restrict.unwrap_or(0) != 0;
    if !restricted {
        return Ok((false, Vec::new()));
    }

    let allowed: Vec<String> = sqlx::query_scalar(
        "SELECT account_id FROM project_member_allowed_accounts WHERE user_id = ? AND project_id = ? ORDER BY account_id ASC",
    )
    .bind(user_id)
    .bind(project_id)
    .fetch_all(db)
    .await?;

    Ok((true, allowed))
}

async fn ensure_project_and_user_exist(
    db: &SqlitePool,
    project_id: &str,
    user_id: &str,
) -> Result<(), AppError> {
    let project_exists: Option<String> =
        sqlx::query_scalar("SELECT id FROM projects WHERE id = ?")
            .bind(project_id)
            .fetch_optional(db)
            .await?;
    if project_exists.is_none() {
        return Err(AppError::not_found("project not found"));
    }

    let user_exists: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(db)
        .await?;
    if user_exists.is_none() {
        return Err(AppError::not_found("user not found"));
    }

    Ok(())
}

async fn get_member_account_acl(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path((project_id, user_id)): Path<(String, String)>,
) -> Result<Json<AccountAclDto>, AppError> {
    ensure_project_and_user_exist(&state.db, &project_id, &user_id).await?;
    let (restricted, allowed_accounts) = load_acl_state(&state.db, &user_id, &project_id).await?;
    Ok(Json(AccountAclDto {
        user_id,
        project_id,
        restricted,
        allowed_accounts,
    }))
}

async fn list_project_account_acls(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<AccountAclDto>>, AppError> {
    let exists: Option<String> = sqlx::query_scalar("SELECT id FROM projects WHERE id = ?")
        .bind(&project_id)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(AppError::not_found("project not found"));
    }

    let member_ids: Vec<String> =
        sqlx::query_scalar("SELECT user_id FROM project_memberships WHERE project_id = ?")
            .bind(&project_id)
            .fetch_all(&state.db)
            .await?;

    let mut out = Vec::with_capacity(member_ids.len());
    for user_id in member_ids {
        let (restricted, allowed_accounts) =
            load_acl_state(&state.db, &user_id, &project_id).await?;
        out.push(AccountAclDto {
            user_id,
            project_id: project_id.clone(),
            restricted,
            allowed_accounts,
        });
    }
    Ok(Json(out))
}

async fn update_member_account_acl(
    State(state): State<AppState>,
    admin: AdminUser,
    Path((project_id, user_id)): Path<(String, String)>,
    Json(req): Json<UpdateAccountAclRequest>,
) -> Result<Json<AccountAclDto>, AppError> {
    ensure_project_and_user_exist(&state.db, &project_id, &user_id).await?;

    let now = Utc::now().to_rfc3339();
    let mut tx = state.db.begin().await?;

    match req.allowed_accounts {
        None => {
            sqlx::query(
                "INSERT INTO project_member_acl_settings (user_id, project_id, restrict_accounts, updated_at) VALUES (?, ?, 0, ?)
                 ON CONFLICT(user_id, project_id) DO UPDATE SET restrict_accounts = 0, updated_at = excluded.updated_at",
            )
            .bind(&user_id)
            .bind(&project_id)
            .bind(&now)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                "DELETE FROM project_member_allowed_accounts WHERE user_id = ? AND project_id = ?",
            )
            .bind(&user_id)
            .bind(&project_id)
            .execute(&mut *tx)
            .await?;
        }
        Some(ref ids) => {
            let mut deduped = Vec::new();
            let mut seen = HashSet::new();
            for aid in ids {
                if seen.insert(aid.clone()) {
                    let exists: Option<String> =
                        sqlx::query_scalar("SELECT id FROM cli_accounts WHERE id = ?")
                            .bind(aid)
                            .fetch_optional(&mut *tx)
                            .await?;
                    if exists.is_none() {
                        return Err(AppError::bad_request(format!("unknown account_id: {aid}")));
                    }
                    deduped.push(aid.clone());
                }
            }

            sqlx::query(
                "INSERT INTO project_member_acl_settings (user_id, project_id, restrict_accounts, updated_at) VALUES (?, ?, 1, ?)
                 ON CONFLICT(user_id, project_id) DO UPDATE SET restrict_accounts = 1, updated_at = excluded.updated_at",
            )
            .bind(&user_id)
            .bind(&project_id)
            .bind(&now)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                "DELETE FROM project_member_allowed_accounts WHERE user_id = ? AND project_id = ?",
            )
            .bind(&user_id)
            .bind(&project_id)
            .execute(&mut *tx)
            .await?;

            for aid in &deduped {
                sqlx::query(
                    "INSERT INTO project_member_allowed_accounts (user_id, project_id, account_id) VALUES (?, ?, ?)",
                )
                .bind(&user_id)
                .bind(&project_id)
                .bind(aid)
                .execute(&mut *tx)
                .await?;
            }
        }
    }

    tx.commit().await?;

    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "update_member_account_acl",
        Some("project_member"),
        Some(&format!("{project_id}:{user_id}")),
        None,
    )
    .await?;

    let (restricted, allowed_accounts) = load_acl_state(&state.db, &user_id, &project_id).await?;
    Ok(Json(AccountAclDto {
        user_id,
        project_id,
        restricted,
        allowed_accounts,
    }))
}

pub(crate) async fn build_project_cli_state(
    db: &SqlitePool,
    ctx: &ProjectContext,
) -> Result<ProjectCliStateDto, AppError> {
    let row = sqlx::query(
        "SELECT active_provider, active_account_id FROM project_active_cli WHERE project_id = ?",
    )
    .bind(&ctx.project.id)
    .fetch_optional(db)
    .await?;

    let (active_provider, active_account_id) = match row {
        Some(r) => {
            let p_str: String = r.try_get("active_provider")?;
            let provider: CliProvider = p_str
                .parse()
                .map_err(|e: String| AppError::internal(format!("corrupt provider in db: {e}")))?;
            let account_id: Option<String> = r.try_get("active_account_id")?;
            (provider, account_id)
        }
        None => (CliProvider::ClaudeCode, None),
    };

    if ctx.effective_role == Role::Viewer {
        return Ok(ProjectCliStateDto {
            project_id: ctx.project.id.clone(),
            active_provider,
            active_account_id: None,
            active_account_name: None,
            can_switch: false,
            available_accounts: Vec::new(),
        });
    }

    let (restricted, allowed_list) = if ctx.effective_role == Role::Admin {
        (false, Vec::new())
    } else {
        load_acl_state(db, &ctx.user.user.id, &ctx.project.id).await?
    };
    let allowed_set: HashSet<String> = allowed_list.into_iter().collect();

    let rows = sqlx::query(
        "SELECT id, provider, name, slug, profile_dir, created_at, updated_at FROM cli_accounts ORDER BY created_at ASC",
    )
    .fetch_all(db)
    .await?;

    let mut available_accounts = Vec::new();
    let mut active_account_name = None;

    for r in rows {
        let acc = row_to_account(&r)?;
        if active_account_id.as_deref() == Some(acc.id.as_str()) {
            active_account_name = Some(acc.name.clone());
        }
        if !restricted || allowed_set.contains(&acc.id) {
            available_accounts.push(AccountSummaryDto {
                id: acc.id,
                provider: acc.provider,
                name: acc.name,
                slug: acc.slug,
                has_credentials: acc.has_credentials,
            });
        }
    }

    Ok(ProjectCliStateDto {
        project_id: ctx.project.id.clone(),
        active_provider,
        active_account_id,
        active_account_name,
        can_switch: true,
        available_accounts,
    })
}

async fn get_project_cli_state(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
) -> Result<Json<ProjectCliStateDto>, AppError> {
    Ok(Json(build_project_cli_state(&state.db, &ctx).await?))
}

async fn switch_project_cli(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Json(req): Json<SwitchProjectCliRequest>,
) -> Result<Json<ProjectCliStateDto>, AppError> {
    Ok(Json(select_project_cli(&state, &ctx, req).await?))
}

pub(crate) async fn select_project_cli(
    state: &AppState,
    ctx: &ProjectContext,
    req: SwitchProjectCliRequest,
) -> Result<ProjectCliStateDto, AppError> {
    if ctx.effective_role == Role::Viewer {
        return Err(AppError::forbidden("viewer role cannot switch cli accounts"));
    }

    let resolved_account_id = match req.account_id {
        Some(aid) => {
            let acc = load_account(state, &aid).await?;
            if acc.provider != req.provider {
                return Err(AppError::bad_request(format!(
                    "account {} belongs to {} rather than {}",
                    acc.name,
                    acc.provider.display_name(),
                    req.provider.display_name()
                )));
            }
            if ctx.effective_role != Role::Admin {
                let (restricted, allowed) =
                    load_acl_state(&state.db, &ctx.user.user.id, &ctx.project.id).await?;
                if restricted && !allowed.iter().any(|id| id == &aid) {
                    return Err(AppError::forbidden(
                        "you are not permitted to use this CLI account",
                    ));
                }
            }
            Some(aid)
        }
        None => {
            let remembered: Option<String> = sqlx::query_scalar(
                "SELECT account_id FROM project_provider_accounts WHERE project_id = ? AND provider = ?",
            )
            .bind(&ctx.project.id)
            .bind(req.provider.as_str())
            .fetch_optional(&state.db)
            .await?;

            if let Some(aid) = remembered {
                if ctx.effective_role == Role::Admin {
                    Some(aid)
                } else {
                    let (restricted, allowed) =
                        load_acl_state(&state.db, &ctx.user.user.id, &ctx.project.id).await?;
                    if !restricted || allowed.iter().any(|id| id == &aid) {
                        Some(aid)
                    } else {
                        None
                    }
                }
            } else {
                None
            }
        }
    };

    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO project_active_cli (project_id, active_provider, active_account_id, updated_by, updated_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(project_id) DO UPDATE SET
           active_provider = excluded.active_provider,
           active_account_id = excluded.active_account_id,
           updated_by = excluded.updated_by,
           updated_at = excluded.updated_at",
    )
    .bind(&ctx.project.id)
    .bind(req.provider.as_str())
    .bind(&resolved_account_id)
    .bind(&ctx.user.user.id)
    .bind(&now)
    .execute(&state.db)
    .await?;

    if let Some(ref aid) = resolved_account_id {
        sqlx::query(
            "INSERT INTO project_provider_accounts (project_id, provider, account_id, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(project_id, provider) DO UPDATE SET
               account_id = excluded.account_id,
               updated_at = excluded.updated_at",
        )
        .bind(&ctx.project.id)
        .bind(req.provider.as_str())
        .bind(aid)
        .bind(&now)
        .execute(&state.db)
        .await?;
    }

    record_audit_log(
        state,
        Some(&ctx.user.user.id),
        "switch_project_cli",
        Some("project"),
        Some(&ctx.project.id),
        Some(req.provider.as_str()),
    )
    .await?;

    build_project_cli_state(&state.db, ctx).await
}

/// Resolves the active provider, optional account profile, and isolated env vars
/// for a project (used by CLI turn execution in Tickets #5/#7/#10).
pub async fn resolve_project_cli_env(
    db: &SqlitePool,
    project_id: &str,
) -> Result<(CliProvider, Option<CliAccountDto>, BTreeMap<String, String>), AppError> {
    let (provider, account) = resolve_project_cli_account(db, project_id).await?;
    let env = account.as_ref().map_or_else(BTreeMap::new, |account| {
        account_env_vars(account.provider, StdPath::new(&account.profile_dir))
    });
    Ok((provider, account, env))
}

pub(crate) async fn resolve_project_cli_account(
    db: &SqlitePool,
    project_id: &str,
) -> Result<(CliProvider, Option<CliAccountDto>), AppError> {
    let row = sqlx::query(
        "SELECT active_provider, active_account_id FROM project_active_cli WHERE project_id = ?",
    )
    .bind(project_id)
    .fetch_optional(db)
    .await?;

    let (provider, account_id) = match row {
        Some(r) => {
            let p_str: String = r.try_get("active_provider")?;
            let provider: CliProvider = p_str
                .parse()
                .map_err(|e: String| AppError::internal(format!("corrupt provider in db: {e}")))?;
            let account_id: Option<String> = r.try_get("active_account_id")?;
            (provider, account_id)
        }
        None => (CliProvider::ClaudeCode, None),
    };

    if let Some(aid) = account_id {
        let acc_row = sqlx::query(
            "SELECT id, provider, name, slug, profile_dir, created_at, updated_at FROM cli_accounts WHERE id = ?",
        )
        .bind(&aid)
        .fetch_optional(db)
        .await?;
        if let Some(r) = acc_row {
            return Ok((provider, Some(row_to_account(&r)?)));
        }
    }

    Ok((provider, None))
}

pub(crate) async fn resolve_provider_account(
    db: &SqlitePool,
    project_id: &str,
    provider: CliProvider,
) -> Result<Option<CliAccountDto>, AppError> {
    let row = sqlx::query(
        "SELECT a.id, a.provider, a.name, a.slug, a.profile_dir, a.created_at, a.updated_at
         FROM project_provider_accounts p JOIN cli_accounts a
           ON a.id = p.account_id AND a.provider = p.provider
         WHERE p.project_id = ? AND p.provider = ?",
    )
    .bind(project_id)
    .bind(provider.as_str())
    .fetch_optional(db)
    .await?;
    row.as_ref().map(row_to_account).transpose()
}

pub(crate) async fn cron_account_env(
    db: &SqlitePool,
    provider: CliProvider,
    account_id: &str,
) -> Result<BTreeMap<String, String>, AppError> {
    let profile: Option<String> = sqlx::query_scalar(
        "SELECT profile_dir FROM cli_accounts WHERE id = ? AND provider = ?",
    )
    .bind(account_id)
    .bind(provider.as_str())
    .fetch_optional(db)
    .await?;
    let profile = profile.ok_or_else(|| AppError::bad_request("CLI account does not belong to this provider"))?;
    Ok(account_env_vars(provider, StdPath::new(&profile)))
}
