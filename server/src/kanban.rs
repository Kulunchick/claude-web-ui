use crate::{
    AppState,
    auth::{AppError, ProjectContext, project_rbac_middleware},
};
use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    middleware,
    routing::{get, patch, post},
};
use chrono::Utc;
use claude_web_ui_shared::kanban::{
    CreateKanbanCardRequest, KanbanCardDto, KanbanColumn as Column, MoveKanbanCardRequest,
    UpdateKanbanCardRequest,
};
use sqlx::{Row, SqliteConnection, SqlitePool};
use uuid::Uuid;

mod mcp;
mod worker;
pub use mcp::stdio_bridge;
pub(crate) use worker::{prepare_invocation, record_diff};

pub(crate) fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/api/projects/{project_id}/kanban", get(board).post(create))
        .route("/api/projects/{project_id}/kanban/{card_id}", patch(update))
        .route(
            "/api/projects/{project_id}/kanban/{card_id}/diff",
            get(diff),
        )
        .route(
            "/api/projects/{project_id}/kanban/{card_id}/move",
            post(move_http),
        )
        .route(
            "/api/projects/{project_id}/kanban/{card_id}/decompose",
            post(decompose),
        )
        .layer(middleware::from_fn_with_state(
            state,
            project_rbac_middleware,
        ))
        .merge(mcp_router())
}

pub(crate) fn mcp_router() -> Router<AppState> {
    Router::new().route("/api/mcp/kanban/{project_id}", post(mcp::request))
}

const CARD_SELECT: &str = "
    SELECT c.*, t.status AS session_status,
        (SELECT json_group_array(d.blocker_id) FROM kanban_dependencies d
         WHERE d.card_id = c.id) AS depends_on,
        (SELECT json_group_array(d.blocker_id) FROM kanban_dependencies d
         JOIN kanban_cards b ON d.blocker_id = b.id
         WHERE d.card_id = c.id AND b.lane NOT IN ('review','done')) AS unresolved_blockers
    FROM kanban_cards c LEFT JOIN chat_turns t ON t.id = c.turn_id";

fn decode_card(row: sqlx::sqlite::SqliteRow) -> Result<KanbanCardDto, AppError> {
    fn parse<T: serde::de::DeserializeOwned>(value: String) -> Result<T, AppError> {
        serde_json::from_str(&value).map_err(|error| AppError::internal(error.to_string()))
    }
    fn parse_enum<T: serde::de::DeserializeOwned>(
        value: Option<String>,
    ) -> Result<Option<T>, AppError> {
        value
            .map(|value| parse(serde_json::Value::String(value).to_string()))
            .transpose()
    }
    Ok(KanbanCardDto {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        parent_id: row.try_get("parent_id")?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        column: parse_enum(Some(row.try_get("lane")?))?
            .ok_or_else(|| AppError::internal("missing lane"))?,
        position: row.try_get("position")?,
        depends_on: parse(row.try_get("depends_on")?)?,
        unresolved_blockers: parse(row.try_get("unresolved_blockers")?)?,
        assigned_cli: parse_enum(row.try_get("assigned_cli")?)?,
        create_git_branch: row.try_get("create_git_branch")?,
        git_branch_name: row.try_get("git_branch_name")?,
        session_id: row.try_get("session_id")?,
        session_status: parse_enum(row.try_get("session_status")?)?,
        blocked_reason: row.try_get("blocked_reason")?,
        result_summary: row.try_get("result_summary")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub async fn list_cards(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<Vec<KanbanCardDto>, AppError> {
    sqlx::query(&format!(
        "{CARD_SELECT} WHERE c.project_id = ? ORDER BY c.position, c.created_at, c.id"
    ))
    .bind(project_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(decode_card)
    .collect()
}

async fn find_card(
    conn: &mut SqliteConnection,
    project_id: &str,
    id: &str,
) -> Result<KanbanCardDto, AppError> {
    sqlx::query(&format!(
        "{CARD_SELECT} WHERE c.project_id = ? AND c.id = ?"
    ))
    .bind(project_id)
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or_else(|| AppError::not_found("Kanban card not found"))
    .and_then(decode_card)
}

async fn replace_dependencies(
    conn: &mut SqliteConnection,
    project_id: &str,
    id: &str,
    blockers: &[String],
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM kanban_dependencies WHERE card_id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    for blocker in blockers {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM kanban_cards WHERE project_id = ? AND id = ?)",
        )
        .bind(project_id)
        .bind(blocker)
        .fetch_one(&mut *conn)
        .await?;
        if !exists {
            return Err(AppError::bad_request(
                "dependency must be a card in this project",
            ));
        }
        let cycle: bool = sqlx::query_scalar(
            "WITH RECURSIVE dependencies(id) AS (SELECT ? UNION
                SELECT d.blocker_id FROM kanban_dependencies d JOIN dependencies r ON d.card_id = r.id)
             SELECT EXISTS(SELECT 1 FROM dependencies WHERE id = ?)"
        ).bind(blocker).bind(id).fetch_one(&mut *conn).await?;
        if cycle {
            return Err(AppError::bad_request("dependency would create a cycle"));
        }
        sqlx::query(
            "INSERT OR IGNORE INTO kanban_dependencies (card_id, blocker_id) VALUES (?, ?)",
        )
        .bind(id)
        .bind(blocker)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

fn title(value: &str) -> Result<&str, AppError> {
    let title = value.trim();
    if title.is_empty() {
        return Err(AppError::bad_request("title cannot be empty"));
    }
    Ok(title)
}

async fn create_card(
    state: &AppState,
    project: &str,
    user: &str,
    req: CreateKanbanCardRequest,
) -> Result<KanbanCardDto, AppError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(parent_id) = &req.parent_id {
        find_card(&mut tx, project, parent_id).await?;
    }
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let lane = if req.column == Column::Running {
        Column::Ready
    } else {
        req.column
    }
    .as_str();
    sqlx::query(
        "INSERT INTO kanban_cards
         (id, project_id, parent_id, title, description, lane, position, create_git_branch,
          git_branch_name, created_by, run_user_id, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?,
          (SELECT COALESCE(MAX(position)+1,0) FROM kanban_cards WHERE project_id = ? AND lane = ?),
          ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(project)
    .bind(req.parent_id)
    .bind(title(&req.title)?)
    .bind(req.description)
    .bind(lane)
    .bind(project)
    .bind(lane)
    .bind(req.create_git_branch)
    .bind(req.git_branch_name.filter(|_| req.create_git_branch))
    .bind(user)
    .bind(user)
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    replace_dependencies(&mut tx, project, &id, &req.depends_on).await?;
    let card = find_card(&mut tx, project, &id).await?;
    tx.commit().await?;
    Ok(card)
}

async fn update_card(
    state: &AppState,
    project: &str,
    id: &str,
    req: UpdateKanbanCardRequest,
) -> Result<KanbanCardDto, AppError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let current = find_card(&mut tx, project, id).await?;
    if let Some(blockers) = req.depends_on.as_ref() {
        if current.column == Column::Running {
            return Err(AppError::conflict(
                "dependencies cannot change while a card is running",
            ));
        }
        replace_dependencies(&mut tx, project, id, blockers).await?;
    }
    sqlx::query(
        "UPDATE kanban_cards SET title = ?, description = COALESCE(?, description),
         create_git_branch = COALESCE(?, create_git_branch),
         git_branch_name = CASE WHEN ? THEN NULL ELSE COALESCE(?, git_branch_name) END,
         updated_at = ? WHERE project_id = ? AND id = ?",
    )
    .bind(title(req.title.as_deref().unwrap_or(&current.title))?)
    .bind(req.description)
    .bind(req.create_git_branch)
    .bind(req.create_git_branch == Some(false))
    .bind(
        req.git_branch_name
            .filter(|_| req.create_git_branch.unwrap_or(current.create_git_branch)),
    )
    .bind(Utc::now().to_rfc3339())
    .bind(project)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    let card = find_card(&mut tx, project, id).await?;
    tx.commit().await?;
    Ok(card)
}

pub async fn move_card(
    server: &crate::GatewayServer,
    project: &str,
    id: &str,
    user: &str,
    req: MoveKanbanCardRequest,
) -> Result<KanbanCardDto, AppError> {
    // Caller authorizes the user against this project, as the HTTP route does via project RBAC.
    move_inner(&server.state, project, id, user, req).await
}

async fn move_inner(
    state: &AppState,
    project: &str,
    id: &str,
    user: &str,
    req: MoveKanbanCardRequest,
) -> Result<KanbanCardDto, AppError> {
    move_expected(state, project, id, user, None, req).await
}

pub(crate) async fn move_expected(
    state: &AppState,
    project: &str,
    id: &str,
    user: &str,
    expected: Option<Column>,
    req: MoveKanbanCardRequest,
) -> Result<KanbanCardDto, AppError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let current = find_card(&mut tx, project, id).await?;
    if expected.is_some_and(|expected| expected != current.column) {
        return Err(AppError::conflict("card has moved since this notification"));
    }
    let target = match (req.column, current.column) {
        (Column::Running, Column::Running) => Column::Running,
        (Column::Running, _) => Column::Ready,
        (other, _) => other,
    };
    if current.column == Column::Running
        && !matches!(target, Column::Running | Column::Blocked | Column::Review)
    {
        return Err(AppError::conflict(
            "a running card can only move to Blocked or Review",
        ));
    }
    let active: bool = sqlx::query_scalar("SELECT active_run FROM kanban_cards WHERE id = ?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if active && matches!(target, Column::Ready | Column::Triage) {
        return Err(AppError::conflict(
            "wait for the current turn to finish before requeueing",
        ));
    }

    let mut destination: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM kanban_cards WHERE project_id = ? AND lane = ? AND id <> ?
         ORDER BY position, created_at, id",
    )
    .bind(project)
    .bind(target.as_str())
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    let position = req
        .position
        .unwrap_or(destination.len() as i64)
        .clamp(0, destination.len() as i64) as usize;
    destination.insert(position, id.to_owned());
    for (position, card_id) in destination.iter().enumerate() {
        sqlx::query("UPDATE kanban_cards SET position = ? WHERE id = ?")
            .bind(position as i64)
            .bind(card_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "UPDATE kanban_cards SET lane = ?,
            run_user_id = CASE WHEN ? = 'ready' THEN ? ELSE run_user_id END,
            blocked_reason = CASE WHEN ? = 'blocked' THEN COALESCE(?, blocked_reason) ELSE NULL END,
            result_summary = COALESCE(?, result_summary), updated_at = ? WHERE id = ?",
    )
    .bind(target.as_str())
    .bind(target.as_str())
    .bind(user)
    .bind(target.as_str())
    .bind(req.blocked_reason)
    .bind(req.result_summary)
    .bind(Utc::now().to_rfc3339())
    .bind(id)
    .execute(&mut *tx)
    .await?;
    let card = find_card(&mut tx, project, id).await?;
    tx.commit().await?;
    if current.column != card.column {
        let _ = state.kanban_events.send(card.clone());
    }
    Ok(card)
}

async fn board(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
) -> Result<Json<Vec<KanbanCardDto>>, AppError> {
    Ok(Json(list_cards(&state.db, &ctx.project.id).await?))
}

#[derive(serde::Serialize)]
struct CardDiff {
    unified: Option<String>,
}

async fn diff(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_, id)): Path<(String, String)>,
) -> Result<Json<CardDiff>, AppError> {
    let unified =
        sqlx::query_scalar("SELECT git_diff FROM kanban_cards WHERE project_id = ? AND id = ?")
            .bind(&ctx.project.id)
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::not_found("Kanban card not found"))?;
    Ok(Json(CardDiff { unified }))
}

async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Json(req): Json<CreateKanbanCardRequest>,
) -> Result<(StatusCode, Json<KanbanCardDto>), AppError> {
    Ok((
        StatusCode::CREATED,
        Json(create_card(&state, &ctx.project.id, &ctx.user.user.id, req).await?),
    ))
}

async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_, id)): Path<(String, String)>,
    Json(req): Json<UpdateKanbanCardRequest>,
) -> Result<Json<KanbanCardDto>, AppError> {
    Ok(Json(update_card(&state, &ctx.project.id, &id, req).await?))
}

async fn move_http(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_, id)): Path<(String, String)>,
    Json(req): Json<MoveKanbanCardRequest>,
) -> Result<Json<KanbanCardDto>, AppError> {
    Ok(Json(
        move_inner(&state, &ctx.project.id, &id, &ctx.user.user.id, req).await?,
    ))
}

async fn decompose(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_, id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let changed = sqlx::query(
        "UPDATE kanban_cards SET decompose = 1, run_user_id = ?, updated_at = ?
         WHERE project_id = ? AND id = ? AND lane = 'triage' AND active_run = 0",
    )
    .bind(&ctx.user.user.id)
    .bind(Utc::now().to_rfc3339())
    .bind(&ctx.project.id)
    .bind(id)
    .execute(&state.db)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(AppError::conflict(
            "only idle Triage cards can be decomposed",
        ));
    }
    Ok(StatusCode::ACCEPTED)
}

pub(crate) fn start(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(worker::run(state))
}
