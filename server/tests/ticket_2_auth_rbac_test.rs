use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use claude_web_ui_server::fakes::{FakeCliRunner, FakeMessengerTransport};
use claude_web_ui_server::{fs_link, GatewayConfig, GatewayServer};
use claude_web_ui_shared::auth::{
    AuthStatusResponse, AuthTokensResponse, InviteDto, MeResponse, ProjectDto, ProjectMemberDto,
    RevokeSessionsResponse, Role,
};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

struct TestHarness {
    _tmp: TempDir,
    server: GatewayServer,
}

impl TestHarness {
    async fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let mut config = GatewayConfig::for_data_dir(tmp.path().join("data"));
        config.jwt_secret = "test-jwt-secret-key-at-least-32-bytes-long".to_string();
        let server = GatewayServer::new(
            config,
            Arc::new(FakeCliRunner::new()),
            Arc::new(FakeMessengerTransport::new()),
        )
        .await
        .expect("gateway server init");
        Self { _tmp: tmp, server }
    }

    async fn request_json(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(tok) = token {
            builder = builder.header("authorization", format!("Bearer {tok}"));
        }
        let req_body = match body {
            Some(val) => {
                builder = builder.header("content-type", "application/json");
                Body::from(val.to_string())
            }
            None => Body::empty(),
        };
        let req = builder.body(req_body).expect("request");
        let resp = self
            .server
            .router()
            .oneshot(req)
            .await
            .expect("router response");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        let val = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                Value::String(String::from_utf8_lossy(&bytes).into_owned())
            })
        };
        (status, val)
    }

    async fn decode<T: DeserializeOwned>(val: Value) -> T {
        serde_json::from_value(val).expect("decode json response")
    }

    async fn bootstrap_admin(&self) -> AuthTokensResponse {
        let (status, body) = self
            .request_json(
                Method::POST,
                "/api/auth/bootstrap",
                None,
                Some(json!({
                    "username": "admin",
                    "email": "admin@example.com",
                    "password": "correct-horse-battery"
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        Self::decode(body).await
    }
}

#[tokio::test]
async fn bootstrap_admin_initializes_fresh_server_and_blocks_second_bootstrap() {
    let h = TestHarness::new().await;

    let (status, body) = h
        .request_json(Method::GET, "/api/auth/status", None, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let auth_status: AuthStatusResponse = TestHarness::decode(body).await;
    assert!(!auth_status.initialized);

    let (status, body) = h
        .request_json(
            Method::POST,
            "/api/auth/bootstrap",
            None,
            Some(json!({
                "username": "admin",
                "email": "admin@example.com",
                "password": "correct-horse-battery"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let tokens: AuthTokensResponse = TestHarness::decode(body).await;
    assert_eq!(tokens.user.username, "admin");
    assert_eq!(tokens.user.role, Role::Admin);
    assert!(!tokens.access_token.is_empty());
    assert!(!tokens.refresh_token.is_empty());

    let (status, body) = h
        .request_json(Method::GET, "/api/auth/status", None, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let auth_status: AuthStatusResponse = TestHarness::decode(body).await;
    assert!(auth_status.initialized);

    let (status, _) = h
        .request_json(
            Method::POST,
            "/api/auth/bootstrap",
            None,
            Some(json!({
                "username": "second-admin",
                "email": "second@example.com",
                "password": "another-password"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn login_supports_username_and_email_refresh_rotates_tokens_and_admin_revokes_sessions() {
    let h = TestHarness::new().await;
    let boot = h.bootstrap_admin().await;

    let (status, _) = h
        .request_json(
            Method::POST,
            "/api/auth/login",
            None,
            Some(json!({
                "identifier": "admin",
                "password": "wrong-password"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = h
        .request_json(
            Method::POST,
            "/api/auth/login",
            None,
            Some(json!({
                "identifier": "admin",
                "password": "correct-horse-battery"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let login_by_username: AuthTokensResponse = TestHarness::decode(body).await;
    assert_eq!(login_by_username.user.id, boot.user.id);

    let (status, body) = h
        .request_json(
            Method::POST,
            "/api/auth/login",
            None,
            Some(json!({
                "identifier": "admin@example.com",
                "password": "correct-horse-battery"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let login_by_email: AuthTokensResponse = TestHarness::decode(body).await;

    let (status, body) = h
        .request_json(
            Method::GET,
            "/api/auth/me",
            Some(&login_by_email.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let me: MeResponse = TestHarness::decode(body).await;
    assert_eq!(me.user.username, "admin");

    let (status, body) = h
        .request_json(
            Method::POST,
            "/api/auth/refresh",
            None,
            Some(json!({ "refresh_token": login_by_email.refresh_token })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let refreshed: AuthTokensResponse = TestHarness::decode(body).await;

    // Reusing rotated refresh token must fail
    let (status, _) = h
        .request_json(
            Method::POST,
            "/api/auth/refresh",
            None,
            Some(json!({ "refresh_token": login_by_email.refresh_token })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Admin revokes all sessions for this user
    let revoke_path = format!("/api/admin/users/{}/revoke-sessions", boot.user.id);
    let (status, body) = h
        .request_json(
            Method::POST,
            &revoke_path,
            Some(&refreshed.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let revoked: RevokeSessionsResponse = TestHarness::decode(body).await;
    assert!(revoked.revoked_count >= 1);

    // Revoked access token is immediately rejected
    let (status, _) = h
        .request_json(
            Method::GET,
            "/api/auth/me",
            Some(&refreshed.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Revoked refresh token is immediately rejected
    let (status, _) = h
        .request_json(
            Method::POST,
            "/api/auth/refresh",
            None,
            Some(json!({ "refresh_token": refreshed.refresh_token })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admin_registers_project_workspace_and_validates_host_directory() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;

    let missing_dir = h._tmp.path().join("does-not-exist");
    let (status, _) = h
        .request_json(
            Method::POST,
            "/api/projects",
            Some(&admin.access_token),
            Some(json!({
                "name": "Missing",
                "workspace_path": missing_dir.to_string_lossy()
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let workspace_a = h._tmp.path().join("repo-alpha");
    std::fs::create_dir_all(&workspace_a).expect("create repo-alpha");

    let (status, body) = h
        .request_json(
            Method::POST,
            "/api/projects",
            Some(&admin.access_token),
            Some(json!({
                "name": "Repo Alpha",
                "workspace_path": workspace_a.to_string_lossy()
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let project: ProjectDto = TestHarness::decode(body).await;
    assert_eq!(project.name, "Repo Alpha");
    assert_eq!(project.slug, "repo-alpha");

    let (status, body) = h
        .request_json(
            Method::GET,
            "/api/projects",
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let projects: Vec<ProjectDto> = TestHarness::decode(body).await;
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].id, project.id);

    let (status, body) = h
        .request_json(
            Method::PATCH,
            &format!("/api/projects/{}", project.id),
            Some(&admin.access_token),
            Some(json!({ "name": "Repo Alpha Renamed" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let updated: ProjectDto = TestHarness::decode(body).await;
    assert_eq!(updated.name, "Repo Alpha Renamed");
}

#[tokio::test]
async fn single_use_invite_creates_user_with_role_and_allowed_projects() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;

    let ws_a = h._tmp.path().join("alpha");
    let ws_b = h._tmp.path().join("beta");
    std::fs::create_dir_all(&ws_a).unwrap();
    std::fs::create_dir_all(&ws_b).unwrap();

    let (_, body_a) = h
        .request_json(
            Method::POST,
            "/api/projects",
            Some(&admin.access_token),
            Some(json!({ "name": "Alpha", "workspace_path": ws_a.to_string_lossy() })),
        )
        .await;
    let project_a: ProjectDto = TestHarness::decode(body_a).await;

    let (_, body_b) = h
        .request_json(
            Method::POST,
            "/api/projects",
            Some(&admin.access_token),
            Some(json!({ "name": "Beta", "workspace_path": ws_b.to_string_lossy() })),
        )
        .await;
    let _project_b: ProjectDto = TestHarness::decode(body_b).await;

    let (status, body) = h
        .request_json(
            Method::POST,
            "/api/admin/invites",
            Some(&admin.access_token),
            Some(json!({
                "role": "developer",
                "project_ids": [project_a.id]
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let invite: InviteDto = TestHarness::decode(body).await;
    assert_eq!(invite.role, Role::Developer);
    assert_eq!(invite.project_ids, vec![project_a.id.clone()]);
    assert!(invite.used_by.is_none());

    let (status, body) = h
        .request_json(
            Method::POST,
            "/api/auth/invites/redeem",
            None,
            Some(json!({
                "code": invite.code,
                "username": "dev1",
                "email": "dev1@example.com",
                "password": "dev-password-1"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let dev_tokens: AuthTokensResponse = TestHarness::decode(body).await;
    assert_eq!(dev_tokens.user.role, Role::Developer);

    // Dev sees only Alpha
    let (status, body) = h
        .request_json(
            Method::GET,
            "/api/projects",
            Some(&dev_tokens.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let visible: Vec<ProjectDto> = TestHarness::decode(body).await;
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, project_a.id);

    // Reusing the same invite fails with 409 Conflict
    let (status, _) = h
        .request_json(
            Method::POST,
            "/api/auth/invites/redeem",
            None,
            Some(json!({
                "code": invite.code,
                "username": "dev2",
                "email": "dev2@example.com",
                "password": "dev-password-2"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Unknown invite code fails with 404 Not Found
    let (status, _) = h
        .request_json(
            Method::POST,
            "/api/auth/invites/redeem",
            None,
            Some(json!({
                "code": "inv_nonexistent",
                "username": "dev3",
                "email": "dev3@example.com",
                "password": "dev-password-3"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rbac_matrix_enforces_admin_developer_and_viewer_project_boundaries() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;

    let ws_a = h._tmp.path().join("rbac-alpha");
    let ws_b = h._tmp.path().join("rbac-beta");
    std::fs::create_dir_all(&ws_a).unwrap();
    std::fs::create_dir_all(&ws_b).unwrap();

    let (_, body_a) = h
        .request_json(
            Method::POST,
            "/api/projects",
            Some(&admin.access_token),
            Some(json!({ "name": "Alpha", "workspace_path": ws_a.to_string_lossy() })),
        )
        .await;
    let project_a: ProjectDto = TestHarness::decode(body_a).await;

    let (_, body_b) = h
        .request_json(
            Method::POST,
            "/api/projects",
            Some(&admin.access_token),
            Some(json!({ "name": "Beta", "workspace_path": ws_b.to_string_lossy() })),
        )
        .await;
    let project_b: ProjectDto = TestHarness::decode(body_b).await;

    // Create Developer and Viewer invites scoped only to Alpha
    let (_, dev_inv_body) = h
        .request_json(
            Method::POST,
            "/api/admin/invites",
            Some(&admin.access_token),
            Some(json!({ "role": "developer", "project_ids": [project_a.id] })),
        )
        .await;
    let dev_invite: InviteDto = TestHarness::decode(dev_inv_body).await;

    let (_, viewer_inv_body) = h
        .request_json(
            Method::POST,
            "/api/admin/invites",
            Some(&admin.access_token),
            Some(json!({ "role": "viewer", "project_ids": [project_a.id] })),
        )
        .await;
    let viewer_invite: InviteDto = TestHarness::decode(viewer_inv_body).await;

    let (_, dev_auth_body) = h
        .request_json(
            Method::POST,
            "/api/auth/invites/redeem",
            None,
            Some(json!({
                "code": dev_invite.code,
                "username": "dev",
                "email": "dev@example.com",
                "password": "password-123"
            })),
        )
        .await;
    let dev: AuthTokensResponse = TestHarness::decode(dev_auth_body).await;

    let (_, viewer_auth_body) = h
        .request_json(
            Method::POST,
            "/api/auth/invites/redeem",
            None,
            Some(json!({
                "code": viewer_invite.code,
                "username": "viewer",
                "email": "viewer@example.com",
                "password": "password-123"
            })),
        )
        .await;
    let viewer: AuthTokensResponse = TestHarness::decode(viewer_auth_body).await;

    // Unauthenticated rejected
    let (status, _) = h
        .request_json(Method::GET, "/api/projects", None, None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Admin has full access
    let (status, _) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}", project_b.id),
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Developer: read/write assigned Alpha, forbidden on unassigned Beta, forbidden on admin routes
    let (status, _) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}", project_a.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = h
        .request_json(
            Method::PATCH,
            &format!("/api/projects/{}", project_a.id),
            Some(&dev.access_token),
            Some(json!({ "name": "Alpha Updated by Dev" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}", project_b.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = h
        .request_json(
            Method::DELETE,
            &format!("/api/projects/{}", project_a.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = h
        .request_json(Method::GET, "/api/admin/users", Some(&dev.access_token), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Viewer: read-only on assigned Alpha, 403 on mutations and unassigned Beta
    let (status, _) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}", project_a.id),
            Some(&viewer.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = h
        .request_json(
            Method::PATCH,
            &format!("/api/projects/{}", project_a.id),
            Some(&viewer.access_token),
            Some(json!({ "name": "Should Fail" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}", project_b.id),
            Some(&viewer.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Admin assigns Developer to Beta, verifies access, then revokes
    let (status, _) = h
        .request_json(
            Method::POST,
            &format!("/api/admin/projects/{}/members", project_b.id),
            Some(&admin.access_token),
            Some(json!({ "user_id": dev.user.id })),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/admin/projects/{}/members", project_b.id),
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let members: Vec<ProjectMemberDto> = TestHarness::decode(body).await;
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].user_id, dev.user.id);

    let (status, _) = h
        .request_json(
            Method::PATCH,
            &format!("/api/projects/{}", project_b.id),
            Some(&dev.access_token),
            Some(json!({ "name": "Beta Updated by Dev" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = h
        .request_json(
            Method::DELETE,
            &format!("/api/admin/projects/{}/members/{}", project_b.id, dev.user.id),
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}", project_b.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn embedded_ui_assets_and_fs_link_work() {
    let h = TestHarness::new().await;

    let resp = h
        .server
        .router()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.contains("text/html"), "unexpected content-type: {ct}");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&body).contains("Claude Web UI"));

    let resp_js = h
        .server
        .router()
        .oneshot(Request::builder().uri("/app.js").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp_js.status(), StatusCode::OK);

    let (api_404, _) = h
        .request_json(Method::GET, "/api/unknown-endpoint", None, None)
        .await;
    assert_eq!(api_404, StatusCode::NOT_FOUND);

    let src = h._tmp.path().join("shared_skills");
    let link = h._tmp.path().join("account_a").join("skills");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("SKILL.md"), "# Test Skill").unwrap();
    fs_link::link_dir(&src, &link).expect("link_dir should succeed");
    let content = std::fs::read_to_string(link.join("SKILL.md")).unwrap();
    assert_eq!(content, "# Test Skill");
}
