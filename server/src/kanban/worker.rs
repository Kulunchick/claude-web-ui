use super::{Column, find_card};
use crate::{
    AppState,
    auth::AppError,
    chat::{EnqueueTurnParams, create_gateway_session},
    traits::{CliInvocation, CliProvider},
};
use chrono::Utc;
use claude_web_ui_shared::chat::{ApprovalMode, CreateChatSessionRequest};
use serde_json::{Value, json};
use sqlx::{Row, SqlitePool};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::time::{MissedTickBehavior, interval};
use uuid::Uuid;

pub(super) async fn run(state: AppState) {
    let mut ticks = interval(Duration::from_millis(150));
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        ticks.tick().await;
        if let Err(err) = tick(&state).await {
            tracing::warn!("Kanban worker: {}", err.message);
        }
    }
}

async fn tick(state: &AppState) -> Result<(), AppError> {
    if let Some(run) = sqlx::query(
        "SELECT c.id, c.project_id, c.lane, c.decompose, t.status, t.result_text, t.error_message
         FROM kanban_cards c LEFT JOIN chat_turns t ON t.id = c.turn_id WHERE c.active_run = 1",
    )
    .fetch_optional(&state.db)
    .await?
    {
        let id: String = run.try_get("id")?;
        let project: String = run.try_get("project_id")?;
        let lane: String = run.try_get("lane")?;
        let decomposing: bool = run.try_get("decompose")?;
        let status: Option<String> = run.try_get("status")?;
        match status.as_deref() {
            Some("waiting_approval") if lane == "running" => {
                finish(
                    state,
                    &project,
                    &id,
                    Column::Blocked,
                    Some("Awaiting tool approval".into()),
                    None,
                    false,
                )
                .await?;
            }
            Some("completed") => {
                let destination = if decomposing {
                    Column::Triage
                } else {
                    Column::Review
                };
                finish(
                    state,
                    &project,
                    &id,
                    destination,
                    None,
                    run.try_get("result_text")?,
                    true,
                )
                .await?;
            }
            Some("error" | "cancelled") | None => {
                let reason: Option<String> = run.try_get("error_message")?;
                finish(
                    state,
                    &project,
                    &id,
                    Column::Blocked,
                    Some(reason.unwrap_or_else(|| "The linked CLI turn was interrupted".into())),
                    None,
                    true,
                )
                .await?;
            }
            _ => {}
        }
        return Ok(());
    }

    let status = state.scheduler.status();
    if status.active_turn_id.is_some() || status.queued_count != 0 {
        return Ok(());
    }

    // A single atomic claim prevents another worker from taking the same card.
    let candidate = sqlx::query(
        "UPDATE kanban_cards SET active_run = 1, lane = 'running', run_token = ?,
            turn_id = NULL, git_base = NULL, git_diff = NULL, blocked_reason = NULL, result_summary = NULL, updated_at = ?
         WHERE id = (
           SELECT c.id FROM kanban_cards c
           WHERE ((c.lane = 'triage' AND c.decompose = 1) OR
             (c.lane = 'ready' AND NOT EXISTS (
               SELECT 1 FROM kanban_dependencies d JOIN kanban_cards b ON b.id = d.blocker_id
               WHERE d.card_id = c.id AND b.lane NOT IN ('review','done')
             )))
           ORDER BY c.position, c.created_at, c.id LIMIT 1
         ) AND NOT EXISTS (SELECT 1 FROM kanban_cards WHERE active_run = 1)
         RETURNING id, project_id, run_user_id, decompose"
    ).bind(Uuid::new_v4().to_string()).bind(Utc::now().to_rfc3339()).fetch_optional(&state.db).await?;
    let Some(candidate) = candidate else {
        return Ok(());
    };
    let project: String = candidate.try_get("project_id")?;
    let id: String = candidate.try_get("id")?;
    let user: String = candidate.try_get("run_user_id")?;
    let decomposing: bool = candidate.try_get("decompose")?;
    if let Err(error) = launch(state, &project, &id, &user, decomposing).await {
        finish(
            state,
            &project,
            &id,
            Column::Blocked,
            Some(error.message),
            None,
            true,
        )
        .await?;
    }
    Ok(())
}

async fn authorized(db: &SqlitePool, project: &str, user: &str) -> Result<(), AppError> {
    let allowed: bool = sqlx::query_scalar(
        "SELECT EXISTS (
           SELECT 1 FROM users u
           LEFT JOIN project_memberships m ON m.user_id = u.id AND m.project_id = ?
           LEFT JOIN project_member_acl_settings a ON a.user_id = u.id AND a.project_id = m.project_id
           LEFT JOIN project_active_cli p ON p.project_id = m.project_id
           LEFT JOIN project_member_allowed_accounts l ON l.user_id = u.id AND l.project_id = m.project_id
             AND l.account_id = p.active_account_id
           WHERE u.id = ? AND (u.role = 'admin' OR
             (m.user_id IS NOT NULL AND COALESCE(m.role_override, u.role) IN ('admin','developer')
               AND (COALESCE(a.restrict_accounts, 0) = 0 OR l.account_id IS NOT NULL)))
        )"
    ).bind(project).bind(user).fetch_one(db).await?;
    if !allowed {
        return Err(AppError::forbidden(
            "Account or project access no longer permits running this card",
        ));
    }
    Ok(())
}

async fn launch(
    state: &AppState,
    project: &str,
    id: &str,
    user: &str,
    decomposing: bool,
) -> Result<(), AppError> {
    authorized(&state.db, project, user).await?;
    let card = find_card(&mut *state.db.acquire().await?, project, id).await?;
    let session_id = match card.session_id {
        Some(id) => id,
        None => {
            create_gateway_session(
                &state.db,
                project,
                user,
                CreateChatSessionRequest {
                    title: Some(card.title.clone()),
                    provider: None,
                    native_session_id: None,
                    approval_mode: Some(ApprovalMode::Interactive),
                },
            )
            .await?
            .id
        }
    };
    sqlx::query("UPDATE kanban_cards SET session_id = ? WHERE id = ?")
        .bind(&session_id)
        .bind(id)
        .execute(&state.db)
        .await?;
    let instruction = if decomposing {
        "Plan this card without editing files. Use kanban_show and kanban_create to create small Ready child cards with parent_id set to this card. Use depends_on to link child cards that must run in order."
    } else {
        "Work on this project task. Summarize the result upon completion. Use kanban_block with a reason if you need user input."
    };
    let prompt = format!(
        "{instruction}\n\nKanban card: {id}\nTitle: {}\n{}",
        card.title, card.description
    );
    state
        .scheduler
        .enqueue_kanban_turn(
            EnqueueTurnParams {
                project_id: project.into(),
                session_id,
                user_id: user.into(),
                prompt,
            },
            id,
        )
        .await?;
    let card = find_card(&mut *state.db.acquire().await?, project, id).await?;
    let _ = state.kanban_events.send(card);
    Ok(())
}

async fn finish(
    state: &AppState,
    project: &str,
    id: &str,
    lane: Column,
    reason: Option<String>,
    summary: Option<String>,
    terminal: bool,
) -> Result<(), AppError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let current = find_card(&mut tx, project, id).await?;
    let auto_move = current.column == Column::Running
        || (current.column == Column::Blocked
            && current.blocked_reason.as_deref() == Some("Awaiting tool approval"));
    if auto_move {
        sqlx::query("UPDATE kanban_cards SET lane = ?, blocked_reason = ?, result_summary = COALESCE(?, result_summary),
                     updated_at = ? WHERE id = ?")
            .bind(lane.as_str()).bind(reason).bind(summary).bind(Utc::now().to_rfc3339()).bind(id)
            .execute(&mut *tx).await?;
    }
    if terminal {
        sqlx::query(
            "UPDATE kanban_cards SET active_run = 0, run_token = NULL, decompose = 0 WHERE id = ?",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    let card = find_card(&mut tx, project, id).await?;
    tx.commit().await?;
    if current.column != card.column {
        let _ = state.kanban_events.send(card);
    }
    Ok(())
}

/// Install provider-specific MCP configuration only for the selected turn.
pub(crate) async fn prepare_invocation(
    db: &SqlitePool,
    project: &str,
    turn_id: &str,
    server_url: &str,
    invocation: &mut CliInvocation,
) -> Result<Option<McpConfigFile>, AppError> {
    let run = sqlx::query(
        "SELECT id, run_token, run_user_id, create_git_branch, git_branch_name, decompose
         FROM kanban_cards WHERE project_id = ? AND turn_id = ? AND active_run = 1",
    )
    .bind(project)
    .bind(turn_id)
    .fetch_optional(db)
    .await?;
    let Some(run) = run else {
        return Ok(None);
    };
    let user: String = run.try_get("run_user_id")?;
    authorized(db, project, &user).await?;
    let id: String = run.try_get("id")?;
    let branch_requested: bool = run.try_get("create_git_branch")?;
    let decomposing: bool = run.try_get("decompose")?;
    if branch_requested && !decomposing {
        let branch: String = run
            .try_get::<Option<String>, _>("git_branch_name")?
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("kanban/{}", &id[..8]));
        switch_branch(&invocation.working_dir, &branch).await?;
        sqlx::query("UPDATE kanban_cards SET git_branch_name = ? WHERE id = ?")
            .bind(branch)
            .bind(&id)
            .execute(db)
            .await?;
    }

    if !decomposing
        && let Ok(head) = git(&invocation.working_dir, &["rev-parse", "--verify", "HEAD"]).await
        && head.status.success()
    {
        sqlx::query("UPDATE kanban_cards SET git_base = ? WHERE id = ?")
            .bind(String::from_utf8_lossy(&head.stdout).trim())
            .bind(id)
            .execute(db)
            .await?;
    }

    let token: String = run.try_get("run_token")?;

    let exe = std::env::current_exe().map_err(|error| AppError::internal(error.to_string()))?;
    let command = exe.to_string_lossy();
    let args = [
        "mcp-kanban",
        "--project-id",
        project,
        "--server-url",
        server_url,
    ];
    // The provider passes the token in its environment to the stdio server.
    invocation
        .env
        .insert("CLAUDE_WEB_UI_KANBAN_RUN_TOKEN".into(), token);
    let options = match invocation.provider {
        CliProvider::ClaudeCode => vec![
            "--mcp-config".to_owned(),
            json!({"mcpServers":{"gateway-kanban":{"command":command,"args":args}}}).to_string(),
        ],
        CliProvider::Codex => vec![
            "-c".to_owned(),
            format!("mcp_servers.gateway-kanban.command={}", json!(command)),
            "-c".to_owned(),
            format!("mcp_servers.gateway-kanban.args={}", json!(args)),
            "-c".to_owned(),
            "mcp_servers.gateway-kanban.env_vars=[\"CLAUDE_WEB_UI_KANBAN_RUN_TOKEN\"]".to_owned(),
        ],
        CliProvider::Antigravity => {
            return McpConfigFile::install(
                &invocation.working_dir,
                json!({"command":command,"args":args}),
            )
            .map(Some);
        }
    };
    let offset = if invocation.provider == CliProvider::Codex {
        0
    } else {
        invocation.args.len() - 1
    };
    invocation.args.splice(offset..offset, options);
    Ok(None)
}

async fn git(workspace: &Path, args: &[&str]) -> Result<std::process::Output, AppError> {
    tokio::process::Command::new("git")
        .current_dir(workspace)
        .args(args)
        .output()
        .await
        .map_err(|error| AppError::internal(format!("Could not run git: {error}")))
}

pub(crate) async fn record_diff(
    db: &SqlitePool,
    turn_id: &str,
    workspace: &str,
) -> Result<(), AppError> {
    let base: Option<String> = sqlx::query_scalar(
        "SELECT git_base FROM kanban_cards WHERE turn_id = ? AND git_base IS NOT NULL",
    )
    .bind(turn_id)
    .fetch_optional(db)
    .await?;
    let Some(base) = base else {
        return Ok(());
    };
    let result = git(
        Path::new(workspace),
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            &base,
            "--",
        ],
    )
    .await?;
    if result.status.success() {
        const MAX_BYTES: usize = 1024 * 1024;
        let mut diff =
            String::from_utf8_lossy(&result.stdout[..result.stdout.len().min(MAX_BYTES)])
                .into_owned();
        if result.stdout.len() > MAX_BYTES {
            diff.push_str("\n[Diff truncated]\n");
        }
        sqlx::query("UPDATE kanban_cards SET git_diff = ? WHERE turn_id = ?")
            .bind(diff)
            .bind(turn_id)
            .execute(db)
            .await?;
    }
    Ok(())
}

async fn switch_branch(workspace: &Path, branch: &str) -> Result<(), AppError> {
    if branch.starts_with('-')
        || branch == "HEAD"
        || !git(workspace, &["check-ref-format", "--branch", branch])
            .await?
            .status
            .success()
    {
        return Err(AppError::bad_request("Invalid git branch name"));
    }
    let refname = format!("refs/heads/{branch}");
    let existing = git(workspace, &["show-ref", "--verify", "--quiet", &refname])
        .await?
        .status
        .success();
    let result = if existing {
        git(workspace, &["switch", "--no-guess", branch]).await?
    } else {
        git(workspace, &["switch", "-c", branch]).await?
    };
    if !result.status.success() {
        return Err(AppError::conflict(format!(
            "Could not switch to {branch}: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        )));
    }
    Ok(())
}

pub(crate) struct McpConfigFile {
    path: PathBuf,
    original: Option<Vec<u8>>,
    installed: Vec<u8>,
}

impl McpConfigFile {
    fn install(workspace: &Path, server: Value) -> Result<Self, AppError> {
        let install = || -> Result<Self, Box<dyn std::error::Error>> {
            let dir = workspace.join(".agents");
            std::fs::create_dir_all(&dir)?;
            let path = dir.join("mcp_config.json");
            if !dir.canonicalize()?.starts_with(workspace.canonicalize()?) || path.is_symlink() {
                return Err("MCP configuration must reside in the project directory".into());
            }
            let original = match std::fs::read(&path) {
                Ok(contents) => Some(contents),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            let mut config: Value = match original.as_deref() {
                Some(value) => serde_json::from_slice(value)?,
                None => json!({ "mcpServers": {} }),
            };
            let root = config
                .as_object_mut()
                .ok_or("MCP configuration must be an object")?;
            let servers = root
                .entry("mcpServers")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or("mcpServers must be an object")?;
            servers.insert("gateway-kanban".to_owned(), server);
            let installed = serde_json::to_vec_pretty(&config)?;
            std::fs::write(&path, &installed)?;
            Ok(Self {
                path,
                original,
                installed,
            })
        };
        install().map_err(|error| {
            AppError::internal(format!("Could not configure Antigravity MCP: {error}"))
        })
    }
}

impl Drop for McpConfigFile {
    fn drop(&mut self) {
        if !self.path.is_symlink()
            && std::fs::read(&self.path).ok().as_deref() == Some(&self.installed)
        {
            let result = match &self.original {
                Some(bytes) => std::fs::write(&self.path, bytes),
                None => std::fs::remove_file(&self.path),
            };
            if let Err(error) = result {
                tracing::warn!("Could not restore Antigravity MCP configuration: {error}");
            }
        }
    }
}
