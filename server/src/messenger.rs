use crate::{
    AppState, GatewayServer, accounts,
    auth::{self, AppError, AuthUser},
    chat::{self, EnqueueTurnParams},
    kanban,
    traits::{IncomingMessengerEvent, InlineButton, MessengerPlatform, OutboundMessage},
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, post},
};
use chrono::Utc;
use claude_web_ui_shared::{
    accounts::{CliProvider, SwitchProjectCliRequest},
    auth::{Role, UserDto},
    chat::{ApprovalMode, CreateChatSessionRequest, ResolvePermissionRequest},
    kanban::{KanbanColumn, MoveKanbanCardRequest},
    messenger::PairingCode,
};
use rand::Rng;
use sqlx::{Row, SqliteConnection};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use uuid::Uuid;

mod progress;
pub mod transport;

const PAIRING_TTL: u64 = 300;
const MENU_LIMIT: usize = 20;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/messenger/pairing-code", post(issue_pairing_code))
        .route(
            "/api/admin/users/{user_id}/messenger/{platform}",
            delete(unpair),
        )
}

async fn issue_pairing_code(
    user: AuthUser,
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<PairingCode>), AppError> {
    let now = Utc::now().timestamp();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("DELETE FROM messenger_pairing_codes WHERE user_id = ? OR expires_at <= ?")
        .bind(&user.user.id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    for _ in 0..20 {
        let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000));
        let expires_in_secs = PAIRING_TTL;
        let inserted =
            sqlx::query("INSERT OR IGNORE INTO messenger_pairing_codes VALUES (?, ?, ?)")
                .bind(&code)
                .bind(&user.user.id)
                .bind(now + expires_in_secs as i64)
                .execute(&mut *tx)
                .await?;
        if inserted.rows_affected() != 0 {
            tx.commit().await?;
            return Ok((
                StatusCode::CREATED,
                Json(PairingCode {
                    code,
                    expires_in_secs,
                }),
            ));
        }
    }
    Err(AppError::conflict("could not allocate a pairing code"))
}

async fn unpair(
    admin: auth::AdminUser,
    State(state): State<AppState>,
    Path((user_id, platform)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    MessengerPlatform::from_storage(&platform)
        .ok_or_else(|| AppError::bad_request("unknown platform"))?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("DELETE FROM user_messenger_links WHERE user_id = ? AND platform = ?")
        .bind(&user_id)
        .bind(&platform)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM messenger_pairing_codes WHERE user_id = ?")
        .bind(&user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    auth::record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "unpair_messenger",
        Some("user"),
        Some(&user_id),
        Some(&platform),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Clone, PartialEq, Eq)]
struct Destination {
    platform: MessengerPlatform,
    chat: String,
    thread: Option<String>,
    private_user: Option<String>,
}

impl From<&IncomingMessengerEvent> for Destination {
    fn from(event: &IncomingMessengerEvent) -> Self {
        Self {
            platform: event.platform,
            chat: event.chat_id.clone(),
            thread: event.thread_id.clone(),
            private_user: event.is_private.then(|| event.external_user_id.clone()),
        }
    }
}

impl Destination {
    async fn send(
        &self,
        state: &AppState,
        text: impl Into<String>,
        buttons: Vec<Vec<InlineButton>>,
    ) -> Result<String, AppError> {
        state
            .messenger
            .send_message(OutboundMessage {
                platform: self.platform,
                chat_id: self.chat.clone(),
                thread_id: self.thread.clone(),
                text: text.into(),
                buttons,
            })
            .await
            .map_err(AppError::internal)
    }

    async fn reply(&self, state: &AppState, text: impl Into<String>) -> Result<(), AppError> {
        self.send(state, text, Vec::new()).await.map(|_| ())
    }
}

#[derive(Debug, Clone)]
enum Action {
    Project,
    Session(String),
    Provider(CliProvider),
    Account(CliProvider, String),
    Stop(String),
    Approval {
        turn: String,
        request: String,
        allow: bool,
    },
    CardMove {
        card: String,
        from: KanbanColumn,
        to: KanbanColumn,
    },
}

#[derive(Clone)]
struct Callback {
    destination: Destination,
    project: String,
    action: Action,
    expires: Instant,
}

#[derive(Default)]
pub(crate) struct Callbacks(Mutex<HashMap<String, Callback>>);

impl Callbacks {
    fn button(
        &self,
        destination: &Destination,
        project: &str,
        label: impl Into<String>,
        action: Action,
        ttl: Duration,
    ) -> InlineButton {
        let key = Uuid::new_v4().simple().to_string();
        let mut callbacks = self.0.lock().expect("messenger callback mutex");
        let now = Instant::now();
        callbacks.retain(|_, callback| callback.expires > now);
        callbacks.insert(
            key.clone(),
            Callback {
                destination: destination.clone(),
                project: project.to_owned(),
                action,
                expires: now + ttl,
            },
        );
        InlineButton {
            label: label.into(),
            callback_data: key,
        }
    }

    fn get(&self, key: &str, destination: &Destination) -> Result<Callback, AppError> {
        let callbacks = self.0.lock().expect("messenger callback mutex");
        callbacks
            .get(key)
            .filter(|callback| {
                callback.expires > Instant::now() && callback.destination == *destination
            })
            .cloned()
            .ok_or_else(|| {
                AppError::forbidden("this button is expired or belongs to another conversation")
            })
    }
}

async fn redeem(state: &AppState, destination: &Destination, code: &str) -> Result<(), AppError> {
    let external = destination
        .private_user
        .as_deref()
        .ok_or_else(|| AppError::forbidden("send /pair in a direct message to the bot"))?;
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AppError::bad_request("usage: /pair <6-digit code>"));
    }
    let platform = destination.platform.as_str();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let now = Utc::now().timestamp();
    sqlx::query("DELETE FROM messenger_pairing_attempts WHERE attempted_at <= ?")
        .bind(now - PAIRING_TTL as i64)
        .execute(&mut *tx)
        .await?;
    let (all, own): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(CASE WHEN platform = ? AND external_user_id = ? THEN 1 END)
         FROM messenger_pairing_attempts",
    )
    .bind(platform)
    .bind(external)
    .fetch_one(&mut *tx)
    .await?;
    if all >= 100 || own >= 5 {
        return Err(AppError::forbidden(
            "too many pairing attempts, try again in five minutes",
        ));
    }
    let user: Option<String> = sqlx::query_scalar(
        "SELECT user_id FROM messenger_pairing_codes WHERE code = ? AND expires_at > ?",
    )
    .bind(code)
    .bind(now)
    .fetch_optional(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO messenger_pairing_attempts VALUES (?, ?, ?)")
        .bind(platform)
        .bind(external)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    let Some(user) = user else {
        tx.commit().await?;
        return Err(AppError::forbidden("invalid or expired pairing code"));
    };
    let inserted = sqlx::query("INSERT OR IGNORE INTO user_messenger_links VALUES (?, ?, ?, ?)")
        .bind(&user)
        .bind(platform)
        .bind(external)
        .bind(&destination.chat)
        .execute(&mut *tx)
        .await?;
    if inserted.rows_affected() == 0 {
        tx.commit().await?;
        return Err(AppError::conflict(
            "this messenger identity or WebUI account is already paired",
        ));
    }
    sqlx::query("DELETE FROM messenger_pairing_codes WHERE code = ?")
        .bind(code)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    auth::record_audit_log(
        state,
        Some(&user),
        "pair_messenger",
        Some("user"),
        Some(&user),
        Some(platform),
    )
    .await?;
    destination
        .reply(state, "Account paired. Use /project to get started.")
        .await
}

async fn messenger_user(
    state: &AppState,
    platform: MessengerPlatform,
    external: &str,
) -> Result<AuthUser, AppError> {
    let link: Option<String> = sqlx::query_scalar(
        "SELECT user_id FROM user_messenger_links WHERE platform = ? AND external_user_id = ?",
    )
    .bind(platform.as_str())
    .bind(external)
    .fetch_optional(&state.db)
    .await?;
    let user = match link {
        Some(id) => auth::load_user_by_id(state, &id).await?,
        None => None,
    }
    .ok_or_else(|| {
        AppError::unauthorized(
            "pair your account in a direct message with /pair <code> from the WebUI",
        )
    })?;
    Ok(AuthUser {
        user,
        session_id: String::new(),
    })
}

pub(crate) async fn handle(
    server: &GatewayServer,
    event: IncomingMessengerEvent,
) -> Result<(), AppError> {
    let destination = Destination::from(&event);
    let result = dispatch(server, &destination, &event).await;
    if let Err(error) = &result {
        let text = if error.status.is_server_error() {
            tracing::warn!(code = error.code, "messenger operation failed");
            "The gateway could not complete this action"
        } else {
            error.message.as_str()
        };
        let _ = destination.reply(&server.state, text).await;
    }
    result
}

async fn dispatch(
    server: &GatewayServer,
    dest: &Destination,
    event: &IncomingMessengerEvent,
) -> Result<(), AppError> {
    let state = &server.state;
    let text = event.text.as_deref().unwrap_or_default().trim();
    let (command, argument) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let command = command.split('@').next().unwrap_or(command);
    let argument = argument.trim();

    if command == "/pair" && event.callback_data.is_none() {
        return redeem(state, dest, argument).await;
    }

    let actor = messenger_user(state, event.platform, &event.external_user_id).await?;
    if let Some(key) = &event.callback_data {
        return apply_callback(server, dest, actor, key).await;
    }

    if command == "/help" || command == "/start" {
        return dest.reply(state, "/project, /session, /new [interactive], /cli, /account, /kanban, /stop, /notify [on|off]").await;
    }

    if command == "/bind" {
        if dest.private_user.is_some() || actor.user.role != Role::Admin {
            return Err(AppError::forbidden(
                "only an administrator can bind a group or channel",
            ));
        }
        let project = find_project(state, &actor.user, argument).await?;
        let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
        if channel_project(&mut tx, dest)
            .await?
            .as_deref()
            .is_some_and(|id| id != project)
        {
            sqlx::query("DELETE FROM messenger_channels WHERE platform = ? AND chat_id = ?")
                .bind(dest.platform.as_str())
                .bind(&dest.chat)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("INSERT OR IGNORE INTO messenger_channels (platform, chat_id, project_id, notify, notification_thread_id)
            VALUES (?, ?, ?, 1, ?)")
            .bind(dest.platform.as_str()).bind(&dest.chat).bind(&project).bind(&dest.thread)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        return dest
            .reply(
                state,
                "Channel linked. Start a prompt in a forum topic or thread.",
            )
            .await;
    }

    if command == "/project" && dest.private_user.is_some() {
        return if argument.is_empty() {
            let projects = auth::load_accessible_projects(state, &actor.user).await?;
            let choices = projects
                .iter()
                .map(|p| (p.name.clone(), p.id.as_str(), Action::Project));
            send_menu(state, dest, "Choose a project", choices).await
        } else {
            let project = find_project(state, &actor.user, argument).await?;
            select_project(state, dest, &project).await
        };
    }

    let (project, session) = context(state, dest).await?;
    let write = !matches!(
        command,
        "/session" | "/cli" | "/account" | "/kanban" | "/notify"
    ) || (matches!(command, "/cli" | "/account") && !argument.is_empty());
    let ctx = auth::authorize_project(state, actor, &project, write).await?;

    match command {
        "/session" if dest.private_user.is_some() => {
            if argument.is_empty() {
                let sessions = sqlx::query(
                    "SELECT id, title FROM chat_sessions WHERE project_id = ?
                    ORDER BY updated_at DESC LIMIT ?",
                )
                .bind(&project)
                .bind(MENU_LIMIT as i64)
                .fetch_all(&state.db)
                .await?;
                let choices = sessions.iter().map(|row| {
                    let id: &str = row.get("id");
                    (
                        row.get("title"),
                        project.as_str(),
                        Action::Session(id.to_owned()),
                    )
                });
                return send_menu(state, dest, "Choose a session", choices).await;
            }
            select_session(state, dest, &project, argument).await
        }
        "/new" if dest.private_user.is_some() => {
            let (mode, title) = match argument.split_once(' ').unwrap_or((argument, "")) {
                ("interactive", title) => (ApprovalMode::Interactive, title),
                ("full_auto", title) => (ApprovalMode::FullAuto, title),
                _ => (ApprovalMode::FullAuto, argument),
            };
            let session = chat::create_gateway_session(
                &state.db,
                &project,
                &ctx.user.user.id,
                CreateChatSessionRequest {
                    title: Some(title.to_owned()),
                    approval_mode: Some(mode),
                    provider: None,
                    native_session_id: None,
                },
            )
            .await?;
            select_session(state, dest, &project, &session.id).await
        }
        "/cli" | "/account" if dest.private_user.is_some() => {
            let cli = accounts::build_project_cli_state(&state.db, &ctx).await?;
            if command == "/cli" && argument.is_empty() {
                if !cli.can_switch {
                    return dest
                        .reply(
                            state,
                            format!("Active CLI: {}", cli.active_provider.display_name()),
                        )
                        .await;
                }
                let choices = [
                    CliProvider::ClaudeCode,
                    CliProvider::Codex,
                    CliProvider::Antigravity,
                ]
                .into_iter()
                .map(|provider| {
                    (
                        provider.display_name().into(),
                        project.as_str(),
                        Action::Provider(provider),
                    )
                });
                return send_menu(state, dest, "Choose a CLI", choices).await;
            }
            if command == "/account" && argument.is_empty() {
                if !cli.can_switch {
                    return Err(AppError::forbidden(
                        "viewer role cannot switch CLI accounts",
                    ));
                }
                let provider = match session.as_deref() {
                    Some(id) => {
                        chat::load_chat_session(&state.db, &project, id)
                            .await?
                            .provider
                    }
                    None => cli.active_provider,
                };
                let choices = cli
                    .available_accounts
                    .iter()
                    .filter(|a| a.provider == provider)
                    .map(|a| {
                        (
                            a.name.clone(),
                            project.as_str(),
                            Action::Account(provider, a.id.clone()),
                        )
                    });
                return send_menu(state, dest, "Choose an account", choices).await;
            }
            if command == "/cli" {
                let provider = argument.parse().map_err(AppError::bad_request)?;
                return switch_cli(state, dest, &ctx, provider, None).await;
            }
            let account = cli
                .available_accounts
                .iter()
                .find(|account| account.id == argument)
                .ok_or_else(|| AppError::not_found("account not available"))?;
            switch_cli(
                state,
                dest,
                &ctx,
                account.provider,
                Some(account.id.clone()),
            )
            .await
        }
        "/kanban" => {
            let cards = kanban::list_cards(&state.db, &project).await?;
            let text = if cards.is_empty() {
                "No Kanban cards.".to_owned()
            } else {
                cards
                    .iter()
                    .take(MENU_LIMIT)
                    .map(|c| format!("{} · {}", c.column.as_str(), c.title))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let mut buttons = Vec::new();
            if ctx.effective_role != Role::Viewer && dest.private_user.is_some() {
                for card in cards.iter().take(MENU_LIMIT) {
                    if let Some((label, to)) = kanban_action(card.column) {
                        buttons.push(vec![state.messenger_callbacks.button(
                            dest,
                            &project,
                            format!("{label}: {}", card.title),
                            Action::CardMove {
                                card: card.id.clone(),
                                from: card.column,
                                to,
                            },
                            Duration::from_secs(900),
                        )]);
                    }
                }
            }
            dest.send(state, text, buttons).await.map(|_| ())
        }
        "/notify" => toggle_notifications(state, dest, &ctx, argument).await,
        "/stop" => {
            let session = session.ok_or_else(|| AppError::not_found("no session selected"))?;
            state
                .scheduler
                .stop_session_turn(&project, &session)
                .await?;
            dest.reply(state, "Stopping turn.").await
        }
        cmd if cmd.starts_with('/') => {
            Err(AppError::bad_request("command unavailable here, use /help"))
        }
        _ if text.is_empty() => Ok(()),
        _ => {
            let session = match session {
                Some(id) => id,
                None if dest.private_user.is_some() => {
                    return Err(AppError::bad_request(
                        "choose /session or create one with /new",
                    ));
                }
                None => bind_thread(state, dest, &project, &ctx.user.user.id).await?,
            };
            let events = state.scheduler.subscribe();
            let response = state
                .scheduler
                .enqueue_authorized_turn(EnqueueTurnParams {
                    project_id: project.clone(),
                    session_id: session,
                    user_id: ctx.user.user.id.clone(),
                    prompt: text.to_owned(),
                })
                .await?;
            let gateway = state.clone();
            let external_user = event.external_user_id.clone();
            let dest = dest.clone();
            tokio::spawn(async move {
                if let Err(error) = progress::follow(
                    gateway,
                    dest,
                    project,
                    response.turn.id,
                    external_user,
                    events,
                )
                .await
                {
                    tracing::warn!(code = error.code, "messenger status delivery failed");
                }
            });
            Ok(())
        }
    }
}

async fn find_project(state: &AppState, user: &UserDto, query: &str) -> Result<String, AppError> {
    auth::load_accessible_projects(state, user)
        .await?
        .into_iter()
        .find(|project| project.id == query || project.slug == query)
        .map(|project| project.id)
        .ok_or_else(|| AppError::not_found("choose an accessible project by ID or slug"))
}

async fn channel_project(
    connection: &mut SqliteConnection,
    dest: &Destination,
) -> Result<Option<String>, AppError> {
    Ok(sqlx::query_scalar(
        "SELECT project_id FROM messenger_channels WHERE platform = ? AND chat_id = ?",
    )
    .bind(dest.platform.as_str())
    .bind(&dest.chat)
    .fetch_optional(connection)
    .await?)
}

async fn context(
    state: &AppState,
    dest: &Destination,
) -> Result<(String, Option<String>), AppError> {
    if let Some(user) = &dest.private_user {
        let selected = sqlx::query_as(
            "SELECT project_id, session_id FROM messenger_dm_contexts
             WHERE platform = ? AND external_user_id = ?",
        )
        .bind(dest.platform.as_str())
        .bind(user)
        .fetch_optional(&state.db)
        .await?;
        return selected.ok_or_else(|| AppError::bad_request("choose a project with /project"));
    }
    let project = channel_project(&mut *state.db.acquire().await?, dest)
        .await?
        .ok_or_else(|| {
            AppError::forbidden("this channel needs an administrator to run /bind <project>")
        })?;
    let session = match &dest.thread {
        Some(thread) => sqlx::query_scalar("SELECT session_id FROM messenger_threads WHERE platform = ? AND chat_id = ? AND thread_id = ?")
            .bind(dest.platform.as_str()).bind(&dest.chat).bind(thread).fetch_optional(&state.db).await?,
        None => None,
    };
    Ok((project, session))
}

async fn bind_thread(
    state: &AppState,
    dest: &Destination,
    project: &str,
    user: &str,
) -> Result<String, AppError> {
    let thread = dest
        .thread
        .as_deref()
        .ok_or_else(|| AppError::bad_request("send prompts inside a forum topic or thread"))?;
    let (provider, _, _) = accounts::resolve_project_cli_env(&state.db, project).await?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if channel_project(&mut tx, dest).await?.as_deref() != Some(project) {
        return Err(AppError::conflict("channel project changed"));
    }
    let existing = sqlx::query_scalar("SELECT session_id FROM messenger_threads WHERE platform = ? AND chat_id = ? AND thread_id = ?")
        .bind(dest.platform.as_str()).bind(&dest.chat).bind(thread).fetch_optional(&mut *tx).await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let id = chat::insert_gateway_session(
        &mut tx,
        project,
        user,
        provider,
        CreateChatSessionRequest {
            title: Some(format!("{} thread {}", dest.platform.as_str(), thread)),
            approval_mode: None,
            provider: Some(provider),
            native_session_id: None,
        },
    )
    .await?;
    sqlx::query("INSERT INTO messenger_threads VALUES (?, ?, ?, ?)")
        .bind(dest.platform.as_str())
        .bind(&dest.chat)
        .bind(thread)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(id)
}

async fn select_project(
    state: &AppState,
    dest: &Destination,
    project: &str,
) -> Result<(), AppError> {
    let user = dest
        .private_user
        .as_deref()
        .ok_or_else(|| AppError::forbidden("use a direct message"))?;
    sqlx::query("INSERT INTO messenger_dm_contexts (platform, external_user_id, project_id)
        VALUES (?, ?, ?) ON CONFLICT (platform, external_user_id)
        DO UPDATE SET project_id = excluded.project_id,
            session_id = CASE WHEN messenger_dm_contexts.project_id = excluded.project_id THEN session_id ELSE NULL END")
        .bind(dest.platform.as_str()).bind(user).bind(project).execute(&state.db).await?;
    dest.reply(state, "Project selected. Use /session or /new.")
        .await
}

async fn select_session(
    state: &AppState,
    dest: &Destination,
    project: &str,
    id: &str,
) -> Result<(), AppError> {
    let user = dest
        .private_user
        .as_deref()
        .ok_or_else(|| AppError::forbidden("use a direct message"))?;
    let session = chat::load_chat_session(&state.db, project, id).await?;
    let updated = sqlx::query(
        "UPDATE messenger_dm_contexts SET session_id = ? WHERE platform = ?
        AND external_user_id = ? AND project_id = ?",
    )
    .bind(id)
    .bind(dest.platform.as_str())
    .bind(user)
    .bind(project)
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::conflict("project selection has changed"));
    }
    dest.reply(state, format!("Session selected: {}", session.title))
        .await
}

async fn switch_cli(
    state: &AppState,
    dest: &Destination,
    ctx: &auth::ProjectContext,
    provider: CliProvider,
    account_id: Option<String>,
) -> Result<(), AppError> {
    let (_, session) = context(state, dest).await?;
    let current = match session {
        Some(id) => Some(chat::load_chat_session(&state.db, &ctx.project.id, &id).await?),
        None => None,
    };
    accounts::select_project_cli(
        state,
        ctx,
        SwitchProjectCliRequest {
            provider,
            account_id,
        },
    )
    .await?;
    if let Some(session) = current.filter(|session| session.provider != provider) {
        if session.native_session_id.is_some() || has_turns(state, &session.id).await? {
            let replacement = chat::handoff::create(state, ctx, &session.id, provider).await?;
            select_session(state, dest, &ctx.project.id, &replacement.id).await?;
        } else {
            sqlx::query(
                "UPDATE chat_sessions SET provider = ? WHERE id = ? AND native_session_id IS NULL
                AND NOT EXISTS (SELECT 1 FROM chat_turns WHERE session_id = chat_sessions.id)",
            )
            .bind(provider.as_str())
            .bind(session.id)
            .execute(&state.db)
            .await?;
        }
    }
    dest.reply(state, format!("Active CLI: {}", provider.display_name()))
        .await
}

async fn has_turns(state: &AppState, session: &str) -> Result<bool, AppError> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chat_turns WHERE session_id = ?)")
            .bind(session)
            .fetch_one(&state.db)
            .await?,
    )
}

async fn send_menu<'a>(
    state: &AppState,
    dest: &Destination,
    title: &str,
    choices: impl Iterator<Item = (String, &'a str, Action)>,
) -> Result<(), AppError> {
    let rows: Vec<_> = choices
        .take(MENU_LIMIT)
        .map(|(label, project, action)| {
            vec![state.messenger_callbacks.button(
                dest,
                project,
                label,
                action,
                Duration::from_secs(900),
            )]
        })
        .collect();
    let text = if rows.is_empty() {
        "No choices available."
    } else {
        title
    };
    dest.send(state, text, rows).await.map(|_| ())
}

async fn apply_callback(
    server: &GatewayServer,
    dest: &Destination,
    actor: AuthUser,
    key: &str,
) -> Result<(), AppError> {
    let state = &server.state;
    let callback = state.messenger_callbacks.get(key, dest)?;
    let write = !matches!(callback.action, Action::Project | Action::Session(_));
    let ctx = auth::authorize_project(state, actor, &callback.project, write).await?;
    if dest.private_user.is_none()
        && channel_project(&mut *state.db.acquire().await?, dest)
            .await?
            .as_deref()
            != Some(&callback.project)
    {
        return Err(AppError::forbidden(
            "this channel no longer belongs to the project",
        ));
    }
    let project = callback.project.as_str();
    match callback.action {
        Action::Project => select_project(state, dest, project).await,
        Action::Session(session) => select_session(state, dest, project, &session).await,
        Action::Provider(provider) => switch_cli(state, dest, &ctx, provider, None).await,
        Action::Account(provider, account) => {
            switch_cli(state, dest, &ctx, provider, Some(account)).await
        }
        Action::Stop(turn) => {
            state.scheduler.cancel_turn(project, &turn).await?;
            dest.reply(state, "Stopping turn.").await
        }
        Action::Approval {
            turn,
            request,
            allow,
        } => {
            state
                .scheduler
                .resolve_permission(
                    project,
                    &turn,
                    ResolvePermissionRequest {
                        approved: allow,
                        request_id: Some(request),
                    },
                )
                .await?;
            Ok(())
        }
        Action::CardMove { card, from, to } => {
            kanban::move_expected(
                state,
                project,
                &card,
                &ctx.user.user.id,
                Some(from),
                MoveKanbanCardRequest {
                    column: to,
                    position: None,
                    blocked_reason: None,
                    result_summary: None,
                },
            )
            .await?;
            dest.reply(state, format!("Card moved to {}.", to.as_str()))
                .await
        }
    }
}

fn kanban_action(column: KanbanColumn) -> Option<(&'static str, KanbanColumn)> {
    match column {
        KanbanColumn::Review => Some(("Mark done", KanbanColumn::Done)),
        KanbanColumn::Blocked => Some(("Move to ready", KanbanColumn::Ready)),
        _ => None,
    }
}

async fn toggle_notifications(
    state: &AppState,
    dest: &Destination,
    ctx: &auth::ProjectContext,
    argument: &str,
) -> Result<(), AppError> {
    let enabled = match argument {
        "" | "on" => true,
        "off" => false,
        _ => return Err(AppError::bad_request("usage: /notify [on|off]")),
    };
    if let Some(user) = &dest.private_user {
        let query = if enabled {
            "INSERT OR IGNORE INTO messenger_dm_notifications VALUES (?, ?, ?)"
        } else {
            "DELETE FROM messenger_dm_notifications WHERE platform = ? AND external_user_id = ? AND project_id = ?"
        };
        sqlx::query(query)
            .bind(dest.platform.as_str())
            .bind(user)
            .bind(&ctx.project.id)
            .execute(&state.db)
            .await?;
    } else {
        if ctx.user.user.role != Role::Admin {
            return Err(AppError::forbidden(
                "only an administrator can change channel notifications",
            ));
        }
        sqlx::query(
            "UPDATE messenger_channels SET notify = ?, notification_thread_id = ?
             WHERE platform = ? AND chat_id = ? AND project_id = ?",
        )
        .bind(enabled)
        .bind(&dest.thread)
        .bind(dest.platform.as_str())
        .bind(&dest.chat)
        .bind(&ctx.project.id)
        .execute(&state.db)
        .await?;
    }
    dest.reply(
        state,
        if enabled {
            "Kanban notifications enabled."
        } else {
            "Kanban notifications disabled."
        },
    )
    .await
}

pub(crate) use progress::start_notifications;
