use crate::accounts::{bridge_pty_websocket, resolve_project_cli_env};
use crate::auth::{project_rbac_middleware, record_audit_log, AppError, ProjectContext};
use crate::traits::{CliInvocation, CliRunner, PtyInvocation, TurnEvent};
use crate::{AppState, GatewayConfig};
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use claude_web_ui_shared::accounts::CliProvider;
use claude_web_ui_shared::auth::Role;
use claude_web_ui_shared::chat::{
    ApprovalMode, CancelTurnResponse, ChatSessionDetailDto, ChatSessionDto, ChatStreamEnvelope,
    ChatTurnDto, ChatTurnStatus, CreateChatSessionRequest, NativeTranscriptEntryDto,
    PendingPermissionDto, ResolvePermissionRequest, SchedulerStatusDto, SendChatTurnRequest,
    SendChatTurnResponse, UpdateChatSessionRequest,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path as StdPath, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{broadcast, mpsc, oneshot};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AdapterTurnRequest {
    pub prompt: String,
    pub resume_session_id: Option<String>,
    pub working_dir: PathBuf,
    pub env: BTreeMap<String, String>,
    pub approval_mode: ApprovalMode,
}

pub trait CliAdapter: Send + Sync {
    fn provider(&self) -> CliProvider;
    fn build_invocation(&self, req: &AdapterTurnRequest) -> CliInvocation;
    fn build_pty_invocation(
        &self,
        resume_session_id: Option<&str>,
        working_dir: PathBuf,
        env: BTreeMap<String, String>,
        cols: u16,
        rows: u16,
    ) -> PtyInvocation;
    fn parse_stream_line(&self, line: &str) -> Vec<TurnEvent>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudeCodeAdapter;

impl CliAdapter for ClaudeCodeAdapter {
    fn provider(&self) -> CliProvider {
        CliProvider::ClaudeCode
    }

    fn build_invocation(&self, req: &AdapterTurnRequest) -> CliInvocation {
        let mut args = vec!["-p".to_string()];
        if let Some(ref sid) = req.resume_session_id {
            args.push("--resume".to_string());
            args.push(sid.clone());
        }
        args.extend([
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--verbose".to_string(),
        ]);
        if req.approval_mode == ApprovalMode::FullAuto {
            args.push("--dangerously-skip-permissions".to_string());
        }
        args.push(req.prompt.clone());
        CliInvocation {
            provider: CliProvider::ClaudeCode,
            program: "claude".to_string(),
            args,
            working_dir: req.working_dir.clone(),
            env: req.env.clone(),
        }
    }

    fn build_pty_invocation(
        &self,
        resume_session_id: Option<&str>,
        working_dir: PathBuf,
        env: BTreeMap<String, String>,
        cols: u16,
        rows: u16,
    ) -> PtyInvocation {
        let mut args = Vec::new();
        if let Some(sid) = resume_session_id {
            args.push("--resume".to_string());
            args.push(sid.to_string());
        }
        PtyInvocation {
            provider: CliProvider::ClaudeCode,
            program: "claude".to_string(),
            args,
            working_dir,
            env,
            cols,
            rows,
        }
    }

    fn parse_stream_line(&self, line: &str) -> Vec<TurnEvent> {
        parse_claude_stream_line(line)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CodexAdapter;

impl CliAdapter for CodexAdapter {
    fn provider(&self) -> CliProvider {
        CliProvider::Codex
    }

    fn build_invocation(&self, req: &AdapterTurnRequest) -> CliInvocation {
        let mut args = vec!["exec".to_string()];
        if let Some(ref sid) = req.resume_session_id {
            args.push("resume".to_string());
            args.push(sid.clone());
        }
        args.push("--json".to_string());
        if req.approval_mode == ApprovalMode::FullAuto {
            args.push("--full-auto".to_string());
        }
        args.push(req.prompt.clone());
        CliInvocation {
            provider: CliProvider::Codex,
            program: "codex".to_string(),
            args,
            working_dir: req.working_dir.clone(),
            env: req.env.clone(),
        }
    }

    fn build_pty_invocation(
        &self,
        resume_session_id: Option<&str>,
        working_dir: PathBuf,
        env: BTreeMap<String, String>,
        cols: u16,
        rows: u16,
    ) -> PtyInvocation {
        let mut args = Vec::new();
        if let Some(sid) = resume_session_id {
            args.push("resume".to_string());
            args.push(sid.to_string());
        }
        PtyInvocation {
            provider: CliProvider::Codex,
            program: "codex".to_string(),
            args,
            working_dir,
            env,
            cols,
            rows,
        }
    }

    fn parse_stream_line(&self, line: &str) -> Vec<TurnEvent> {
        parse_codex_stream_line(line)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct AntigravityAdapter;

impl CliAdapter for AntigravityAdapter {
    fn provider(&self) -> CliProvider {
        CliProvider::Antigravity
    }

    fn build_invocation(&self, req: &AdapterTurnRequest) -> CliInvocation {
        let mut args = vec!["--json".to_string()];
        if let Some(ref sid) = req.resume_session_id {
            args.push("--resume".to_string());
            args.push(sid.clone());
        }
        if req.approval_mode == ApprovalMode::FullAuto {
            args.push("--full-auto".to_string());
        }
        args.push(req.prompt.clone());
        CliInvocation {
            provider: CliProvider::Antigravity,
            program: "agy".to_string(),
            args,
            working_dir: req.working_dir.clone(),
            env: req.env.clone(),
        }
    }

    fn build_pty_invocation(
        &self,
        resume_session_id: Option<&str>,
        working_dir: PathBuf,
        env: BTreeMap<String, String>,
        cols: u16,
        rows: u16,
    ) -> PtyInvocation {
        let mut args = Vec::new();
        if let Some(sid) = resume_session_id {
            args.push("--resume".to_string());
            args.push(sid.to_string());
        }
        PtyInvocation {
            provider: CliProvider::Antigravity,
            program: "agy".to_string(),
            args,
            working_dir,
            env,
            cols,
            rows,
        }
    }

    fn parse_stream_line(&self, line: &str) -> Vec<TurnEvent> {
        parse_generic_stream_line(line)
    }
}

pub fn adapter_for(provider: CliProvider) -> Box<dyn CliAdapter> {
    match provider {
        CliProvider::ClaudeCode => Box::new(ClaudeCodeAdapter),
        CliProvider::Codex => Box::new(CodexAdapter),
        CliProvider::Antigravity => Box::new(AntigravityAdapter),
    }
}

#[derive(Debug, Clone)]
pub struct EnqueueTurnParams {
    pub project_id: String,
    pub session_id: String,
    pub user_id: String,
    pub prompt: String,
}

struct ActiveTurnControl {
    turn_id: String,
    session_id: String,
    project_id: String,
    cancel_tx: Option<oneshot::Sender<()>>,
    approval_tx: mpsc::UnboundedSender<bool>,
    pending_permission: Option<PendingPermissionDto>,
}

#[derive(Default)]
struct SchedulerInner {
    active: Option<ActiveTurnControl>,
    queue: VecDeque<String>,
    cancelled_queued: HashSet<String>,
}

struct QueuedTurnJob {
    turn_id: String,
    session_id: String,
    project_id: String,
    prompt: String,
}

#[derive(Clone)]
pub struct ExecutionScheduler {
    db: SqlitePool,
    _config: Arc<GatewayConfig>,
    cli_runner: Arc<dyn CliRunner>,
    inner: Arc<Mutex<SchedulerInner>>,
    job_tx: mpsc::UnboundedSender<QueuedTurnJob>,
    events_tx: broadcast::Sender<ChatStreamEnvelope>,
    seq: Arc<AtomicU64>,
}

impl ExecutionScheduler {
    pub fn new(
        db: SqlitePool,
        config: Arc<GatewayConfig>,
        cli_runner: Arc<dyn CliRunner>,
    ) -> Self {
        let (job_tx, job_rx) = mpsc::unbounded_channel();
        let (events_tx, _) = broadcast::channel(512);
        let scheduler = Self {
            db,
            _config: config,
            cli_runner,
            inner: Arc::new(Mutex::new(SchedulerInner::default())),
            job_tx,
            events_tx,
            seq: Arc::new(AtomicU64::new(1)),
        };
        let worker_scheduler = scheduler.clone();
        tokio::spawn(async move {
            worker_scheduler.run_worker_loop(job_rx).await;
        });
        scheduler
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ChatStreamEnvelope> {
        self.events_tx.subscribe()
    }

    pub fn status(&self) -> SchedulerStatusDto {
        let guard = self.inner.lock().expect("scheduler mutex poisoned");
        SchedulerStatusDto {
            active_turn_id: guard.active.as_ref().map(|a| a.turn_id.clone()),
            active_session_id: guard.active.as_ref().map(|a| a.session_id.clone()),
            active_project_id: guard.active.as_ref().map(|a| a.project_id.clone()),
            queued_count: guard.queue.len(),
            pending_permission: guard
                .active
                .as_ref()
                .and_then(|a| a.pending_permission.clone()),
        }
    }

    /// Reusable helper for Tickets #7 (Kanban), #9 (Messenger), and #10 (Cron)
    /// to run a prompt against an existing or new project session.
    pub async fn submit_project_prompt(
        &self,
        project_id: &str,
        session_id: Option<&str>,
        user_id: &str,
        prompt: &str,
        approval_mode: Option<ApprovalMode>,
    ) -> Result<SendChatTurnResponse, AppError> {
        let resolved_session_id = match session_id {
            Some(sid) => {
                let exists: Option<String> = sqlx::query_scalar(
                    "SELECT id FROM chat_sessions WHERE id = ? AND project_id = ?",
                )
                .bind(sid)
                .bind(project_id)
                .fetch_optional(&self.db)
                .await?;
                exists.ok_or_else(|| AppError::not_found("chat session not found"))?
            }
            None => {
                let session = create_gateway_session(
                    &self.db,
                    project_id,
                    user_id,
                    CreateChatSessionRequest {
                        title: Some(truncate_title(prompt)),
                        provider: None,
                        native_session_id: None,
                        approval_mode,
                    },
                )
                .await?;
                session.id
            }
        };

        self.enqueue_turn(EnqueueTurnParams {
            project_id: project_id.to_string(),
            session_id: resolved_session_id,
            user_id: user_id.to_string(),
            prompt: prompt.to_string(),
        })
        .await
    }

    pub async fn enqueue_turn(
        &self,
        params: EnqueueTurnParams,
    ) -> Result<SendChatTurnResponse, AppError> {
        self.enqueue_project_turn(params, None).await
    }

    pub(crate) async fn enqueue_kanban_turn(
        &self, params: EnqueueTurnParams, card_id: &str,
    ) -> Result<SendChatTurnResponse, AppError> {
        self.enqueue_project_turn(params, Some(card_id)).await
    }

    async fn enqueue_project_turn(
        &self, params: EnqueueTurnParams, card_id: Option<&str>,
    ) -> Result<SendChatTurnResponse, AppError> {
        let prompt = params.prompt.trim().to_string();
        if prompt.is_empty() {
            return Err(AppError::bad_request("prompt cannot be empty"));
        }

        let session = load_chat_session(&self.db, &params.project_id, &params.session_id).await?;
        let (active_provider, active_account, _) =
            resolve_project_cli_env(&self.db, &params.project_id).await?;
        let provider = if session.provider != active_provider && session.native_session_id.is_none()
        {
            active_provider
        } else {
            session.provider
        };
        let account_id = active_account
            .filter(|a| a.provider == provider)
            .map(|a| a.id);

        let turn_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        let mut tx = self.db.begin().await?;

        sqlx::query(
            "INSERT INTO chat_turns (id, session_id, project_id, user_id, provider, account_id, prompt, status, events_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, '[]', ?)",
        )
        .bind(&turn_id)
        .bind(&params.session_id)
        .bind(&params.project_id)
        .bind(&params.user_id)
        .bind(provider.as_str())
        .bind(&account_id)
        .bind(&prompt)
        .bind(ChatTurnStatus::Queued.as_str())
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        sqlx::query("UPDATE chat_sessions SET updated_at = ? WHERE id = ?")
            .bind(&now)
            .bind(&params.session_id)
            .execute(&mut *tx)
            .await?;

        if let Some(card_id) = card_id {
            sqlx::query("UPDATE kanban_cards SET turn_id = ?, assigned_cli = ? WHERE id = ?")
                .bind(&turn_id).bind(provider.as_str()).bind(card_id).execute(&mut *tx).await?;
        }
        tx.commit().await?;

        let queue_position = {
            let mut guard = self.inner.lock().expect("scheduler mutex poisoned");
            guard.queue.push_back(turn_id.clone());
            let active_offset = usize::from(guard.active.is_some());
            guard.queue.len() - 1 + active_offset
        };

        let _ = self.job_tx.send(QueuedTurnJob {
            turn_id: turn_id.clone(),
            session_id: params.session_id.clone(),
            project_id: params.project_id.clone(),
            prompt,
        });

        let turn = load_chat_turn(&self.db, &params.project_id, &turn_id).await?;
        Ok(SendChatTurnResponse {
            turn,
            queue_position,
        })
    }

    pub async fn cancel_turn(
        &self,
        project_id: &str,
        turn_id: &str,
    ) -> Result<CancelTurnResponse, AppError> {
        let turn = load_chat_turn(&self.db, project_id, turn_id).await?;
        if matches!(
            turn.status,
            ChatTurnStatus::Completed | ChatTurnStatus::Error | ChatTurnStatus::Cancelled
        ) {
            return Ok(CancelTurnResponse {
                turn_id: turn.id,
                cancelled: false,
            });
        }

        let mut cancelled_active = false;
        {
            let mut guard = self.inner.lock().expect("scheduler mutex poisoned");
            if let Some(active) = guard.active.as_mut()
                && active.turn_id == turn_id
            {
                if let Some(tx) = active.cancel_tx.take() {
                    let _ = tx.send(());
                }
                let _ = active.approval_tx.send(false);
                cancelled_active = true;
            } else if guard.queue.iter().any(|id| id == turn_id) {
                guard.queue.retain(|id| id != turn_id);
                guard.cancelled_queued.insert(turn_id.to_string());
            }
        }

        if !cancelled_active {
            self.mark_turn_cancelled(project_id, &turn.session_id, turn_id, "Turn cancelled by user")
                .await?;
        }

        Ok(CancelTurnResponse {
            turn_id: turn_id.to_string(),
            cancelled: true,
        })
    }

    pub async fn stop_session_turn(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<CancelTurnResponse, AppError> {
        let _ = load_chat_session(&self.db, project_id, session_id).await?;

        let target_turn_id: Option<String> = {
            let guard = self.inner.lock().expect("scheduler mutex poisoned");
            if let Some(active) = guard.active.as_ref()
                && active.project_id == project_id
                && active.session_id == session_id
            {
                Some(active.turn_id.clone())
            } else {
                None
            }
        };

        let turn_id = match target_turn_id {
            Some(id) => id,
            None => sqlx::query_scalar::<_, String>(
                "SELECT id FROM chat_turns
                 WHERE project_id = ? AND session_id = ? AND status IN ('queued', 'running', 'waiting_approval')
                 ORDER BY created_at DESC LIMIT 1",
            )
            .bind(project_id)
            .bind(session_id)
            .fetch_optional(&self.db)
            .await?
            .ok_or_else(|| AppError::not_found("no active or queued turn for this session"))?,
        };

        self.cancel_turn(project_id, &turn_id).await
    }

    pub async fn resolve_permission(
        &self,
        project_id: &str,
        turn_id: &str,
        req: ResolvePermissionRequest,
    ) -> Result<ChatTurnDto, AppError> {
        let _ = load_chat_turn(&self.db, project_id, turn_id).await?;
        self.dispatch_permission_decision(turn_id, req.request_id.as_deref(), req.approved)
            .await?;
        load_chat_turn(&self.db, project_id, turn_id).await
    }

    pub async fn resolve_permission_by_request_id(
        &self,
        request_id: &str,
        approved: bool,
    ) -> Result<ChatTurnDto, AppError> {
        let (project_id, turn_id) = {
            let guard = self.inner.lock().expect("scheduler mutex poisoned");
            let active = guard
                .active
                .as_ref()
                .ok_or_else(|| AppError::conflict("no active turn awaiting permission"))?;
            let pending = active
                .pending_permission
                .as_ref()
                .ok_or_else(|| AppError::conflict("active turn is not waiting for permission"))?;
            if pending.request_id != request_id {
                return Err(AppError::conflict("permission request_id does not match active request"));
            }
            (active.project_id.clone(), active.turn_id.clone())
        };
        self.dispatch_permission_decision(&turn_id, Some(request_id), approved)
            .await?;
        load_chat_turn(&self.db, &project_id, &turn_id).await
    }

    async fn dispatch_permission_decision(
        &self,
        turn_id: &str,
        request_id: Option<&str>,
        approved: bool,
    ) -> Result<(), AppError> {
        let approval_tx = {
            let mut guard = self.inner.lock().expect("scheduler mutex poisoned");
            let active = guard
                .active
                .as_mut()
                .ok_or_else(|| AppError::conflict("no active turn awaiting permission"))?;
            if active.turn_id != turn_id {
                return Err(AppError::conflict("turn is not currently active"));
            }
            let pending = active
                .pending_permission
                .as_ref()
                .ok_or_else(|| AppError::conflict("turn has no pending permission request"))?;
            if let Some(expected_req) = request_id
                && pending.request_id != expected_req
            {
                return Err(AppError::bad_request("permission request_id mismatch"));
            }
            active.pending_permission = None;
            active.approval_tx.clone()
        };

        sqlx::query(
            "UPDATE chat_turns SET status = ?, pending_permission_json = NULL WHERE id = ? AND status = ?",
        )
        .bind(ChatTurnStatus::Running.as_str())
        .bind(turn_id)
        .bind(ChatTurnStatus::WaitingApproval.as_str())
        .execute(&self.db)
        .await?;

        approval_tx
            .send(approved)
            .map_err(|_| AppError::conflict("turn runner is no longer accepting approval"))?;
        Ok(())
    }

    async fn run_worker_loop(self, mut job_rx: mpsc::UnboundedReceiver<QueuedTurnJob>) {
        while let Some(job) = job_rx.recv().await {
            let should_skip = {
                let mut guard = self.inner.lock().expect("scheduler mutex poisoned");
                guard.queue.retain(|id| id != &job.turn_id);
                guard.cancelled_queued.remove(&job.turn_id)
            };
            if should_skip {
                continue;
            }
            if let Err(err) = self.execute_single_turn(job).await {
                tracing::error!("execution scheduler turn failure: {:?}", err);
            }
        }
    }

    async fn execute_single_turn(&self, job: QueuedTurnJob) -> Result<(), AppError> {
        let session = load_chat_session(&self.db, &job.project_id, &job.session_id).await?;
        let project_workspace: String =
            sqlx::query_scalar("SELECT workspace_path FROM projects WHERE id = ?")
                .bind(&job.project_id)
                .fetch_one(&self.db)
                .await?;

        let (active_provider, active_account, active_env) =
            resolve_project_cli_env(&self.db, &job.project_id).await?;

        // Use session's provider unless the session has not started yet and project switched CLIs,
        // and always inject the active account env vars when provider matches.
        let effective_provider = session.provider;
        let (account_id, env) = if active_provider == effective_provider {
            (active_account.map(|a| a.id), active_env)
        } else {
            resolve_provider_account_env(&self.db, &job.project_id, effective_provider).await?
        };

        let started_at = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE chat_turns SET status = ?, account_id = ?, started_at = ? WHERE id = ?",
        )
        .bind(ChatTurnStatus::Running.as_str())
        .bind(&account_id)
        .bind(&started_at)
        .bind(&job.turn_id)
        .execute(&self.db)
        .await?;

        let adapter = adapter_for(effective_provider);
        let mut invocation = adapter.build_invocation(&AdapterTurnRequest {
            prompt: job.prompt.clone(),
            resume_session_id: session.native_session_id.clone(),
            working_dir: PathBuf::from(&project_workspace),
            env,
            approval_mode: session.approval_mode,
        });

        let _mcp_config = match crate::kanban::prepare_invocation(
            &self.db, &job.project_id, &job.turn_id, &self._config.kanban_mcp_url, &mut invocation,
        ).await {
            Ok(config) => config,
            Err(error) => {
                sqlx::query("UPDATE chat_turns SET status = 'error', error_message = ?, completed_at = ? WHERE id = ?")
                    .bind(&error.message).bind(Utc::now().to_rfc3339()).bind(&job.turn_id)
                    .execute(&self.db).await?;
                return Err(error);
            }
        };

        let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
        let (approval_tx, approval_rx) = mpsc::unbounded_channel::<bool>();
        let (raw_event_tx, mut raw_event_rx) = mpsc::unbounded_channel::<TurnEvent>();

        {
            let mut guard = self.inner.lock().expect("scheduler mutex poisoned");
            guard.active = Some(ActiveTurnControl {
                turn_id: job.turn_id.clone(),
                session_id: job.session_id.clone(),
                project_id: job.project_id.clone(),
                cancel_tx: Some(cancel_tx),
                approval_tx: approval_tx.clone(),
                pending_permission: None,
            });
        }

        let runner = self.cli_runner.clone();
        let mut runner_task = tokio::spawn(async move {
            runner
                .run_turn(invocation, raw_event_tx, approval_rx)
                .await
        });

        let mut events_acc: Vec<TurnEvent> = Vec::new();
        let mut final_status = ChatTurnStatus::Completed;
        let mut final_result_text = String::new();
        let mut final_error: Option<String> = None;

        loop {
            tokio::select! {
                _ = &mut cancel_rx => {
                    runner_task.abort();
                    final_status = ChatTurnStatus::Cancelled;
                    final_error = Some("Turn cancelled by user".to_string());
                    let cancel_event = TurnEvent::TurnError {
                        message: "Turn cancelled by user".to_string(),
                    };
                    events_acc.push(cancel_event.clone());
                    self.emit_envelope(
                        &job.project_id,
                        &job.session_id,
                        &job.turn_id,
                        ChatTurnStatus::Cancelled,
                        cancel_event,
                    );
                    break;
                }
                maybe_event = raw_event_rx.recv() => {
                    match maybe_event {
                        Some(ev) => {
                            let status_for_ev = match &ev {
                                TurnEvent::PermissionRequest { request_id, tool_name, input } => {
                                    if session.approval_mode == ApprovalMode::FullAuto {
                                        let _ = approval_tx.send(true);
                                        ChatTurnStatus::Running
                                    } else {
                                        let pending = PendingPermissionDto {
                                            request_id: request_id.clone(),
                                            tool_name: tool_name.clone(),
                                            input: input.clone(),
                                        };
                                        {
                                            let mut guard = self.inner.lock().expect("scheduler mutex poisoned");
                                            if let Some(active) = guard.active.as_mut()
                                                && active.turn_id == job.turn_id
                                            {
                                                active.pending_permission = Some(pending.clone());
                                            }
                                        }
                                        let pending_json = serde_json::to_string(&pending).ok();
                                        sqlx::query(
                                            "UPDATE chat_turns SET status = ?, pending_permission_json = ? WHERE id = ?",
                                        )
                                        .bind(ChatTurnStatus::WaitingApproval.as_str())
                                        .bind(pending_json)
                                        .bind(&job.turn_id)
                                        .execute(&self.db)
                                        .await?;
                                        ChatTurnStatus::WaitingApproval
                                    }
                                }
                                TurnEvent::TextDelta { text } => {
                                    final_result_text.push_str(text);
                                    ChatTurnStatus::Running
                                }
                                TurnEvent::TurnComplete { session_id: native_sid, result_text } => {
                                    if !result_text.is_empty() {
                                        final_result_text = result_text.clone();
                                    }
                                    if let Some(sid) = native_sid.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                                        let now = Utc::now().to_rfc3339();
                                        let _ = sqlx::query(
                                            "UPDATE chat_sessions SET native_session_id = ?, updated_at = ? WHERE id = ?",
                                        )
                                        .bind(sid)
                                        .bind(&now)
                                        .bind(&job.session_id)
                                        .execute(&self.db)
                                        .await;
                                    }
                                    final_status = ChatTurnStatus::Completed;
                                    ChatTurnStatus::Completed
                                }
                                TurnEvent::TurnError { message } => {
                                    final_status = ChatTurnStatus::Error;
                                    final_error = Some(message.clone());
                                    ChatTurnStatus::Error
                                }
                                _ => ChatTurnStatus::Running,
                            };

                            events_acc.push(ev.clone());
                            let events_json = serde_json::to_string(&events_acc).unwrap_or_else(|_| "[]".to_string());
                            let _ = sqlx::query("UPDATE chat_turns SET events_json = ? WHERE id = ?")
                                .bind(&events_json)
                                .bind(&job.turn_id)
                                .execute(&self.db)
                                .await;

                            self.emit_envelope(
                                &job.project_id,
                                &job.session_id,
                                &job.turn_id,
                                status_for_ev,
                                ev,
                            );
                        }
                        None => {
                            if let Ok(run_res) = (&mut runner_task).await
                                && let Err(err_msg) = run_res
                            {
                                final_status = ChatTurnStatus::Error;
                                final_error = Some(err_msg.clone());
                                let err_ev = TurnEvent::TurnError { message: err_msg };
                                events_acc.push(err_ev.clone());
                                self.emit_envelope(
                                    &job.project_id,
                                    &job.session_id,
                                    &job.turn_id,
                                    ChatTurnStatus::Error,
                                    err_ev,
                                );
                            }
                            break;
                        }
                    }
                }
                res = &mut runner_task => {
                    match res {
                        Ok(Ok(())) => {}
                        Ok(Err(err_msg)) => {
                            final_status = ChatTurnStatus::Error;
                            final_error = Some(err_msg.clone());
                            let err_ev = TurnEvent::TurnError { message: err_msg };
                            events_acc.push(err_ev.clone());
                            self.emit_envelope(
                                &job.project_id,
                                &job.session_id,
                                &job.turn_id,
                                ChatTurnStatus::Error,
                                err_ev,
                            );
                        }
                        Err(join_err) => {
                            if !join_err.is_cancelled() {
                                final_status = ChatTurnStatus::Error;
                                final_error = Some(format!("runner task panicked: {join_err}"));
                            }
                        }
                    }
                    // Drain any remaining buffered events synchronously
                    while let Ok(ev) = raw_event_rx.try_recv() {
                        let status_for_ev = match &ev {
                            TurnEvent::TextDelta { text } => {
                                final_result_text.push_str(text);
                                ChatTurnStatus::Running
                            }
                            TurnEvent::TurnComplete { session_id: native_sid, result_text } => {
                                if !result_text.is_empty() {
                                    final_result_text = result_text.clone();
                                }
                                if let Some(sid) = native_sid.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                                    let now = Utc::now().to_rfc3339();
                                    let _ = sqlx::query(
                                        "UPDATE chat_sessions SET native_session_id = ?, updated_at = ? WHERE id = ?",
                                    )
                                    .bind(sid)
                                    .bind(&now)
                                    .bind(&job.session_id)
                                    .execute(&self.db)
                                    .await;
                                }
                                final_status = ChatTurnStatus::Completed;
                                ChatTurnStatus::Completed
                            }
                            TurnEvent::TurnError { message } => {
                                final_status = ChatTurnStatus::Error;
                                final_error = Some(message.clone());
                                ChatTurnStatus::Error
                            }
                            _ => ChatTurnStatus::Running,
                        };
                        events_acc.push(ev.clone());
                        self.emit_envelope(
                            &job.project_id,
                            &job.session_id,
                            &job.turn_id,
                            status_for_ev,
                            ev,
                        );
                    }
                    break;
                }
            }
        }

        {
            let mut guard = self.inner.lock().expect("scheduler mutex poisoned");
            if guard
                .active
                .as_ref()
                .is_some_and(|a| a.turn_id == job.turn_id)
            {
                guard.active = None;
            }
        }

        if let Err(error) = crate::kanban::record_diff(
            &self.db, &job.turn_id, &project_workspace,
        ).await {
            tracing::warn!("Could not record Kanban git diff: {}", error.message);
        }

        let completed_at = Utc::now().to_rfc3339();
        let events_json = serde_json::to_string(&events_acc).unwrap_or_else(|_| "[]".to_string());
        sqlx::query(
            "UPDATE chat_turns
             SET status = ?, result_text = ?, error_message = ?, pending_permission_json = NULL, events_json = ?, completed_at = ?
             WHERE id = ?",
        )
        .bind(final_status.as_str())
        .bind(if final_result_text.is_empty() {
            None
        } else {
            Some(final_result_text)
        })
        .bind(final_error)
        .bind(&events_json)
        .bind(&completed_at)
        .bind(&job.turn_id)
        .execute(&self.db)
        .await?;

        sqlx::query("UPDATE chat_sessions SET updated_at = ? WHERE id = ?")
            .bind(&completed_at)
            .bind(&job.session_id)
            .execute(&self.db)
            .await?;

        Ok(())
    }

    async fn mark_turn_cancelled(
        &self,
        project_id: &str,
        session_id: &str,
        turn_id: &str,
        message: &str,
    ) -> Result<(), AppError> {
        let now = Utc::now().to_rfc3339();
        let turn = load_chat_turn(&self.db, project_id, turn_id).await?;
        let mut events = turn.events;
        let cancel_ev = TurnEvent::TurnError {
            message: message.to_string(),
        };
        events.push(cancel_ev.clone());
        let events_json = serde_json::to_string(&events).unwrap_or_else(|_| "[]".to_string());

        sqlx::query(
            "UPDATE chat_turns
             SET status = ?, error_message = ?, pending_permission_json = NULL, events_json = ?, completed_at = ?
             WHERE id = ?",
        )
        .bind(ChatTurnStatus::Cancelled.as_str())
        .bind(message)
        .bind(&events_json)
        .bind(&now)
        .bind(turn_id)
        .execute(&self.db)
        .await?;

        self.emit_envelope(
            project_id,
            session_id,
            turn_id,
            ChatTurnStatus::Cancelled,
            cancel_ev,
        );
        Ok(())
    }

    fn emit_envelope(
        &self,
        project_id: &str,
        session_id: &str,
        turn_id: &str,
        status: ChatTurnStatus,
        event: TurnEvent,
    ) {
        let env = ChatStreamEnvelope {
            seq: self.seq.fetch_add(1, Ordering::SeqCst),
            project_id: project_id.to_string(),
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            status,
            event,
            timestamp: Utc::now().to_rfc3339(),
        };
        let _ = self.events_tx.send(env);
    }
}

async fn resolve_provider_account_env(
    db: &SqlitePool,
    project_id: &str,
    provider: CliProvider,
) -> Result<(Option<String>, BTreeMap<String, String>), AppError> {
    let remembered: Option<String> = sqlx::query_scalar(
        "SELECT account_id FROM project_provider_accounts WHERE project_id = ? AND provider = ?",
    )
    .bind(project_id)
    .bind(provider.as_str())
    .fetch_optional(db)
    .await?;

    if let Some(aid) = remembered {
        let profile_dir: Option<String> =
            sqlx::query_scalar("SELECT profile_dir FROM cli_accounts WHERE id = ?")
                .bind(&aid)
                .fetch_optional(db)
                .await?;
        if let Some(dir) = profile_dir {
            return Ok((
                Some(aid),
                crate::accounts::account_env_vars(provider, StdPath::new(&dir)),
            ));
        }
    }
    Ok((None, BTreeMap::new()))
}

#[derive(Debug, Deserialize, Default)]
pub struct ListSessionsQuery {
    pub q: Option<String>,
    pub provider: Option<CliProvider>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ChatWsQuery {
    pub session_id: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ProjectTerminalQuery {
    pub session_id: Option<String>,
    pub mode: Option<String>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/api/projects/{project_id}/sessions",
            get(list_sessions).post(create_session),
        )
        .route(
            "/api/projects/{project_id}/sessions/{session_id}",
            get(get_session).patch(update_session).delete(delete_session),
        )
        .route(
            "/api/projects/{project_id}/sessions/{session_id}/turns",
            post(send_turn),
        )
        .route(
            "/api/projects/{project_id}/sessions/{session_id}/stop",
            post(stop_session),
        )
        .route(
            "/api/projects/{project_id}/turns/{turn_id}/cancel",
            post(cancel_turn_handler),
        )
        .route(
            "/api/projects/{project_id}/turns/{turn_id}/permission",
            post(resolve_permission_handler),
        )
        .route("/api/projects/{project_id}/scheduler", get(get_scheduler_status))
        .route("/api/projects/{project_id}/chat/ws", get(chat_ws_handler))
        .route(
            "/api/projects/{project_id}/terminal/ws",
            get(project_terminal_ws),
        )
        .layer(middleware::from_fn_with_state(state, project_rbac_middleware))
}

fn truncate_title(prompt: &str) -> String {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return "New Chat".to_string();
    }
    let first_line = trimmed.lines().next().unwrap_or(trimmed).trim();
    let mut out: String = first_line.chars().take(60).collect();
    if first_line.chars().count() > 60 {
        out.push_str("...");
    }
    out
}

pub(crate) async fn create_gateway_session(
    db: &SqlitePool,
    project_id: &str,
    user_id: &str,
    req: CreateChatSessionRequest,
) -> Result<ChatSessionDto, AppError> {
    let provider = match req.provider {
        Some(p) => p,
        None => resolve_project_cli_env(db, project_id).await?.0,
    };
    let approval_mode = req.approval_mode.unwrap_or(ApprovalMode::FullAuto);
    let title = req
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{} Session", provider.display_name()));

    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let native_id = req
        .native_session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    sqlx::query(
        "INSERT INTO chat_sessions (id, project_id, provider, native_session_id, title, approval_mode, source, created_by, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, 'gateway', ?, ?, ?)",
    )
    .bind(&id)
    .bind(project_id)
    .bind(provider.as_str())
    .bind(&native_id)
    .bind(&title)
    .bind(approval_mode.as_str())
    .bind(user_id)
    .bind(&now)
    .bind(&now)
    .execute(db)
    .await?;

    load_chat_session(db, project_id, &id).await
}

fn row_to_chat_session(row: &sqlx::sqlite::SqliteRow) -> Result<ChatSessionDto, AppError> {
    let p_str: String = row.try_get("provider")?;
    let provider: CliProvider = p_str
        .parse()
        .map_err(|e: String| AppError::internal(format!("corrupt session provider: {e}")))?;
    let mode_str: String = row.try_get("approval_mode")?;
    let approval_mode: ApprovalMode = mode_str
        .parse()
        .map_err(|e: String| AppError::internal(format!("corrupt approval mode: {e}")))?;

    Ok(ChatSessionDto {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        provider,
        native_session_id: row.try_get("native_session_id")?,
        title: row.try_get("title")?,
        approval_mode,
        source: row.try_get("source")?,
        transcript_path: row.try_get("transcript_path")?,
        message_count: 0,
        last_message_preview: None,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_chat_turn(row: &sqlx::sqlite::SqliteRow) -> Result<ChatTurnDto, AppError> {
    let p_str: String = row.try_get("provider")?;
    let provider: CliProvider = p_str
        .parse()
        .map_err(|e: String| AppError::internal(format!("corrupt turn provider: {e}")))?;
    let s_str: String = row.try_get("status")?;
    let status: ChatTurnStatus = s_str
        .parse()
        .map_err(|e: String| AppError::internal(format!("corrupt turn status: {e}")))?;
    let pending_json: Option<String> = row.try_get("pending_permission_json")?;
    let pending_permission = pending_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<PendingPermissionDto>(s).ok());
    let events_json: String = row.try_get("events_json")?;
    let events = serde_json::from_str::<Vec<TurnEvent>>(&events_json).unwrap_or_default();

    Ok(ChatTurnDto {
        id: row.try_get("id")?,
        session_id: row.try_get("session_id")?,
        project_id: row.try_get("project_id")?,
        user_id: row.try_get("user_id")?,
        provider,
        account_id: row.try_get("account_id")?,
        prompt: row.try_get("prompt")?,
        status,
        result_text: row.try_get("result_text")?,
        error_message: row.try_get("error_message")?,
        pending_permission,
        events,
        created_at: row.try_get("created_at")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
    })
}

async fn load_chat_session(
    db: &SqlitePool,
    project_id: &str,
    session_id: &str,
) -> Result<ChatSessionDto, AppError> {
    let row = sqlx::query(
        "SELECT id, project_id, provider, native_session_id, title, approval_mode, source, transcript_path, created_at, updated_at
         FROM chat_sessions WHERE id = ? AND project_id = ?",
    )
    .bind(session_id)
    .bind(project_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| AppError::not_found("chat session not found"))?;
    let mut dto = row_to_chat_session(&row)?;
    let turns = load_session_turns(db, project_id, session_id).await?;
    if !turns.is_empty() {
        dto.message_count = turns.len();
        dto.last_message_preview = turns
            .last()
            .map(|t| t.result_text.clone().unwrap_or_else(|| t.prompt.clone()));
    }
    if let Some(ref path_str) = dto.transcript_path {
        let entries = parse_native_jsonl_transcript(StdPath::new(path_str));
        if entries.len() > dto.message_count {
            dto.message_count = entries.len();
        }
        if dto.last_message_preview.is_none() {
            dto.last_message_preview = entries.last().map(|e| truncate_title(&e.text));
        }
    }
    Ok(dto)
}

async fn load_session_turns(
    db: &SqlitePool,
    project_id: &str,
    session_id: &str,
) -> Result<Vec<ChatTurnDto>, AppError> {
    let rows = sqlx::query(
        "SELECT id, session_id, project_id, user_id, provider, account_id, prompt, status, result_text, error_message, pending_permission_json, events_json, created_at, started_at, completed_at
         FROM chat_turns WHERE project_id = ? AND session_id = ? ORDER BY created_at ASC",
    )
    .bind(project_id)
    .bind(session_id)
    .fetch_all(db)
    .await?;
    rows.iter().map(row_to_chat_turn).collect()
}

async fn load_chat_turn(
    db: &SqlitePool,
    project_id: &str,
    turn_id: &str,
) -> Result<ChatTurnDto, AppError> {
    let row = sqlx::query(
        "SELECT id, session_id, project_id, user_id, provider, account_id, prompt, status, result_text, error_message, pending_permission_json, events_json, created_at, started_at, completed_at
         FROM chat_turns WHERE id = ? AND project_id = ?",
    )
    .bind(turn_id)
    .bind(project_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| AppError::not_found("chat turn not found"))?;
    row_to_chat_turn(&row)
}

fn encode_workspace_slug(workspace_path: &str) -> String {
    workspace_path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn normalize_path_for_match(raw: &str) -> String {
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(raw);
    let mut s = stripped.replace('\\', "/");
    while s.ends_with('/') && s.len() > 1 {
        s.pop();
    }
    if cfg!(windows) {
        s.to_ascii_lowercase()
    } else {
        s
    }
}

struct DiscoveredNativeSession {
    provider: CliProvider,
    native_session_id: String,
    title: String,
    transcript_path: PathBuf,
    updated_at: String,
}

fn collect_jsonl_files(dir: &StdPath, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

fn extract_text_from_value(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr
            .iter()
            .filter_map(|item| {
                if let Some(s) = item.as_str() {
                    Some(s.to_string())
                } else if let Some(t) = item.get("text").and_then(Value::as_str) {
                    Some(t.to_string())
                } else if let Some(t) = item.get("thinking").and_then(Value::as_str) {
                    Some(t.to_string())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => {
            if let Some(t) = map.get("text").and_then(Value::as_str) {
                t.to_string()
            } else if let Some(c) = map.get("content") {
                extract_text_from_value(c)
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

pub fn parse_native_jsonl_transcript(path: &StdPath) -> Vec<NativeTranscriptEntryDto> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let ts = val
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_string);

        // Claude Code JSONL shape: {"type": "user"|"assistant", "message": {"role": ..., "content": ...}}
        if let Some(msg) = val.get("message") {
            let role = msg
                .get("role")
                .and_then(Value::as_str)
                .or_else(|| val.get("type").and_then(Value::as_str))
                .unwrap_or("assistant")
                .to_string();
            if let Some(arr) = msg.get("content").and_then(Value::as_array) {
                for item in arr {
                    let kind = item.get("type").and_then(Value::as_str).unwrap_or("text");
                    match kind {
                        "thinking" => {
                            let text = item
                                .get("thinking")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            if !text.is_empty() {
                                out.push(NativeTranscriptEntryDto {
                                    role: "thinking".to_string(),
                                    text,
                                    tool_name: None,
                                    timestamp: ts.clone(),
                                });
                            }
                        }
                        "tool_use" => {
                            let name = item
                                .get("name")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            let text = item
                                .get("input")
                                .map(|v| v.to_string())
                                .unwrap_or_default();
                            out.push(NativeTranscriptEntryDto {
                                role: "tool_use".to_string(),
                                text,
                                tool_name: name,
                                timestamp: ts.clone(),
                            });
                        }
                        "tool_result" => {
                            let text = extract_text_from_value(
                                item.get("content").unwrap_or(&Value::Null),
                            );
                            out.push(NativeTranscriptEntryDto {
                                role: "tool_result".to_string(),
                                text,
                                tool_name: None,
                                timestamp: ts.clone(),
                            });
                        }
                        _ => {
                            let text = extract_text_from_value(item);
                            if !text.is_empty() {
                                out.push(NativeTranscriptEntryDto {
                                    role: role.clone(),
                                    text,
                                    tool_name: None,
                                    timestamp: ts.clone(),
                                });
                            }
                        }
                    }
                }
            } else {
                let text = extract_text_from_value(msg.get("content").unwrap_or(msg));
                if !text.is_empty() {
                    out.push(NativeTranscriptEntryDto {
                        role,
                        text,
                        tool_name: None,
                        timestamp: ts,
                    });
                }
            }
            continue;
        }

        // Codex JSONL shape: {"type":"response_item"|"event_msg","payload":{...}} or simple {"role":...,"content":...}
        if let Some(payload) = val.get("payload")
            && let Some(role) = payload.get("role").and_then(Value::as_str)
        {
            let text = extract_text_from_value(payload.get("content").unwrap_or(payload));
            if !text.is_empty() {
                out.push(NativeTranscriptEntryDto {
                    role: role.to_string(),
                    text,
                    tool_name: None,
                    timestamp: ts,
                });
            }
            continue;
        }

        if let Some(role) = val.get("role").and_then(Value::as_str) {
            let text = extract_text_from_value(
                val.get("content")
                    .or_else(|| val.get("text"))
                    .unwrap_or(&Value::Null),
            );
            if !text.is_empty() {
                out.push(NativeTranscriptEntryDto {
                    role: role.to_string(),
                    text,
                    tool_name: None,
                    timestamp: ts,
                });
            }
        }
    }
    out
}

fn inspect_native_session_file(
    provider: CliProvider,
    path: &StdPath,
    project_slug: &str,
    normalized_workspace: &str,
    encoded_workspace_slug: &str,
) -> Option<DiscoveredNativeSession> {
    let raw = std::fs::read_to_string(path).ok()?;
    let mut detected_cwd: Option<String> = None;
    let mut detected_session_id: Option<String> = None;

    for line in raw.lines().take(20) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if detected_cwd.is_none() {
            detected_cwd = val
                .get("cwd")
                .and_then(Value::as_str)
                .or_else(|| {
                    val.get("payload")
                        .and_then(|p| p.get("cwd"))
                        .and_then(Value::as_str)
                })
                .map(str::to_string);
        }
        if detected_session_id.is_none() {
            detected_session_id = val
                .get("sessionId")
                .or_else(|| val.get("session_id"))
                .and_then(Value::as_str)
                .or_else(|| {
                    val.get("payload")
                        .and_then(|p| p.get("id"))
                        .and_then(Value::as_str)
                })
                .map(str::to_string);
        }
    }

    let parent_dir_name = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    let cwd_matches = detected_cwd
        .as_deref()
        .is_some_and(|c| normalize_path_for_match(c) == normalized_workspace);
    let slug_matches = parent_dir_name == project_slug
        || parent_dir_name == encoded_workspace_slug
        || (!encoded_workspace_slug.is_empty()
            && parent_dir_name.eq_ignore_ascii_case(encoded_workspace_slug));

    if !cwd_matches && !slug_matches {
        return None;
    }

    let file_stem = path.file_stem()?.to_string_lossy().into_owned();
    let native_session_id = detected_session_id.unwrap_or(file_stem);
    let entries = parse_native_jsonl_transcript(path);
    let title = entries
        .iter()
        .find(|e| e.role == "user" && !e.text.trim().is_empty())
        .map(|e| truncate_title(&e.text))
        .unwrap_or_else(|| format!("Native {} ({})", provider.display_name(), native_session_id));

    let updated_at = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| DateTime::<Utc>::from(t).to_rfc3339())
        .unwrap_or_else(|| Utc::now().to_rfc3339());

    Some(DiscoveredNativeSession {
        provider,
        native_session_id,
        title,
        transcript_path: path.to_path_buf(),
        updated_at,
    })
}

async fn sync_native_sessions_for_project(
    state: &AppState,
    project_id: &str,
    workspace_path: &str,
    project_slug: &str,
) -> Result<(), AppError> {
    let normalized_ws = normalize_path_for_match(workspace_path);
    let encoded_slug = encode_workspace_slug(workspace_path);

    let mut discovered = Vec::new();
    for provider in CliProvider::ALL {
        let shared_root = state.config.data_dir.join("shared").join(provider.as_str());
        for sub in ["projects", "sessions"] {
            let mut files = Vec::new();
            collect_jsonl_files(&shared_root.join(sub), &mut files);
            for path in files {
                if let Some(item) = inspect_native_session_file(
                    provider,
                    &path,
                    project_slug,
                    &normalized_ws,
                    &encoded_slug,
                ) {
                    discovered.push(item);
                }
            }
        }
    }

    for item in discovered {
        let existing_id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM chat_sessions WHERE project_id = ? AND provider = ? AND native_session_id = ?",
        )
        .bind(project_id)
        .bind(item.provider.as_str())
        .bind(&item.native_session_id)
        .fetch_optional(&state.db)
        .await?;

        let path_str = item.transcript_path.to_string_lossy().into_owned();
        if let Some(id) = existing_id {
            sqlx::query(
                "UPDATE chat_sessions SET transcript_path = COALESCE(transcript_path, ?) WHERE id = ?",
            )
            .bind(&path_str)
            .bind(&id)
            .execute(&state.db)
            .await?;
        } else {
            let id = Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO chat_sessions (id, project_id, provider, native_session_id, title, approval_mode, source, transcript_path, created_by, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, 'full_auto', 'native', ?, NULL, ?, ?)",
            )
            .bind(&id)
            .bind(project_id)
            .bind(item.provider.as_str())
            .bind(&item.native_session_id)
            .bind(&item.title)
            .bind(&path_str)
            .bind(&item.updated_at)
            .bind(&item.updated_at)
            .execute(&state.db)
            .await?;
        }
    }

    Ok(())
}

async fn list_sessions(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Query(query): Query<ListSessionsQuery>,
) -> Result<Json<Vec<ChatSessionDto>>, AppError> {
    sync_native_sessions_for_project(
        &state,
        &ctx.project.id,
        &ctx.project.workspace_path,
        &ctx.project.slug,
    )
    .await?;

    let rows = match query.provider {
        Some(provider) => {
            sqlx::query(
                "SELECT id, project_id, provider, native_session_id, title, approval_mode, source, transcript_path, created_at, updated_at
                 FROM chat_sessions WHERE project_id = ? AND provider = ? ORDER BY updated_at DESC",
            )
            .bind(&ctx.project.id)
            .bind(provider.as_str())
            .fetch_all(&state.db)
            .await?
        }
        None => {
            sqlx::query(
                "SELECT id, project_id, provider, native_session_id, title, approval_mode, source, transcript_path, created_at, updated_at
                 FROM chat_sessions WHERE project_id = ? ORDER BY updated_at DESC",
            )
            .bind(&ctx.project.id)
            .fetch_all(&state.db)
            .await?
        }
    };

    let search = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase);

    let mut out = Vec::new();
    for row in rows {
        let dto = load_chat_session(&state.db, &ctx.project.id, row.try_get::<String, _>("id")?.as_str()).await?;
        if let Some(ref q) = search {
            let matches_title = dto.title.to_ascii_lowercase().contains(q);
            let matches_native = dto
                .native_session_id
                .as_deref()
                .is_some_and(|s| s.to_ascii_lowercase().contains(q));
            let matches_preview = dto
                .last_message_preview
                .as_deref()
                .is_some_and(|s| s.to_ascii_lowercase().contains(q));
            if !matches_title && !matches_native && !matches_preview {
                continue;
            }
        }
        out.push(dto);
    }

    Ok(Json(out))
}

async fn create_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Json(req): Json<CreateChatSessionRequest>,
) -> Result<(StatusCode, Json<ChatSessionDto>), AppError> {
    if ctx.effective_role == Role::Viewer {
        return Err(AppError::forbidden("viewer role cannot create chat sessions"));
    }

    let session = create_gateway_session(&state.db, &ctx.project.id, &ctx.user.user.id, req).await?;
    record_audit_log(
        &state,
        Some(&ctx.user.user.id),
        "create_chat_session",
        Some("chat_session"),
        Some(&session.id),
        Some(session.provider.as_str()),
    )
    .await?;

    Ok((StatusCode::CREATED, Json(session)))
}

async fn get_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, session_id)): Path<(String, String)>,
) -> Result<Json<ChatSessionDetailDto>, AppError> {
    let session = load_chat_session(&state.db, &ctx.project.id, &session_id).await?;
    let turns = load_session_turns(&state.db, &ctx.project.id, &session_id).await?;
    let native_messages = session
        .transcript_path
        .as_deref()
        .map(StdPath::new)
        .map(parse_native_jsonl_transcript)
        .unwrap_or_default();

    Ok(Json(ChatSessionDetailDto {
        session,
        turns,
        native_messages,
    }))
}

async fn update_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, session_id)): Path<(String, String)>,
    Json(req): Json<UpdateChatSessionRequest>,
) -> Result<Json<ChatSessionDto>, AppError> {
    if ctx.effective_role == Role::Viewer {
        return Err(AppError::forbidden("viewer role cannot update chat sessions"));
    }
    let existing = load_chat_session(&state.db, &ctx.project.id, &session_id).await?;
    let next_title = req
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or(existing.title);
    let next_mode = req.approval_mode.unwrap_or(existing.approval_mode);
    let now = Utc::now().to_rfc3339();

    sqlx::query("UPDATE chat_sessions SET title = ?, approval_mode = ?, updated_at = ? WHERE id = ?")
        .bind(&next_title)
        .bind(next_mode.as_str())
        .bind(&now)
        .bind(&session_id)
        .execute(&state.db)
        .await?;

    Ok(Json(load_chat_session(&state.db, &ctx.project.id, &session_id).await?))
}

async fn delete_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, session_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    if ctx.effective_role == Role::Viewer {
        return Err(AppError::forbidden("viewer role cannot delete chat sessions"));
    }
    let res = sqlx::query("DELETE FROM chat_sessions WHERE id = ? AND project_id = ?")
        .bind(&session_id)
        .bind(&ctx.project.id)
        .execute(&state.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::not_found("chat session not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn send_turn(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, session_id)): Path<(String, String)>,
    Json(req): Json<SendChatTurnRequest>,
) -> Result<(StatusCode, Json<SendChatTurnResponse>), AppError> {
    if ctx.effective_role == Role::Viewer {
        return Err(AppError::forbidden("viewer role cannot send prompts"));
    }

    let resp = state
        .scheduler
        .enqueue_turn(EnqueueTurnParams {
            project_id: ctx.project.id.clone(),
            session_id,
            user_id: ctx.user.user.id.clone(),
            prompt: req.prompt,
        })
        .await?;

    Ok((StatusCode::ACCEPTED, Json(resp)))
}

async fn stop_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, session_id)): Path<(String, String)>,
) -> Result<Json<CancelTurnResponse>, AppError> {
    if ctx.effective_role == Role::Viewer {
        return Err(AppError::forbidden("viewer role cannot stop active turns"));
    }
    let resp = state
        .scheduler
        .stop_session_turn(&ctx.project.id, &session_id)
        .await?;
    Ok(Json(resp))
}

async fn cancel_turn_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, turn_id)): Path<(String, String)>,
) -> Result<Json<CancelTurnResponse>, AppError> {
    if ctx.effective_role == Role::Viewer {
        return Err(AppError::forbidden("viewer role cannot cancel turns"));
    }
    let resp = state.scheduler.cancel_turn(&ctx.project.id, &turn_id).await?;
    Ok(Json(resp))
}

async fn resolve_permission_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, turn_id)): Path<(String, String)>,
    Json(req): Json<ResolvePermissionRequest>,
) -> Result<Json<ChatTurnDto>, AppError> {
    if ctx.effective_role == Role::Viewer {
        return Err(AppError::forbidden("viewer role cannot approve tool calls"));
    }
    let turn = state
        .scheduler
        .resolve_permission(&ctx.project.id, &turn_id, req)
        .await?;
    Ok(Json(turn))
}

async fn get_scheduler_status(
    State(state): State<AppState>,
    Extension(_ctx): Extension<ProjectContext>,
) -> Result<Json<SchedulerStatusDto>, AppError> {
    Ok(Json(state.scheduler.status()))
}

async fn chat_ws_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Query(query): Query<ChatWsQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let project_id = ctx.project.id.clone();
    let session_filter = query.session_id;
    Ok(ws.on_upgrade(move |socket| {
        handle_chat_ws(socket, state, project_id, session_filter)
    }))
}

async fn handle_chat_ws(
    mut socket: WebSocket,
    state: AppState,
    project_id: String,
    session_filter: Option<String>,
) {
    let mut rx = state.scheduler.subscribe();
    let initial = json!({
        "type": "scheduler_status",
        "status": state.scheduler.status(),
    });
    if socket
        .send(WsMessage::Text(initial.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            recv = rx.recv() => {
                match recv {
                    Ok(env) => {
                        if env.project_id != project_id {
                            continue;
                        }
                        if let Some(ref sid) = session_filter
                            && &env.session_id != sid
                        {
                            continue;
                        }
                        if let Ok(payload) = serde_json::to_string(&env)
                            && socket.send(WsMessage::Text(payload.into())).await.is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

async fn project_terminal_ws(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Query(query): Query<ProjectTerminalQuery>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Result<Response, AppError> {
    if ctx.effective_role == Role::Viewer {
        return Err(AppError::forbidden(
            "viewer role cannot open project PTY terminals",
        ));
    }
    let ws = ws.map_err(|e| AppError::bad_request(format!("websocket upgrade required: {e}")))?;

    let cols = query.cols.unwrap_or(90).clamp(20, 400);
    let rows = query.rows.unwrap_or(24).clamp(8, 200);
    let working_dir = PathBuf::from(&ctx.project.workspace_path);

    let (active_provider, _account, active_env) =
        resolve_project_cli_env(&state.db, &ctx.project.id).await?;

    let invocation = if let Some(ref sid) = query.session_id {
        let session = load_chat_session(&state.db, &ctx.project.id, sid).await?;
        let env = if session.provider == active_provider {
            active_env
        } else {
            resolve_provider_account_env(&state.db, &ctx.project.id, session.provider)
                .await?
                .1
        };
        adapter_for(session.provider).build_pty_invocation(
            session.native_session_id.as_deref(),
            working_dir,
            env,
            cols,
            rows,
        )
    } else if query.mode.as_deref() == Some("cli") {
        adapter_for(active_provider).build_pty_invocation(None, working_dir, active_env, cols, rows)
    } else {
        let shell_program = if cfg!(windows) {
            "powershell.exe".to_string()
        } else {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
        };
        PtyInvocation {
            provider: active_provider,
            program: shell_program,
            args: Vec::new(),
            working_dir,
            env: active_env,
            cols,
            rows,
        }
    };

    record_audit_log(
        &state,
        Some(&ctx.user.user.id),
        "open_project_terminal",
        Some("project"),
        Some(&ctx.project.id),
        query.session_id.as_deref(),
    )
    .await?;

    Ok(ws.on_upgrade(move |socket| bridge_pty_websocket(socket, state, invocation)))
}

/// Real production CLI turn execution over stdin/stdout JSON streaming.
pub async fn run_native_cli_turn(
    invocation: CliInvocation,
    event_tx: mpsc::UnboundedSender<TurnEvent>,
    mut approval_rx: mpsc::UnboundedReceiver<bool>,
) -> Result<(), String> {
    let mut cmd = Command::new(&invocation.program);
    cmd.args(&invocation.args)
        .current_dir(&invocation.working_dir)
        .envs(&invocation.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {e}", invocation.program))?;

    let mut stdin = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "missing stdout pipe".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "missing stderr pipe".to_string())?;

    let stderr_handle = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        let mut buf = String::new();
        while let Ok(Some(line)) = reader.next_line().await {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(&line);
        }
        buf
    });

    let adapter = adapter_for(invocation.provider);
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut saw_terminal = false;
    let mut captured_text = String::new();

    while let Ok(Some(line)) = stdout_lines.next_line().await {
        let parsed = adapter.parse_stream_line(&line);
        for ev in parsed {
            match &ev {
                TurnEvent::TextDelta { text } => captured_text.push_str(text),
                TurnEvent::TurnComplete { .. } | TurnEvent::TurnError { .. } => {
                    saw_terminal = true;
                }
                TurnEvent::PermissionRequest { request_id, .. } => {
                    let req_id = request_id.clone();
                    let _ = event_tx.send(ev);
                    let approved = approval_rx.recv().await.unwrap_or(false);
                    if let Some(ref mut writer) = stdin {
                        let decision = if approved { "allow" } else { "deny" };
                        let payload = json!({
                            "type": "control_response",
                            "response": {
                                "request_id": req_id,
                                "behavior": decision
                            }
                        });
                        let _ = writer.write_all(payload.to_string().as_bytes()).await;
                        let _ = writer.write_all(b"\n").await;
                        let _ = writer.flush().await;
                    }
                    continue;
                }
                _ => {}
            }
            let _ = event_tx.send(ev);
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("wait on {} failed: {e}", invocation.program))?;
    let stderr_text = stderr_handle.await.unwrap_or_default();

    if !saw_terminal {
        if status.success() {
            let _ = event_tx.send(TurnEvent::TurnComplete {
                session_id: None,
                result_text: captured_text,
            });
        } else {
            let msg = if stderr_text.trim().is_empty() {
                format!("{} exited with status {status}", invocation.program)
            } else {
                stderr_text
            };
            let _ = event_tx.send(TurnEvent::TurnError { message: msg });
        }
    }

    Ok(())
}

fn parse_claude_stream_line(line: &str) -> Vec<TurnEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(val) = serde_json::from_str::<Value>(trimmed) else {
        return vec![TurnEvent::TextDelta {
            text: format!("{trimmed}\n"),
        }];
    };

    let kind = val.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "assistant" => {
            let mut out = Vec::new();
            if let Some(arr) = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(Value::as_array)
            {
                for item in arr {
                    match item.get("type").and_then(Value::as_str).unwrap_or("") {
                        "thinking" => {
                            if let Some(t) = item.get("thinking").and_then(Value::as_str) {
                                out.push(TurnEvent::ThinkingDelta {
                                    text: t.to_string(),
                                });
                            }
                        }
                        "text" => {
                            if let Some(t) = item.get("text").and_then(Value::as_str) {
                                out.push(TurnEvent::TextDelta {
                                    text: t.to_string(),
                                });
                            }
                        }
                        "tool_use" => {
                            let id = item
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("tool")
                                .to_string();
                            let name = item
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("tool")
                                .to_string();
                            let input = item.get("input").cloned().unwrap_or(Value::Null);
                            if name.eq_ignore_ascii_case("Task") || name.eq_ignore_ascii_case("Agent") {
                                out.push(TurnEvent::SubagentStart {
                                    id: id.clone(),
                                    name: input
                                        .get("subagent_type")
                                        .or_else(|| input.get("description"))
                                        .and_then(Value::as_str)
                                        .unwrap_or(&name)
                                        .to_string(),
                                });
                            }
                            out.push(TurnEvent::ToolCallStart { id, name, input });
                        }
                        _ => {}
                    }
                }
            }
            out
        }
        "user" => {
            let mut out = Vec::new();
            if let Some(arr) = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(Value::as_array)
            {
                for item in arr {
                    if item.get("type").and_then(Value::as_str) == Some("tool_result") {
                        let id = item
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string();
                        let output = extract_text_from_value(
                            item.get("content").unwrap_or(&Value::Null),
                        );
                        let is_error = item
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        out.push(TurnEvent::ToolCallResult {
                            id,
                            output,
                            is_error,
                        });
                    }
                }
            }
            out
        }
        "control_request" => {
            let request_id = val
                .get("request_id")
                .and_then(Value::as_str)
                .unwrap_or("perm-1")
                .to_string();
            let req = val.get("request").unwrap_or(&val);
            let tool_name = req
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let input = req.get("input").cloned().unwrap_or(Value::Null);
            vec![TurnEvent::PermissionRequest {
                request_id,
                tool_name,
                input,
            }]
        }
        "result" => {
            let is_error = val.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            let result_text = val
                .get("result")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if is_error {
                vec![TurnEvent::TurnError {
                    message: result_text,
                }]
            } else {
                let session_id = val
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                vec![TurnEvent::TurnComplete {
                    session_id,
                    result_text,
                }]
            }
        }
        _ => Vec::new(),
    }
}

fn parse_codex_stream_line(line: &str) -> Vec<TurnEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(val) = serde_json::from_str::<Value>(trimmed) else {
        return vec![TurnEvent::TextDelta {
            text: format!("{trimmed}\n"),
        }];
    };

    let kind = val.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "item.completed" | "item.started" => {
            let item = val.get("item").unwrap_or(&val);
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
            match item_type {
                "agent_message" | "message" => {
                    let text = item
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if text.is_empty() {
                        Vec::new()
                    } else {
                        vec![TurnEvent::TextDelta { text }]
                    }
                }
                "reasoning" => {
                    let text = item
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if text.is_empty() {
                        Vec::new()
                    } else {
                        vec![TurnEvent::ThinkingDelta { text }]
                    }
                }
                "command_execution" | "mcp_tool_call" => {
                    let id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("cmd")
                        .to_string();
                    let name = item
                        .get("command")
                        .or_else(|| item.get("tool"))
                        .and_then(Value::as_str)
                        .unwrap_or(item_type)
                        .to_string();
                    if kind == "item.started" {
                        vec![TurnEvent::ToolCallStart {
                            id,
                            name,
                            input: item.clone(),
                        }]
                    } else {
                        let output = item
                            .get("aggregated_output")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let exit_code = item.get("exit_code").and_then(Value::as_i64).unwrap_or(0);
                        vec![TurnEvent::ToolCallResult {
                            id,
                            output,
                            is_error: exit_code != 0,
                        }]
                    }
                }
                _ => Vec::new(),
            }
        }
        "turn.completed" => {
            let session_id = val
                .get("thread_id")
                .or_else(|| val.get("session_id"))
                .and_then(Value::as_str)
                .map(str::to_string);
            vec![TurnEvent::TurnComplete {
                session_id,
                result_text: String::new(),
            }]
        }
        "error" | "turn.failed" => {
            let message = val
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("codex turn error")
                .to_string();
            vec![TurnEvent::TurnError { message }]
        }
        _ => parse_generic_stream_line(trimmed),
    }
}

fn parse_generic_stream_line(line: &str) -> Vec<TurnEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Ok(ev) = serde_json::from_str::<TurnEvent>(trimmed) {
        return vec![ev];
    }
    if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
        if let Some(text) = val.get("text").and_then(Value::as_str) {
            return vec![TurnEvent::TextDelta {
                text: text.to_string(),
            }];
        }
        return Vec::new();
    }
    vec![TurnEvent::TextDelta {
        text: format!("{trimmed}\n"),
    }]
}
