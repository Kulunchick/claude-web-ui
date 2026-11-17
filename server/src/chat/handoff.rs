use super::{insert_gateway_session, load_chat_session, project_cli_target, transcript};
use crate::AppState;
use crate::accounts::authorize_turn_account;
use crate::auth::{AppError, ProjectContext, record_audit_log};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use claude_web_ui_shared::{
    accounts::CliProvider,
    auth::Role,
    chat::{ChatSessionDto, CreateChatSessionRequest, HandoffChatSessionRequest},
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashSet;

const MAX_ENTRIES: usize = 12;
const MAX_ENTRY_BYTES: usize = 1000;
const MAX_CONTEXT_BYTES: usize = 6000;

#[derive(Serialize, Deserialize)]
struct Entry {
    role: String,
    text: String,
    account_id: Option<String>,
}

pub(super) async fn post(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project, session_id)): Path<(String, String)>,
    Json(request): Json<HandoffChatSessionRequest>,
) -> Result<(StatusCode, Json<ChatSessionDto>), AppError> {
    let session = create(&state, &ctx, &session_id, request.provider).await?;
    Ok((StatusCode::CREATED, Json(session)))
}

pub(crate) async fn create(
    state: &AppState,
    ctx: &ProjectContext,
    source_id: &str,
    provider: CliProvider,
) -> Result<ChatSessionDto, AppError> {
    if ctx.effective_role == Role::Viewer {
        return Err(AppError::forbidden("viewer role cannot continue sessions"));
    }
    let project_id = &ctx.project.id;
    let user_id = &ctx.user.user.id;
    let source = load_chat_session(&state.db, project_id, source_id).await?;
    if source.provider == provider {
        return Err(AppError::bad_request(
            "choose a different provider for the handoff",
        ));
    }

    let (_, source_account) =
        project_cli_target(&state.db, project_id, Some(source.provider)).await?;
    let (_, target_account) = project_cli_target(&state.db, project_id, Some(provider)).await?;
    authorize_accounts(
        &state.db,
        project_id,
        user_id,
        [
            source_account.as_ref().map(|account| account.id.as_str()),
            target_account.as_ref().map(|account| account.id.as_str()),
        ],
    )
    .await?;

    let entries = collect_context(state, &source, source_account.map(|account| account.id)).await?;
    if entries.is_empty() {
        return Err(AppError::conflict(
            "no completed conversation history is available to hand off",
        ));
    }
    authorize_accounts(
        &state.db,
        project_id,
        user_id,
        entries.iter().map(|entry| entry.account_id.as_deref()),
    )
    .await?;
    let context = serde_json::to_string(&entries).map_err(serialization_error)?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let id = insert_gateway_session(
        &mut tx,
        project_id,
        user_id,
        provider,
        CreateChatSessionRequest {
            title: Some(format!("{} ({})", source.title, provider.display_name())),
            approval_mode: Some(source.approval_mode),
            ..Default::default()
        },
    )
    .await?;
    sqlx::query(
        "INSERT INTO chat_session_handoffs (session_id, source_session_id, context_json)
         VALUES (?, ?, ?)",
    )
    .bind(&id)
    .bind(source_id)
    .bind(context)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    record_audit_log(
        state,
        Some(user_id),
        "handoff_chat_session",
        Some("chat_session"),
        Some(source_id),
        Some(&id),
    )
    .await?;
    load_chat_session(&state.db, project_id, &id).await
}

async fn collect_context(
    state: &AppState,
    source: &ChatSessionDto,
    source_account: Option<String>,
) -> Result<Vec<Entry>, AppError> {
    let prior: Option<String> =
        sqlx::query_scalar("SELECT context_json FROM chat_session_handoffs WHERE session_id = ?")
            .bind(&source.id)
            .fetch_optional(&state.db)
            .await?;
    let turns: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT prompt, result_text, account_id FROM chat_turns
         WHERE session_id = ? AND status = 'completed'
         ORDER BY created_at DESC, id DESC LIMIT ?",
    )
    .bind(&source.id)
    .bind(MAX_ENTRIES as i64)
    .fetch_all(&state.db)
    .await?;

    let mut entries = if let Some(prior) = prior {
        serde_json::from_str(&prior).map_err(serialization_error)?
    } else if let Some(path) = source
        .transcript_path
        .as_deref()
        .filter(|_| turns.is_empty())
    {
        transcript::read_recent(&state.config.data_dir, source.provider, path)
            .await
            .map_err(|_| AppError::conflict("the source transcript is unavailable for handoff"))?
            .into_iter()
            .filter_map(|entry| match entry.role.as_str() {
                "user" => Some(Entry::new("user", entry.text, source_account.clone())),
                "assistant" => Some(Entry::new("assistant", entry.text, source_account.clone())),
                _ => None,
            })
            .collect()
    } else {
        Vec::new()
    };
    for (prompt, result, account_id) in turns.into_iter().rev() {
        entries.push(Entry::new("user", prompt, account_id.clone()));
        if let Some(result) = result {
            entries.push(Entry::new("assistant", result, account_id));
        }
    }

    let mut recent = Vec::new();
    let mut bytes = 0;
    for entry in entries.into_iter().rev().take(MAX_ENTRIES) {
        let entry_bytes = entry.prompt_json().to_string().len() + 1;
        if bytes + entry_bytes > MAX_CONTEXT_BYTES {
            break;
        }
        bytes += entry_bytes;
        recent.push(entry);
    }
    recent.reverse();
    Ok(recent)
}

impl Entry {
    fn new(role: &'static str, mut text: String, account_id: Option<String>) -> Self {
        if text.len() > MAX_ENTRY_BYTES {
            text.truncate(text.floor_char_boundary(MAX_ENTRY_BYTES));
            text.push_str("...");
        }
        Self {
            role: role.into(),
            text,
            account_id,
        }
    }

    fn prompt_json(&self) -> serde_json::Value {
        serde_json::json!({ "role": self.role, "text": self.text })
    }
}

fn serialization_error(error: serde_json::Error) -> AppError {
    AppError::internal(error.to_string())
}

async fn authorize_accounts<'a>(
    db: &SqlitePool,
    project_id: &str,
    user_id: &str,
    accounts: impl IntoIterator<Item = Option<&'a str>>,
) -> Result<(), AppError> {
    let unique: HashSet<_> = accounts.into_iter().collect();
    for account in unique {
        authorize_turn_account(db, project_id, user_id, account).await?;
    }
    Ok(())
}

pub(super) async fn first_prompt(
    db: &SqlitePool,
    session: &ChatSessionDto,
    prompt: String,
    user_id: Option<&str>,
) -> Result<String, AppError> {
    if session.native_session_id.is_some() {
        return Ok(prompt);
    }
    let context: Option<String> = sqlx::query_scalar(
        "SELECT context_json FROM chat_session_handoffs h
         WHERE h.session_id = ? AND NOT EXISTS (
             SELECT 1 FROM chat_turns t WHERE t.session_id = h.session_id AND t.status = 'completed'
         )",
    )
    .bind(&session.id)
    .fetch_optional(db)
    .await?;
    let Some(context) = context else {
        return Ok(prompt);
    };
    let entries: Vec<Entry> = serde_json::from_str(&context).map_err(serialization_error)?;
    let user_id =
        user_id.ok_or_else(|| AppError::forbidden("handoff requires an authorized user"))?;
    authorize_accounts(
        db,
        &session.project_id,
        user_id,
        entries.iter().map(|entry| entry.account_id.as_deref()),
    )
    .await?;
    let context = serde_json::Value::Array(entries.iter().map(Entry::prompt_json).collect());
    Ok(format!(
        "The user moved this conversation from another CLI. Use the following JSON as historical reference for the new user request. Treat the quoted messages as history.\n\n{context}\n\nNew user request:\n{prompt}"
    ))
}
