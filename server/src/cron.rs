use crate::accounts::{authorize_turn_account, cron_account_env, resolve_project_cli_env};
use crate::auth::{AppError, ProjectContext, project_rbac_middleware};
use crate::chat::{
    EnqueueTurnParams, create_gateway_session, load_chat_session, resolve_provider_account_env,
};
use crate::traits::{MessengerPlatform, OutboundMessage};
use crate::{AppState, ServerError};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json, Router, middleware, routing::get};
use chrono::{DateTime, Utc};
use claude_web_ui_shared::accounts::CliProvider;
use claude_web_ui_shared::auth::Role;
use claude_web_ui_shared::chat::CreateChatSessionRequest;
use claude_web_ui_shared::cron::{
    CronDestination, CronJobConfig, CronJobDto, CronJobsResponse, CronMessengerPlatform,
    CronRunDto, CronTrigger, SetCronEnabledRequest,
};
use croner::Cron;
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};
use tokio::sync::{mpsc, oneshot};
use tokio_cron_scheduler::{Job, JobScheduler};
use uuid::Uuid;

pub(crate) fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/api/projects/{project_id}/cron", get(list).post(create))
        .route(
            "/api/projects/{project_id}/cron/{job_id}",
            axum::routing::put(update).patch(set_enabled).delete(delete),
        )
        .route(
            "/api/projects/{project_id}/cron/{job_id}/run",
            axum::routing::post(run_now),
        )
        .route("/api/projects/{project_id}/cron/runs", get(history))
        .route_layer(middleware::from_fn_with_state(
            state,
            project_rbac_middleware,
        ))
}

fn next_run(expression: &str, now: DateTime<Utc>) -> Result<i64, AppError> {
    let cron = expression
        .parse::<Cron>()
        .map_err(|error| AppError::bad_request(format!("Invalid cron expression: {error}")))?;
    let next = cron
        .find_next_occurrence(&now, false)
        .map_err(|error| AppError::bad_request(format!("No upcoming UTC run: {error}")))?;
    Ok(next.timestamp())
}

fn encode(value: &impl serde::Serialize) -> Result<String, AppError> {
    serde_json::to_string(value).map_err(|error| AppError::internal(error.to_string()))
}

fn decode<T: serde::de::DeserializeOwned>(value: String) -> Result<T, AppError> {
    serde_json::from_str(&value).map_err(|error| AppError::internal(error.to_string()))
}

fn job(row: &SqliteRow) -> Result<CronJobDto, AppError> {
    Ok(CronJobDto {
        id: row.try_get("id")?,
        owner_id: row.try_get("owner_id")?,
        config: decode(row.try_get("config_json")?)?,
        next_run_at: row
            .try_get::<Option<i64>, _>("next_run_at")?
            .and_then(|ts| DateTime::from_timestamp(ts, 0))
            .map(|at| at.to_rfc3339()),
    })
}

fn run(row: &SqliteRow) -> Result<CronRunDto, AppError> {
    let scheduled: bool = row.try_get("scheduled")?;
    Ok(CronRunDto {
        id: row.try_get("id")?,
        job_id: row.try_get("job_id")?,
        trigger: if scheduled {
            CronTrigger::Scheduled
        } else {
            CronTrigger::Manual
        },
        actor_id: row.try_get("actor_id")?,
        prompt: row.try_get("prompt")?,
        status: row
            .try_get::<String, _>("status")?
            .parse()
            .map_err(AppError::internal)?,
        queued_at: row.try_get("queued_at")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        session_id: row.try_get("session_id")?,
        summary: row.try_get("summary")?,
        error: row.try_get("error")?,
        notification_error: row.try_get("notification_error")?,
    })
}

async fn destinations(db: &SqlitePool, project_id: &str) -> Result<Vec<CronDestination>, AppError> {
    let rows = sqlx::query(
        "SELECT platform, chat_id, notification_thread_id
        FROM messenger_channels WHERE project_id = ? ORDER BY platform, chat_id",
    )
    .bind(project_id)
    .fetch_all(db)
    .await?;
    rows.iter()
        .map(|row| {
            let platform = match row.try_get::<&str, _>("platform")? {
                "telegram" => CronMessengerPlatform::Telegram,
                "discord" => CronMessengerPlatform::Discord,
                _ => return Err(AppError::internal("Unrecognized messenger platform")),
            };
            Ok(CronDestination {
                platform,
                chat_id: row.try_get("chat_id")?,
                thread_id: row.try_get("notification_thread_id")?,
            })
        })
        .collect()
}

async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
) -> Result<Json<CronJobsResponse>, AppError> {
    let rows =
        sqlx::query("SELECT * FROM cron_jobs WHERE project_id = ? AND deleted = 0 ORDER BY id")
            .bind(&ctx.project.id)
            .fetch_all(&state.db)
            .await?;
    Ok(Json(CronJobsResponse {
        writable: ctx.effective_role != Role::Viewer,
        jobs: rows.iter().map(job).collect::<Result<_, _>>()?,
        destinations: destinations(&state.db, &ctx.project.id).await?,
    }))
}

async fn history(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
) -> Result<Json<Vec<CronRunDto>>, AppError> {
    let rows = sqlx::query(
        "SELECT r.*, COALESCE(r.started_at, t.started_at) AS displayed_started_at,
            COALESCE(t.status, r.status) AS displayed_status
         FROM cron_runs r JOIN cron_jobs j ON j.id = r.job_id
         LEFT JOIN chat_turns t ON t.id = r.turn_id AND t.completed_at IS NULL
         WHERE j.project_id = ? ORDER BY r.queued_at DESC",
    )
    .bind(&ctx.project.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.iter()
            .map(|row| {
                let mut value = run(row)?;
                value.status = row
                    .try_get::<String, _>("displayed_status")?
                    .parse()
                    .map_err(AppError::internal)?;
                value.started_at = row.try_get("displayed_started_at")?;
                Ok(value)
            })
            .collect::<Result<_, AppError>>()?,
    ))
}

async fn load_job(db: &SqlitePool, project_id: &str, job_id: &str) -> Result<CronJobDto, AppError> {
    let row =
        sqlx::query("SELECT * FROM cron_jobs WHERE id = ? AND project_id = ? AND deleted = 0")
            .bind(job_id)
            .bind(project_id)
            .fetch_optional(db)
            .await?
            .ok_or_else(|| AppError::not_found("Cron schedule not found"))?;
    job(&row)
}

async fn resolve_target(
    db: &SqlitePool,
    project_id: &str,
    actor: &str,
    config: &CronJobConfig,
) -> Result<(CliProvider, Option<String>), AppError> {
    let (active, active_account, _) = resolve_project_cli_env(db, project_id).await?;
    let pinned_provider = match &config.pinned_session_id {
        Some(id) => Some(load_chat_session(db, project_id, id).await?.provider),
        None => None,
    };
    let provider = config.provider.or(pinned_provider).unwrap_or(active);
    if pinned_provider.is_some_and(|pinned| pinned != provider) {
        return Err(AppError::bad_request(
            "Pinned session belongs to a different CLI provider",
        ));
    }

    let account = match &config.account_id {
        Some(id) => {
            cron_account_env(db, provider, id).await?;
            Some(id.clone())
        }
        None if provider == active => active_account
            .filter(|account| account.provider == provider)
            .map(|account| account.id),
        None => {
            resolve_provider_account_env(db, project_id, provider)
                .await?
                .0
        }
    };
    authorize_turn_account(db, project_id, actor, account.as_deref()).await?;
    Ok((provider, account))
}

async fn validate(
    db: &SqlitePool,
    ctx: &ProjectContext,
    config: &mut CronJobConfig,
) -> Result<Option<i64>, AppError> {
    config.expression = config.expression.trim().to_owned();
    config.prompt = config.prompt.trim().to_owned();
    if config.prompt.is_empty() {
        return Err(AppError::bad_request("Prompt is required"));
    }
    let next = next_run(&config.expression, Utc::now())?;
    resolve_target(db, &ctx.project.id, &ctx.user.user.id, config).await?;
    if let Some(destination) = &config.notification
        && !destinations(db, &ctx.project.id)
            .await?
            .contains(destination)
    {
        return Err(AppError::bad_request(
            "Select a messenger channel linked to this project",
        ));
    }
    Ok(config.enabled.then_some(next))
}

async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Json(mut config): Json<CronJobConfig>,
) -> Result<(StatusCode, Json<CronJobDto>), AppError> {
    let _guard = state.cron_admission.lock().await;
    let next = validate(&state.db, &ctx, &mut config).await?;
    let id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO cron_jobs (id, project_id, owner_id, config_json, next_run_at) VALUES (?, ?, ?, ?, ?)")
        .bind(&id).bind(&ctx.project.id).bind(&ctx.user.user.id)
        .bind(encode(&config)?).bind(next).execute(&state.db).await?;
    Ok((
        StatusCode::CREATED,
        Json(load_job(&state.db, &ctx.project.id, &id).await?),
    ))
}

async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_, id)): Path<(String, String)>,
    Json(mut config): Json<CronJobConfig>,
) -> Result<Json<CronJobDto>, AppError> {
    let _guard = state.cron_admission.lock().await;
    load_job(&state.db, &ctx.project.id, &id).await?;
    let next = validate(&state.db, &ctx, &mut config).await?;
    // The user who last saves the configuration owns and authorizes future automatic runs.
    sqlx::query("UPDATE cron_jobs SET owner_id = ?, config_json = ?, next_run_at = ? WHERE id = ?")
        .bind(&ctx.user.user.id)
        .bind(encode(&config)?)
        .bind(next)
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(Json(load_job(&state.db, &ctx.project.id, &id).await?))
}

async fn set_enabled(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_, id)): Path<(String, String)>,
    Json(request): Json<SetCronEnabledRequest>,
) -> Result<Json<CronJobDto>, AppError> {
    let _guard = state.cron_admission.lock().await;
    let mut job = load_job(&state.db, &ctx.project.id, &id).await?;
    if job.config.enabled == request.enabled {
        return Ok(Json(job));
    }
    job.config.enabled = request.enabled;
    let (owner, next) = if request.enabled {
        (
            Some(ctx.user.user.id.clone()),
            validate(&state.db, &ctx, &mut job.config).await?,
        )
    } else {
        (job.owner_id, None)
    };
    sqlx::query("UPDATE cron_jobs SET owner_id = ?, config_json = ?, next_run_at = ? WHERE id = ?")
        .bind(owner)
        .bind(encode(&job.config)?)
        .bind(next)
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(Json(load_job(&state.db, &ctx.project.id, &id).await?))
}

async fn delete(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_, id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let _guard = state.cron_admission.lock().await;
    let changed = sqlx::query("UPDATE cron_jobs SET deleted = 1, next_run_at = NULL WHERE id = ? AND project_id = ? AND deleted = 0")
        .bind(id).bind(&ctx.project.id).execute(&state.db).await?.rows_affected();
    if changed == 0 {
        return Err(AppError::not_found("Cron schedule not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn run_now(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_, id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<CronRunDto>), AppError> {
    let _guard = state.cron_admission.lock().await;
    let job = load_job(&state.db, &ctx.project.id, &id).await?;
    let run_id = admit(&state, &ctx.project.id, &job, Some(&ctx.user.user.id), None).await?;
    let row = sqlx::query("SELECT * FROM cron_runs WHERE id = ?")
        .bind(run_id)
        .fetch_one(&state.db)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(run(&row)?)))
}

async fn admit(
    state: &AppState,
    project_id: &str,
    job: &CronJobDto,
    actor: Option<&str>,
    next: Option<i64>,
) -> Result<String, AppError> {
    let run_id = Uuid::new_v4().to_string();
    let mut tx = state.db.begin().await?;
    if let Some(next) = next {
        sqlx::query("UPDATE cron_jobs SET next_run_at = ? WHERE id = ?")
            .bind(next)
            .bind(&job.id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("INSERT INTO cron_runs (id, job_id, actor_id, scheduled, queued_at, prompt, notification_json)
        VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(&run_id).bind(&job.id).bind(actor).bind(next.is_some()).bind(Utc::now().to_rfc3339())
        .bind(&job.config.prompt).bind(job.config.notification.as_ref().map(encode).transpose()?)
        .execute(&mut *tx).await?;
    tx.commit().await?;

    if let Err(error) = enqueue(state, project_id, job, actor, &run_id).await {
        sqlx::query("UPDATE cron_runs SET status = 'error', error = ?, completed_at = ? WHERE id = ? AND turn_id IS NULL")
            .bind(error.message).bind(Utc::now().to_rfc3339()).bind(&run_id).execute(&state.db).await?;
    }
    Ok(run_id)
}

async fn enqueue(
    state: &AppState,
    project_id: &str,
    job: &CronJobDto,
    actor: Option<&str>,
    run_id: &str,
) -> Result<(), AppError> {
    let actor = actor.ok_or_else(|| AppError::forbidden("Schedule owner no longer exists"))?;
    let (provider, account_id) = resolve_target(&state.db, project_id, actor, &job.config).await?;
    let session_id = match &job.config.pinned_session_id {
        Some(id) => id.clone(),
        None => {
            create_gateway_session(
                &state.db,
                project_id,
                actor,
                CreateChatSessionRequest {
                    title: Some(format!(
                        "Cron: {}",
                        job.config.prompt.chars().take(72).collect::<String>()
                    )),
                    provider: Some(provider),
                    native_session_id: None,
                    approval_mode: None,
                },
            )
            .await?
            .id
        }
    };
    state
        .scheduler
        .enqueue_cron_turn(
            EnqueueTurnParams {
                project_id: project_id.to_owned(),
                session_id,
                user_id: actor.to_owned(),
                prompt: job.config.prompt.clone(),
            },
            run_id,
            provider,
            account_id,
        )
        .await?;
    Ok(())
}

async fn dispatch_due(state: &AppState) -> Result<(), AppError> {
    let _guard = state.cron_admission.lock().await;
    let now = Utc::now();
    let due = sqlx::query(
        "SELECT * FROM cron_jobs WHERE deleted = 0 AND next_run_at <= ? ORDER BY next_run_at, id",
    )
    .bind(now.timestamp())
    .fetch_all(&state.db)
    .await?;
    for row in due {
        let project_id: String = row.try_get("project_id")?;
        let job = job(&row)?;
        let next = next_run(&job.config.expression, now)?;
        admit(
            state,
            &project_id,
            &job,
            job.owner_id.as_deref(),
            Some(next),
        )
        .await?;
    }
    Ok(())
}

async fn reconcile_runs(db: &SqlitePool) -> Result<(), sqlx::Error> {
    // The caller holds the admission lock, so an unlinked row is no longer being submitted.
    sqlx::query(
        "UPDATE cron_runs SET status = 'error', completed_at = ?,
           error = 'The cron turn could not be queued or its chat session was deleted'
         WHERE completed_at IS NULL AND turn_id IS NULL",
    )
    .bind(Utc::now().to_rfc3339())
    .execute(db)
    .await?;
    sqlx::query(
        "UPDATE cron_runs AS r SET status = t.status, started_at = t.started_at,
            completed_at = t.completed_at, summary = t.result_text, error = t.error_message
         FROM chat_turns t
         WHERE r.turn_id = t.id AND r.completed_at IS NULL AND t.completed_at IS NOT NULL",
    )
    .execute(db)
    .await?;
    Ok(())
}

async fn notify(state: &AppState) -> Result<(), AppError> {
    let rows = sqlx::query(
        "SELECT r.*, j.project_id FROM cron_runs r JOIN cron_jobs j ON j.id = r.job_id
         WHERE r.completed_at IS NOT NULL AND r.notification_json IS NOT NULL
           AND r.notification_handled = 0 ORDER BY r.completed_at LIMIT 10",
    )
    .fetch_all(&state.db)
    .await?;
    for row in rows {
        let run = run(&row)?;
        let destination = decode(row.try_get("notification_json")?)?;
        let error = send_report(state, &run, destination, row.try_get("project_id")?)
            .await
            .err()
            .map(|err| err.message);
        sqlx::query(
            "UPDATE cron_runs SET notification_handled = 1, notification_error = ? WHERE id = ?",
        )
        .bind(error)
        .bind(&run.id)
        .execute(&state.db)
        .await?;
    }
    Ok(())
}

async fn send_report(
    state: &AppState,
    run: &CronRunDto,
    destination: CronDestination,
    project_id: &str,
) -> Result<(), AppError> {
    if !destinations(&state.db, project_id)
        .await?
        .contains(&destination)
    {
        return Err(AppError::forbidden(
            "Cron notification channel is no longer linked to this project",
        ));
    }
    let platform = match destination.platform {
        CronMessengerPlatform::Telegram => MessengerPlatform::Telegram,
        CronMessengerPlatform::Discord => MessengerPlatform::Discord,
    };
    let details = run
        .error
        .as_deref()
        .or(run.summary.as_deref())
        .unwrap_or("No output");
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        state.messenger.send_message(OutboundMessage {
            platform,
            chat_id: destination.chat_id,
            thread_id: destination.thread_id,
            text: format!(
                "Cron run {} ? {}\n{}",
                run.status.as_str(),
                run.prompt.chars().take(120).collect::<String>(),
                details.chars().take(1600).collect::<String>()
            ),
            buttons: Vec::new(),
        }),
    )
    .await
    .map_err(|_| AppError::internal("Cron notification timed out"))?
    .map_err(AppError::internal)?;
    Ok(())
}

pub(crate) async fn start(state: AppState) -> Result<oneshot::Sender<()>, ServerError> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE chat_turns SET status = 'error', completed_at = ?, error_message = 'Gateway restarted before the cron turn finished'
         WHERE id IN (SELECT turn_id FROM cron_runs WHERE completed_at IS NULL)
           AND completed_at IS NULL",
    ).bind(&now).execute(&state.db).await?;
    sqlx::query(
        "UPDATE cron_runs SET status = 'error', completed_at = ?, error = 'Gateway restarted or the chat session was deleted'
         WHERE completed_at IS NULL AND turn_id IS NULL",
    ).bind(&now).execute(&state.db).await?;
    reconcile_runs(&state.db).await?;

    // SQLite owns recurrence and history. The timer only wakes the dispatcher.
    let timer = JobScheduler::new().await.map_err(timer_error)?;
    let (tick_tx, mut tick_rx) = mpsc::channel(1);
    let job = Job::new_async("* * * * * *", move |_, _| {
        let _ = tick_tx.try_send(());
        Box::pin(async {})
    })
    .map_err(timer_error)?;
    timer.add(job).await.map_err(timer_error)?;
    timer.start().await.map_err(timer_error)?;

    let (stop, mut stopped) = oneshot::channel();
    tokio::spawn(async move {
        let mut timer = timer;
        let dispatch = async {
            while tick_rx.recv().await.is_some() {
                let result = async {
                    let result = dispatch_due(&state).await;
                    let _guard = state.cron_admission.lock().await;
                    reconcile_runs(&state.db).await?;
                    result?;
                    Ok::<_, AppError>(())
                }
                .await;
                if let Err(error) = result {
                    tracing::error!(?error, "Cron dispatcher failed");
                }
            }
        };
        let report = async {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if let Err(error) = notify(&state).await {
                    tracing::error!(?error, "Cron notification worker failed");
                }
            }
        };
        tokio::select! {
            _ = &mut stopped => {},
            _ = dispatch => {},
            _ = report => {},
        }
        if let Err(error) = timer.shutdown().await {
            tracing::error!(?error, "Cron timer shutdown failed");
        }
    });
    Ok(stop)
}

fn timer_error(error: tokio_cron_scheduler::JobSchedulerError) -> ServerError {
    ServerError::Config(format!("Cron timer: {error}"))
}
