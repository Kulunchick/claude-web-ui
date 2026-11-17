use crate::AppState;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::body::Body;
use axum::extract::{FromRequestParts, Path, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::http::{Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use chrono::{Duration, Utc};
use claude_web_ui_shared::auth::{
    ApiErrorResponse, AssignProjectMemberRequest, AuthStatusResponse, AuthTokensResponse,
    BootstrapAdminRequest, CreateInviteRequest, CreateProjectRequest, InviteDto, LoginRequest,
    MeResponse, ProjectDto, ProjectMemberDto, RedeemInviteRequest, RefreshTokenRequest,
    RevokeSessionsResponse, Role, UpdateProjectRequest, UserDto,
};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

#[derive(Debug)]
pub struct AppError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl AppError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: message.into(),
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: message.into(),
        }
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "forbidden",
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: message.into(),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: message.into(),
        }
    }
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        match &err {
            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                AppError::conflict("resource already exists")
            }
            _ => AppError::internal(format!("database error: {err}")),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = ApiErrorResponse {
            error: self.message,
            code: self.code.to_string(),
        };
        (self.status, Json(body)).into_response()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenClaims {
    sub: String,
    sid: String,
    jti: String,
    role: Role,
    token_type: String,
    exp: usize,
    iat: usize,
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user: UserDto,
    pub session_id: String,
}

#[derive(Debug, Clone)]
pub struct AdminUser(pub AuthUser);

#[derive(Debug, Clone)]
pub struct ProjectContext {
    pub user: AuthUser,
    pub project: ProjectDto,
    pub effective_role: Role,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_bearer_or_query_token(parts)
            .ok_or_else(|| AppError::unauthorized("missing authorization token"))?;
        authenticate_access_token(state, &token).await
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth = AuthUser::from_request_parts(parts, state).await?;
        if auth.user.role != Role::Admin {
            return Err(AppError::forbidden("admin role required"));
        }
        Ok(AdminUser(auth))
    }
}

fn extract_bearer_or_query_token(parts: &Parts) -> Option<String> {
    extract_token_from_headers_and_query(&parts.headers, parts.uri.query())
}

async fn authenticate_access_token(state: &AppState, token: &str) -> Result<AuthUser, AppError> {
    let claims = decode_token(state, token)?;
    if claims.token_type != "access" {
        return Err(AppError::unauthorized("invalid access token type"));
    }

    let revoked: Option<Option<String>> = sqlx::query_scalar(
        "SELECT revoked_at FROM user_sessions WHERE id = ? AND user_id = ?",
    )
    .bind(&claims.sid)
    .bind(&claims.sub)
    .fetch_optional(&state.db)
    .await?;

    match revoked {
        None => return Err(AppError::unauthorized("session not found")),
        Some(Some(_)) => return Err(AppError::unauthorized("session revoked")),
        Some(None) => {}
    }

    let user = load_user_by_id(state, &claims.sub)
        .await?
        .ok_or_else(|| AppError::unauthorized("user not found"))?;

    Ok(AuthUser {
        user,
        session_id: claims.sid,
    })
}

fn decode_token(state: &AppState, token: &str) -> Result<TokenClaims, AppError> {
    let key = DecodingKey::from_secret(state.config.jwt_secret.as_bytes())
        .map_err(|e| AppError::internal(format!("jwt key failed: {e}")))?;
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    decode::<TokenClaims>(token, &key, &validation)
        .map(|data| data.claims)
        .map_err(|_| AppError::unauthorized("invalid or expired token"))
}

pub fn hash_password(password: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::internal(format!("password hashing failed: {e}")))
}

pub fn verify_password(password: &str, hash: &str) -> Result<bool, AppError> {
    let parsed = PasswordHash::new(hash)
        .map_err(|e| AppError::internal(format!("invalid password hash: {e}")))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

async fn load_user_by_id(state: &AppState, user_id: &str) -> Result<Option<UserDto>, AppError> {
    let row = sqlx::query("SELECT id, username, email, role, created_at FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;
    row.map(|r| row_to_user(&r)).transpose()
}

fn row_to_user(row: &sqlx::sqlite::SqliteRow) -> Result<UserDto, AppError> {
    let role_str: String = row.try_get("role")?;
    let role: Role = role_str
        .parse()
        .map_err(|e: String| AppError::internal(format!("corrupt role in db: {e}")))?;
    Ok(UserDto {
        id: row.try_get("id")?,
        username: row.try_get("username")?,
        email: row.try_get("email")?,
        role,
        created_at: row.try_get("created_at")?,
    })
}

fn row_to_project(row: &sqlx::sqlite::SqliteRow) -> Result<ProjectDto, AppError> {
    Ok(ProjectDto {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        slug: row.try_get("slug")?,
        workspace_path: row.try_get("workspace_path")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn load_accessible_projects(
    state: &AppState,
    user: &UserDto,
) -> Result<Vec<ProjectDto>, AppError> {
    let rows = if user.role == Role::Admin {
        sqlx::query("SELECT id, name, slug, workspace_path, created_at FROM projects ORDER BY created_at ASC")
            .fetch_all(&state.db)
            .await?
    } else {
        sqlx::query(
            r#"
            SELECT p.id, p.name, p.slug, p.workspace_path, p.created_at
            FROM projects p
            INNER JOIN project_memberships m ON m.project_id = p.id
            WHERE m.user_id = ?
            ORDER BY p.created_at ASC
            "#,
        )
        .bind(&user.id)
        .fetch_all(&state.db)
        .await?
    };
    rows.iter().map(row_to_project).collect()
}

pub(crate) async fn record_audit_log(
    state: &AppState,
    user_id: Option<&str>,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<&str>,
    details: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO audit_logs (id, user_id, action, target_type, target_id, details, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(user_id)
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .bind(details)
    .bind(Utc::now().to_rfc3339())
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn issue_session_tokens(
    state: &AppState,
    user: &UserDto,
) -> Result<AuthTokensResponse, AppError> {
    let now = Utc::now();
    let session_id = Uuid::new_v4().to_string();
    let refresh_jti = Uuid::new_v4().to_string();
    let refresh_exp = now + Duration::seconds(state.config.refresh_token_ttl_secs as i64);

    sqlx::query(
        "INSERT INTO user_sessions (id, user_id, refresh_token_id, expires_at, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&session_id)
    .bind(&user.id)
    .bind(&refresh_jti)
    .bind(refresh_exp.to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(&state.db)
    .await?;

    encode_token_pair(state, user, &session_id, &refresh_jti)
}

fn encode_token_pair(
    state: &AppState,
    user: &UserDto,
    session_id: &str,
    refresh_jti: &str,
) -> Result<AuthTokensResponse, AppError> {
    let now = Utc::now().timestamp() as usize;
    let access_exp = now + state.config.access_token_ttl_secs as usize;
    let refresh_exp = now + state.config.refresh_token_ttl_secs as usize;
    let key = EncodingKey::from_secret(state.config.jwt_secret.as_bytes())
        .map_err(|e| AppError::internal(format!("jwt key failed: {e}")))?;

    let access_claims = TokenClaims {
        sub: user.id.clone(),
        sid: session_id.to_string(),
        jti: Uuid::new_v4().to_string(),
        role: user.role,
        token_type: "access".to_string(),
        exp: access_exp,
        iat: now,
    };
    let refresh_claims = TokenClaims {
        sub: user.id.clone(),
        sid: session_id.to_string(),
        jti: refresh_jti.to_string(),
        role: user.role,
        token_type: "refresh".to_string(),
        exp: refresh_exp,
        iat: now,
    };

    let access_token = encode(&Header::default(), &access_claims, &key)
        .map_err(|e| AppError::internal(format!("jwt encode failed: {e}")))?;
    let refresh_token = encode(&Header::default(), &refresh_claims, &key)
        .map_err(|e| AppError::internal(format!("jwt encode failed: {e}")))?;

    Ok(AuthTokensResponse {
        access_token,
        refresh_token,
        token_type: "Bearer".to_string(),
        expires_in_secs: state.config.access_token_ttl_secs,
        user: user.clone(),
    })
}

pub fn router(state: AppState) -> Router<AppState> {
    let project_scoped = Router::new()
        .route(
            "/api/projects/{project_id}",
            get(get_project).patch(update_project).delete(delete_project),
        )
        .layer(middleware::from_fn_with_state(
            state,
            project_rbac_middleware,
        ));

    Router::new()
        .route("/api/auth/status", get(auth_status))
        .route("/api/auth/bootstrap", post(bootstrap_admin))
        .route("/api/auth/login", post(login))
        .route("/api/auth/refresh", post(refresh_token))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/me", get(me))
        .route("/api/auth/invites/redeem", post(redeem_invite))
        .route("/api/projects", get(list_projects).post(create_project))
        .route("/api/admin/users", get(list_users))
        .route(
            "/api/admin/users/{user_id}/revoke-sessions",
            post(revoke_user_sessions),
        )
        .route("/api/admin/invites", get(list_invites).post(create_invite))
        .route(
            "/api/admin/projects/{project_id}/members",
            get(list_project_members).post(assign_project_member),
        )
        .route(
            "/api/admin/projects/{project_id}/members/{user_id}",
            delete(remove_project_member),
        )
        .merge(project_scoped)
}

async fn auth_status(State(state): State<AppState>) -> Result<Json<AuthStatusResponse>, AppError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await?;
    Ok(Json(AuthStatusResponse {
        initialized: count > 0,
    }))
}

async fn bootstrap_admin(
    State(state): State<AppState>,
    Json(req): Json<BootstrapAdminRequest>,
) -> Result<(StatusCode, Json<AuthTokensResponse>), AppError> {
    let username = req.username.trim();
    let email = req.email.trim();
    if username.is_empty() || email.is_empty() {
        return Err(AppError::bad_request("username and email are required"));
    }
    if req.password.len() < 8 {
        return Err(AppError::bad_request(
            "password must be at least 8 characters",
        ));
    }

    let mut tx = state.db.begin().await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&mut *tx)
        .await?;
    if count > 0 {
        return Err(AppError::conflict("gateway is already initialized"));
    }

    let user_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let password_hash = hash_password(&req.password)?;

    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, created_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&user_id)
    .bind(username)
    .bind(email)
    .bind(&password_hash)
    .bind(Role::Admin.as_str())
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    let user = UserDto {
        id: user_id.clone(),
        username: username.to_string(),
        email: email.to_string(),
        role: Role::Admin,
        created_at: now,
    };
    record_audit_log(&state, Some(&user_id), "bootstrap_admin", Some("user"), Some(&user_id), None)
        .await?;
    let tokens = issue_session_tokens(&state, &user).await?;
    Ok((StatusCode::CREATED, Json(tokens)))
}

async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<AuthTokensResponse>, AppError> {
    let identifier = req.identifier.trim();
    if identifier.is_empty() || req.password.is_empty() {
        return Err(AppError::bad_request(
            "identifier and password are required",
        ));
    }

    let row = sqlx::query(
        "SELECT id, username, email, password_hash, role, created_at FROM users WHERE username = ? OR email = ?",
    )
    .bind(identifier)
    .bind(identifier)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::unauthorized("invalid credentials"))?;

    let password_hash: String = row.try_get("password_hash")?;
    if !verify_password(&req.password, &password_hash)? {
        return Err(AppError::unauthorized("invalid credentials"));
    }

    let user = row_to_user(&row)?;
    record_audit_log(&state, Some(&user.id), "login", Some("user"), Some(&user.id), None).await?;
    let tokens = issue_session_tokens(&state, &user).await?;
    Ok(Json(tokens))
}

async fn refresh_token(
    State(state): State<AppState>,
    Json(req): Json<RefreshTokenRequest>,
) -> Result<Json<AuthTokensResponse>, AppError> {
    let claims = decode_token(&state, &req.refresh_token)?;
    if claims.token_type != "refresh" {
        return Err(AppError::unauthorized("invalid refresh token type"));
    }

    let new_jti = Uuid::new_v4().to_string();
    let new_exp =
        (Utc::now() + Duration::seconds(state.config.refresh_token_ttl_secs as i64)).to_rfc3339();

    let result = sqlx::query(
        r#"
        UPDATE user_sessions
        SET refresh_token_id = ?, expires_at = ?
        WHERE id = ? AND user_id = ? AND refresh_token_id = ? AND revoked_at IS NULL
        "#,
    )
    .bind(&new_jti)
    .bind(&new_exp)
    .bind(&claims.sid)
    .bind(&claims.sub)
    .bind(&claims.jti)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::unauthorized(
            "refresh token revoked, expired, or already rotated",
        ));
    }

    let user = load_user_by_id(&state, &claims.sub)
        .await?
        .ok_or_else(|| AppError::unauthorized("user not found"))?;
    let tokens = encode_token_pair(&state, &user, &claims.sid, &new_jti)?;
    Ok(Json(tokens))
}

async fn logout(State(state): State<AppState>, auth: AuthUser) -> Result<StatusCode, AppError> {
    sqlx::query("UPDATE user_sessions SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
        .bind(Utc::now().to_rfc3339())
        .bind(&auth.session_id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn me(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<MeResponse>, AppError> {
    let projects = load_accessible_projects(&state, &auth.user).await?;
    Ok(Json(MeResponse {
        user: auth.user,
        projects,
    }))
}

async fn list_users(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<UserDto>>, AppError> {
    let rows = sqlx::query("SELECT id, username, email, role, created_at FROM users ORDER BY created_at ASC")
        .fetch_all(&state.db)
        .await?;
    let users: Result<Vec<UserDto>, AppError> = rows.iter().map(row_to_user).collect();
    Ok(Json(users?))
}

async fn revoke_user_sessions(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(user_id): Path<String>,
) -> Result<Json<RevokeSessionsResponse>, AppError> {
    let exists: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE id = ?")
        .bind(&user_id)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(AppError::not_found("user not found"));
    }

    let result = sqlx::query(
        "UPDATE user_sessions SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL",
    )
    .bind(Utc::now().to_rfc3339())
    .bind(&user_id)
    .execute(&state.db)
    .await?;

    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "revoke_sessions",
        Some("user"),
        Some(&user_id),
        None,
    )
    .await?;

    Ok(Json(RevokeSessionsResponse {
        revoked_count: result.rows_affected(),
    }))
}

pub async fn project_rbac_middleware(
    State(state): State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, AppError> {
    let token = {
        let headers = req.headers();
        let query = req.uri().query();
        extract_token_from_headers_and_query(headers, query)
            .ok_or_else(|| AppError::unauthorized("missing authorization token"))?
    };
    let auth = authenticate_access_token(&state, &token).await?;

    let project_id = extract_project_id_from_path(req.uri().path())
        .ok_or_else(|| AppError::bad_request("missing project_id in path"))?;

    let project_row = sqlx::query(
        "SELECT id, name, slug, workspace_path, created_at FROM projects WHERE id = ?",
    )
    .bind(&project_id)
    .fetch_optional(&state.db)
    .await?;

    let effective_role = if auth.user.role == Role::Admin {
        if project_row.is_none() {
            return Err(AppError::not_found("project not found"));
        }
        Role::Admin
    } else {
        let membership: Option<Option<String>> = sqlx::query_scalar(
            "SELECT role_override FROM project_memberships WHERE user_id = ? AND project_id = ?",
        )
        .bind(&auth.user.id)
        .bind(&project_id)
        .fetch_optional(&state.db)
        .await?;

        let Some(role_override) = membership else {
            return Err(AppError::forbidden("project access denied"));
        };

        let role = match role_override {
            Some(r) => r
                .parse()
                .map_err(|e: String| AppError::internal(format!("corrupt role_override: {e}")))?,
            None => auth.user.role,
        };

        let method = req.method();
        if role == Role::Viewer && method != Method::GET && method != Method::HEAD {
            return Err(AppError::forbidden("viewer role is read-only"));
        }
        role
    };

    let project = project_row
        .as_ref()
        .map(row_to_project)
        .transpose()?
        .ok_or_else(|| AppError::not_found("project not found"))?;

    req.extensions_mut().insert(ProjectContext {
        user: auth,
        project,
        effective_role,
    });

    Ok(next.run(req).await)
}

fn extract_token_from_headers_and_query(
    headers: &axum::http::HeaderMap,
    query: Option<&str>,
) -> Option<String> {
    if let Some(val) = headers.get(AUTHORIZATION)
        && let Ok(s) = val.to_str()
        && let Some(token) = s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer "))
    {
        let trimmed = token.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Some(q) = query {
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=')
                && k == "token"
                && !v.trim().is_empty()
            {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

fn extract_project_id_from_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/api/projects/")?;
    let id = rest.split('/').next()?.trim();
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

pub(crate) fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() {
        "project".to_string()
    } else {
        trimmed.to_string()
    }
}

fn normalize_workspace_path(path: &std::path::Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let s = canonical.to_string_lossy().to_string();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

async fn list_projects(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<ProjectDto>>, AppError> {
    let projects = load_accessible_projects(&state, &auth.user).await?;
    Ok(Json(projects))
}

async fn create_project(
    State(state): State<AppState>,
    admin: AdminUser,
    Json(req): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<ProjectDto>), AppError> {
    let name = req.name.trim();
    let raw_path = req.workspace_path.trim();
    if name.is_empty() || raw_path.is_empty() {
        return Err(AppError::bad_request(
            "name and workspace_path are required",
        ));
    }

    let path = std::path::Path::new(raw_path);
    if !path.exists() || !path.is_dir() {
        return Err(AppError::bad_request(
            "workspace_path must point to an existing directory on the host",
        ));
    }

    let normalized_path = normalize_workspace_path(path);
    let base_slug = slugify(name);
    let existing_slug: Option<String> =
        sqlx::query_scalar("SELECT id FROM projects WHERE slug = ?")
            .bind(&base_slug)
            .fetch_optional(&state.db)
            .await?;
    let slug = if existing_slug.is_some() {
        format!("{}-{}", base_slug, &Uuid::new_v4().to_string()[..8])
    } else {
        base_slug
    };

    let project_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO projects (id, name, slug, workspace_path, created_by, created_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&project_id)
    .bind(name)
    .bind(&slug)
    .bind(&normalized_path)
    .bind(&admin.0.user.id)
    .bind(&now)
    .execute(&state.db)
    .await?;

    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "create_project",
        Some("project"),
        Some(&project_id),
        None,
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(ProjectDto {
            id: project_id,
            name: name.to_string(),
            slug,
            workspace_path: normalized_path,
            created_at: now,
        }),
    ))
}

async fn get_project(Extension(ctx): Extension<ProjectContext>) -> Json<ProjectDto> {
    Json(ctx.project)
}

async fn update_project(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Json(req): Json<UpdateProjectRequest>,
) -> Result<Json<ProjectDto>, AppError> {
    let mut updated = ctx.project;
    if let Some(name) = req.name {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(AppError::bad_request("project name cannot be empty"));
        }
        sqlx::query("UPDATE projects SET name = ? WHERE id = ?")
            .bind(trimmed)
            .bind(&updated.id)
            .execute(&state.db)
            .await?;
        updated.name = trimmed.to_string();
    }
    Ok(Json(updated))
}

async fn delete_project(
    State(state): State<AppState>,
    admin: AdminUser,
    Extension(ctx): Extension<ProjectContext>,
) -> Result<StatusCode, AppError> {
    sqlx::query("DELETE FROM projects WHERE id = ?")
        .bind(&ctx.project.id)
        .execute(&state.db)
        .await?;
    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "delete_project",
        Some("project"),
        Some(&ctx.project.id),
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_project_members(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<ProjectMemberDto>>, AppError> {
    let exists: Option<String> = sqlx::query_scalar("SELECT id FROM projects WHERE id = ?")
        .bind(&project_id)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(AppError::not_found("project not found"));
    }

    let rows = sqlx::query(
        r#"
        SELECT u.id AS user_id, u.username, u.email, u.role, m.created_at AS assigned_at
        FROM project_memberships m
        INNER JOIN users u ON u.id = m.user_id
        WHERE m.project_id = ?
        ORDER BY m.created_at ASC
        "#,
    )
    .bind(&project_id)
    .fetch_all(&state.db)
    .await?;

    let mut members = Vec::with_capacity(rows.len());
    for row in rows {
        let role_str: String = row.try_get("role")?;
        let role: Role = role_str
            .parse()
            .map_err(|e: String| AppError::internal(format!("corrupt role in db: {e}")))?;
        members.push(ProjectMemberDto {
            user_id: row.try_get("user_id")?,
            username: row.try_get("username")?,
            email: row.try_get("email")?,
            role,
            assigned_at: row.try_get("assigned_at")?,
        });
    }
    Ok(Json(members))
}

async fn assign_project_member(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(project_id): Path<String>,
    Json(req): Json<AssignProjectMemberRequest>,
) -> Result<StatusCode, AppError> {
    let project_exists: Option<String> =
        sqlx::query_scalar("SELECT id FROM projects WHERE id = ?")
            .bind(&project_id)
            .fetch_optional(&state.db)
            .await?;
    if project_exists.is_none() {
        return Err(AppError::not_found("project not found"));
    }
    let user_exists: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE id = ?")
        .bind(&req.user_id)
        .fetch_optional(&state.db)
        .await?;
    if user_exists.is_none() {
        return Err(AppError::not_found("user not found"));
    }

    sqlx::query(
        "INSERT OR IGNORE INTO project_memberships (user_id, project_id, created_at) VALUES (?, ?, ?)",
    )
    .bind(&req.user_id)
    .bind(&project_id)
    .bind(Utc::now().to_rfc3339())
    .execute(&state.db)
    .await?;

    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "assign_project_member",
        Some("project"),
        Some(&project_id),
        Some(&req.user_id),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_project_member(
    State(state): State<AppState>,
    admin: AdminUser,
    Path((project_id, user_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    sqlx::query("DELETE FROM project_memberships WHERE project_id = ? AND user_id = ?")
        .bind(&project_id)
        .bind(&user_id)
        .execute(&state.db)
        .await?;
    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "remove_project_member",
        Some("project"),
        Some(&project_id),
        Some(&user_id),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn load_invite_project_ids(
    state: &AppState,
    invite_id: &str,
) -> Result<Vec<String>, AppError> {
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT project_id FROM invite_projects WHERE invite_id = ?")
            .bind(invite_id)
            .fetch_all(&state.db)
            .await?;
    Ok(ids)
}

async fn create_invite(
    State(state): State<AppState>,
    admin: AdminUser,
    Json(req): Json<CreateInviteRequest>,
) -> Result<(StatusCode, Json<InviteDto>), AppError> {
    let mut tx = state.db.begin().await?;

    for pid in &req.project_ids {
        let exists: Option<String> = sqlx::query_scalar("SELECT id FROM projects WHERE id = ?")
            .bind(pid)
            .fetch_optional(&mut *tx)
            .await?;
        if exists.is_none() {
            return Err(AppError::bad_request(format!("unknown project_id: {pid}")));
        }
    }

    let invite_id = Uuid::new_v4().to_string();
    let code = format!("inv_{}", Uuid::new_v4().simple());
    let now = Utc::now();
    let now_str = now.to_rfc3339();
    let expires_at = req
        .expires_in_hours
        .map(|hrs| (now + Duration::hours(hrs)).to_rfc3339());

    sqlx::query(
        "INSERT INTO user_invites (id, code, role, created_by, expires_at, created_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&invite_id)
    .bind(&code)
    .bind(req.role.as_str())
    .bind(&admin.0.user.id)
    .bind(&expires_at)
    .bind(&now_str)
    .execute(&mut *tx)
    .await?;

    for pid in &req.project_ids {
        sqlx::query("INSERT OR IGNORE INTO invite_projects (invite_id, project_id) VALUES (?, ?)")
            .bind(&invite_id)
            .bind(pid)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;

    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "create_invite",
        Some("invite"),
        Some(&invite_id),
        Some(req.role.as_str()),
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(InviteDto {
            id: invite_id,
            code,
            role: req.role,
            project_ids: req.project_ids,
            created_by: admin.0.user.id,
            used_by: None,
            used_at: None,
            expires_at,
            created_at: now_str,
        }),
    ))
}

async fn list_invites(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<Vec<InviteDto>>, AppError> {
    let rows = sqlx::query(
        "SELECT id, code, role, created_by, used_by, used_at, expires_at, created_at FROM user_invites ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await?;

    let mut invites = Vec::with_capacity(rows.len());
    for row in rows {
        let id: String = row.try_get("id")?;
        let role_str: String = row.try_get("role")?;
        let role: Role = role_str
            .parse()
            .map_err(|e: String| AppError::internal(format!("corrupt role in invite: {e}")))?;
        let project_ids = load_invite_project_ids(&state, &id).await?;
        invites.push(InviteDto {
            id,
            code: row.try_get("code")?,
            role,
            project_ids,
            created_by: row.try_get("created_by")?,
            used_by: row.try_get("used_by")?,
            used_at: row.try_get("used_at")?,
            expires_at: row.try_get("expires_at")?,
            created_at: row.try_get("created_at")?,
        });
    }

    Ok(Json(invites))
}

async fn redeem_invite(
    State(state): State<AppState>,
    Json(req): Json<RedeemInviteRequest>,
) -> Result<(StatusCode, Json<AuthTokensResponse>), AppError> {
    let code = req.code.trim();
    let username = req.username.trim();
    let email = req.email.trim();
    if code.is_empty() || username.is_empty() || email.is_empty() {
        return Err(AppError::bad_request(
            "code, username, and email are required",
        ));
    }
    if req.password.len() < 8 {
        return Err(AppError::bad_request(
            "password must be at least 8 characters",
        ));
    }

    let mut tx = state.db.begin().await?;
    let row = sqlx::query(
        "SELECT id, role, used_by, expires_at FROM user_invites WHERE code = ?",
    )
    .bind(code)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::not_found("invite code not found"))?;

    let used_by: Option<String> = row.try_get("used_by")?;
    if used_by.is_some() {
        return Err(AppError::conflict("invite code has already been redeemed"));
    }

    let expires_at: Option<String> = row.try_get("expires_at")?;
    if let Some(exp_str) = expires_at
        && let Ok(exp) = chrono::DateTime::parse_from_rfc3339(&exp_str)
        && exp.with_timezone(&Utc) < Utc::now()
    {
        return Err(AppError::bad_request("invite code has expired"));
    }

    let invite_id: String = row.try_get("id")?;
    let role_str: String = row.try_get("role")?;
    let role: Role = role_str
        .parse()
        .map_err(|e: String| AppError::internal(format!("corrupt role in invite: {e}")))?;

    let user_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let password_hash = hash_password(&req.password)?;

    sqlx::query(
        "INSERT INTO users (id, username, email, password_hash, role, created_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&user_id)
    .bind(username)
    .bind(email)
    .bind(&password_hash)
    .bind(role.as_str())
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    let updated = sqlx::query(
        "UPDATE user_invites SET used_by = ?, used_at = ? WHERE id = ? AND used_by IS NULL",
    )
    .bind(&user_id)
    .bind(&now)
    .bind(&invite_id)
    .execute(&mut *tx)
    .await?;

    if updated.rows_affected() == 0 {
        return Err(AppError::conflict("invite code has already been redeemed"));
    }

    let project_ids: Vec<String> =
        sqlx::query_scalar("SELECT project_id FROM invite_projects WHERE invite_id = ?")
            .bind(&invite_id)
            .fetch_all(&mut *tx)
            .await?;

    for pid in project_ids {
        sqlx::query(
            "INSERT OR IGNORE INTO project_memberships (user_id, project_id, created_at) VALUES (?, ?, ?)",
        )
        .bind(&user_id)
        .bind(&pid)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    let user = UserDto {
        id: user_id.clone(),
        username: username.to_string(),
        email: email.to_string(),
        role,
        created_at: now,
    };
    record_audit_log(
        &state,
        Some(&user_id),
        "redeem_invite",
        Some("invite"),
        Some(&invite_id),
        None,
    )
    .await?;
    let tokens = issue_session_tokens(&state, &user).await?;
    Ok((StatusCode::CREATED, Json(tokens)))
}
