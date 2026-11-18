use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use claude_web_ui_server::fakes::{FakeCliRunner, FakeMessengerTransport};
use claude_web_ui_server::{GatewayConfig, GatewayServer};
use claude_web_ui_shared::accounts::{
    AccountAclDto, CliAccountDto, CliProvider, CredentialFileDto, ProjectCliStateDto,
};
use claude_web_ui_shared::auth::{AuthTokensResponse, InviteDto, ProjectDto};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tower::ServiceExt;

struct TestHarness {
    _tmp: TempDir,
    data_dir: PathBuf,
    cli_runner: Arc<FakeCliRunner>,
    server: GatewayServer,
}

impl TestHarness {
    async fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let data_dir = tmp.path().join("data");
        let mut config = GatewayConfig::for_data_dir(&data_dir);
        config.jwt_secret = "test-jwt-secret-key-at-least-32-bytes-long".to_string();
        let cli_runner = Arc::new(FakeCliRunner::new());
        let server = GatewayServer::new(
            config,
            cli_runner.clone(),
            Arc::new(FakeMessengerTransport::new()),
        )
        .await
        .expect("gateway server init");
        Self {
            _tmp: tmp,
            data_dir,
            cli_runner,
            server,
        }
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

    async fn create_project(&self, admin_token: &str, name: &str) -> ProjectDto {
        let ws = self._tmp.path().join(format!("ws-{}", name.to_lowercase().replace(' ', "-")));
        std::fs::create_dir_all(&ws).unwrap();
        let (status, body) = self
            .request_json(
                Method::POST,
                "/api/projects",
                Some(admin_token),
                Some(json!({
                    "name": name,
                    "workspace_path": ws.to_string_lossy()
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        Self::decode(body).await
    }

    async fn invite_and_redeem(
        &self,
        admin_token: &str,
        role: &str,
        username: &str,
        project_ids: Vec<String>,
    ) -> AuthTokensResponse {
        let (status, inv_body) = self
            .request_json(
                Method::POST,
                "/api/admin/invites",
                Some(admin_token),
                Some(json!({
                    "role": role,
                    "project_ids": project_ids
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        let invite: InviteDto = Self::decode(inv_body).await;

        let (status, auth_body) = self
            .request_json(
                Method::POST,
                "/api/auth/invites/redeem",
                None,
                Some(json!({
                    "code": invite.code,
                    "username": username,
                    "email": format!("{username}@example.com"),
                    "password": "password-123"
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        Self::decode(auth_body).await
    }

    async fn spawn_tcp(&self) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let router = self.server.router();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        (addr, handle)
    }
}

#[tokio::test]
async fn admin_account_crud_isolates_profiles_links_shared_dirs_and_deletes_safely() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;
    let project = h.create_project(&admin.access_token, "Alpha").await;
    let dev = h
        .invite_and_redeem(&admin.access_token, "developer", "dev1", vec![project.id.clone()])
        .await;
    let viewer = h
        .invite_and_redeem(&admin.access_token, "viewer", "viewer1", vec![project.id.clone()])
        .await;

    // Non-admins cannot create or list admin account profiles
    let (status, _) = h
        .request_json(
            Method::POST,
            "/api/admin/accounts",
            Some(&dev.access_token),
            Some(json!({ "provider": "claude_code", "name": "Blocked" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = h
        .request_json(
            Method::GET,
            "/api/admin/accounts",
            Some(&viewer.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Admin creates two Claude Code accounts, one Codex, and one Antigravity
    let (status, body) = h
        .request_json(
            Method::POST,
            "/api/admin/accounts",
            Some(&admin.access_token),
            Some(json!({ "provider": "claude_code", "name": "Work Claude" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let claude_a: CliAccountDto = TestHarness::decode(body).await;
    assert_eq!(claude_a.provider, CliProvider::ClaudeCode);
    assert_eq!(claude_a.name, "Work Claude");
    assert_eq!(claude_a.slug, "work-claude");

    let (status, body) = h
        .request_json(
            Method::POST,
            "/api/admin/accounts",
            Some(&admin.access_token),
            Some(json!({ "provider": "claude_code", "name": "Personal Claude" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let claude_b: CliAccountDto = TestHarness::decode(body).await;

    let (status, body) = h
        .request_json(
            Method::POST,
            "/api/admin/accounts",
            Some(&admin.access_token),
            Some(json!({ "provider": "codex", "name": "Team Codex" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let codex_a: CliAccountDto = TestHarness::decode(body).await;

    let (status, body) = h
        .request_json(
            Method::POST,
            "/api/admin/accounts",
            Some(&admin.access_token),
            Some(json!({ "provider": "antigravity", "name": "Main Agy" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let agy_a: CliAccountDto = TestHarness::decode(body).await;

    // Duplicate provider+slug returns 409 Conflict
    let (status, _) = h
        .request_json(
            Method::POST,
            "/api/admin/accounts",
            Some(&admin.access_token),
            Some(json!({ "provider": "claude_code", "name": "Work Claude" })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Verify isolated profile dirs and linked shared subdirectories
    let claude_a_dir = PathBuf::from(&claude_a.profile_dir);
    let claude_b_dir = PathBuf::from(&claude_b.profile_dir);
    assert_eq!(
        claude_a_dir,
        h.data_dir.join("accounts").join("claude_code").join("work-claude")
    );
    assert!(claude_a_dir.is_dir());
    assert!(PathBuf::from(&codex_a.profile_dir).is_dir());
    assert!(PathBuf::from(&agy_a.profile_dir).is_dir());

    for sub in ["projects", "sessions", "skills", "agents"] {
        assert!(
            h.data_dir.join("shared").join("claude_code").join(sub).is_dir(),
            "shared/claude_code/{sub} missing"
        );
        assert!(
            claude_a_dir.join(sub).is_dir(),
            "claude_a/{sub} link missing"
        );
        assert!(
            claude_b_dir.join(sub).is_dir(),
            "claude_b/{sub} link missing"
        );
    }

    // Writing a session transcript via Account A's linked sessions dir is visible in Account B and shared dir,
    // while isolated from Codex's sessions dir.
    let shared_session_rel = PathBuf::from("sessions").join("session-123.jsonl");
    std::fs::write(
        claude_a_dir.join(&shared_session_rel),
        "{\"type\":\"message\",\"text\":\"hello\"}\n",
    )
    .expect("write shared session");
    let read_from_b = std::fs::read_to_string(claude_b_dir.join(&shared_session_rel))
        .expect("read shared session from account B");
    assert!(read_from_b.contains("hello"));
    assert!(!PathBuf::from(&codex_a.profile_dir)
        .join(&shared_session_rel)
        .exists());

    // List all accounts and filter by provider
    let (status, body) = h
        .request_json(
            Method::GET,
            "/api/admin/accounts",
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let all: Vec<CliAccountDto> = TestHarness::decode(body).await;
    assert_eq!(all.len(), 4);

    let (status, body) = h
        .request_json(
            Method::GET,
            "/api/admin/accounts?provider=claude_code",
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let claude_only: Vec<CliAccountDto> = TestHarness::decode(body).await;
    assert_eq!(claude_only.len(), 2);

    // Rename Account B
    let (status, body) = h
        .request_json(
            Method::PATCH,
            &format!("/api/admin/accounts/{}", claude_b.id),
            Some(&admin.access_token),
            Some(json!({ "name": "Backup Claude" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let renamed: CliAccountDto = TestHarness::decode(body).await;
    assert_eq!(renamed.name, "Backup Claude");
    assert_eq!(renamed.profile_dir, claude_b.profile_dir);

    // Delete Account A -> unlinks junctions safely without deleting shared session files
    let (status, _) = h
        .request_json(
            Method::DELETE,
            &format!("/api/admin/accounts/{}", claude_a.id),
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!claude_a_dir.exists());
    assert!(
        h.data_dir
            .join("shared")
            .join("claude_code")
            .join(&shared_session_rel)
            .exists(),
        "shared session must survive account deletion"
    );
    assert!(claude_b_dir.join(&shared_session_rel).exists());
}

#[tokio::test]
async fn credential_upload_writes_isolated_files_validates_paths_and_enforces_rbac() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;
    let project = h.create_project(&admin.access_token, "Alpha").await;
    let dev = h
        .invite_and_redeem(&admin.access_token, "developer", "dev1", vec![project.id.clone()])
        .await;
    let viewer = h
        .invite_and_redeem(&admin.access_token, "viewer", "viewer1", vec![project.id.clone()])
        .await;

    let (_, body) = h
        .request_json(
            Method::POST,
            "/api/admin/accounts",
            Some(&admin.access_token),
            Some(json!({ "provider": "claude_code", "name": "Claude A" })),
        )
        .await;
    let claude_a: CliAccountDto = TestHarness::decode(body).await;

    let (_, body) = h
        .request_json(
            Method::POST,
            "/api/admin/accounts",
            Some(&admin.access_token),
            Some(json!({ "provider": "claude_code", "name": "Claude B" })),
        )
        .await;
    let claude_b: CliAccountDto = TestHarness::decode(body).await;

    let (_, body) = h
        .request_json(
            Method::POST,
            "/api/admin/accounts",
            Some(&admin.access_token),
            Some(json!({ "provider": "codex", "name": "Codex A" })),
        )
        .await;
    let codex_a: CliAccountDto = TestHarness::decode(body).await;

    // Non-admin blocked from uploading or listing credentials
    let (status, _) = h
        .request_json(
            Method::POST,
            &format!("/api/admin/accounts/{}/credentials", claude_a.id),
            Some(&dev.access_token),
            Some(json!({ "filename": ".credentials.json", "content": "{}" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = h
        .request_json(
            Method::GET,
            &format!("/api/admin/accounts/{}/credentials", claude_a.id),
            Some(&viewer.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Path traversal / unsafe filenames rejected with 400 Bad Request
    for bad_name in [
        "",
        ".",
        "..",
        "../escape.json",
        "..\\escape.json",
        "nested/auth.json",
        "sessions",
        "skills",
        "projects",
        "agents",
        "bad:file.json",
    ] {
        let (status, _) = h
            .request_json(
                Method::POST,
                &format!("/api/admin/accounts/{}/credentials", claude_a.id),
                Some(&admin.access_token),
                Some(json!({ "filename": bad_name, "content": "{}" })),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "expected 400 for unsafe filename {bad_name:?}"
        );
    }

    // Admin uploads .credentials.json & .claude.json to Claude A and auth.json to Codex A
    let cred_payload = r#"{"claudeAiOauth":{"accessToken":"secret-a"}}"#;
    let (status, body) = h
        .request_json(
            Method::POST,
            &format!("/api/admin/accounts/{}/credentials", claude_a.id),
            Some(&admin.access_token),
            Some(json!({ "filename": ".credentials.json", "content": cred_payload })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let uploaded: CredentialFileDto = TestHarness::decode(body).await;
    assert_eq!(uploaded.filename, ".credentials.json");
    assert_eq!(uploaded.size_bytes, cred_payload.len() as u64);

    let (status, _) = h
        .request_json(
            Method::POST,
            &format!("/api/admin/accounts/{}/credentials", claude_a.id),
            Some(&admin.access_token),
            Some(json!({ "filename": ".claude.json", "content": "{\"hasCompletedOnboarding\":true}" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = h
        .request_json(
            Method::POST,
            &format!("/api/admin/accounts/{}/credentials", codex_a.id),
            Some(&admin.access_token),
            Some(json!({ "filename": "auth.json", "content": "{\"OPENAI_API_KEY\":\"sk-test\"}" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Credentials are isolated to Claude A and never leak into Claude B
    let on_disk_a = std::fs::read_to_string(PathBuf::from(&claude_a.profile_dir).join(".credentials.json"))
        .expect("read credentials on disk");
    assert_eq!(on_disk_a, cred_payload);
    assert!(!PathBuf::from(&claude_b.profile_dir).join(".credentials.json").exists());

    // Admin lists credentials
    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/admin/accounts/{}/credentials", claude_a.id),
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let files: Vec<CredentialFileDto> = TestHarness::decode(body).await;
    let names: Vec<String> = files.into_iter().map(|f| f.filename).collect();
    assert_eq!(names, vec![".claude.json", ".credentials.json"]);

    // Account DTO now reflects has_credentials = true
    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/admin/accounts/{}", claude_a.id),
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let refreshed: CliAccountDto = TestHarness::decode(body).await;
    assert!(refreshed.has_credentials);
    assert_eq!(refreshed.credential_files, vec![".claude.json", ".credentials.json"]);

    // Admin deletes .claude.json
    let (status, _) = h
        .request_json(
            Method::DELETE,
            &format!("/api/admin/accounts/{}/credentials/.claude.json", claude_a.id),
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!PathBuf::from(&claude_a.profile_dir).join(".claude.json").exists());
}

#[tokio::test]
async fn oauth_login_pty_websocket_spawns_isolated_env_per_provider_and_rejects_non_admins() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;
    let project = h.create_project(&admin.access_token, "Alpha").await;
    let dev = h
        .invite_and_redeem(&admin.access_token, "developer", "dev1", vec![project.id.clone()])
        .await;
    let viewer = h
        .invite_and_redeem(&admin.access_token, "viewer", "viewer1", vec![project.id.clone()])
        .await;

    let mut accounts = Vec::new();
    for (provider, name) in [
        ("claude_code", "Claude OAuth"),
        ("codex", "Codex OAuth"),
        ("antigravity", "Agy OAuth"),
    ] {
        let (_, body) = h
            .request_json(
                Method::POST,
                "/api/admin/accounts",
                Some(&admin.access_token),
                Some(json!({ "provider": provider, "name": name })),
            )
            .await;
        let acc: CliAccountDto = TestHarness::decode(body).await;
        accounts.push(acc);
    }

    let (addr, server_task) = h.spawn_tcp().await;

    // Developer and Viewer are forbidden from launching login PTYs
    for non_admin_tok in [&dev.access_token, &viewer.access_token] {
        let ws_url = format!(
            "ws://{addr}/api/admin/accounts/{}/login-pty?token={non_admin_tok}",
            accounts[0].id
        );
        let err = tokio_tungstenite::connect_async(&ws_url)
            .await
            .expect_err("non-admin ws upgrade must fail");
        let err_str = err.to_string();
        assert!(
            err_str.contains("403"),
            "expected 403 Forbidden for non-admin ws, got: {err_str}"
        );
    }

    // Script OAuth URL output for first PTY session
    h.cli_runner.push_pty_output(
        b"Visit https://claude.ai/oauth/authorize?client_id=test to sign in\r\nCode: ".to_vec(),
    );

    for (idx, acc) in accounts.iter().enumerate() {
        let ws_url = format!(
            "ws://{addr}/api/admin/accounts/{}/login-pty?token={}&cols=100&rows=30",
            acc.id, admin.access_token
        );
        let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .expect("admin ws connect");

        // Receive initial PTY output
        let first_msg = ws
            .next()
            .await
            .expect("ws frame")
            .expect("valid ws message");
        let text = first_msg.into_text().expect("text frame");
        let parsed: Value = serde_json::from_str(&text).expect("json frame");
        assert_eq!(parsed["type"], "output");
        if idx == 0 {
            assert!(
                parsed["data"]
                    .as_str()
                    .unwrap_or("")
                    .contains("https://claude.ai/oauth/authorize"),
                "expected scripted OAuth URL in first output, got: {parsed}"
            );
        }

        // Send resize and interactive code input
        ws.send(WsMessage::Text(
            json!({ "type": "resize", "cols": 120, "rows": 40 })
                .to_string()
                .into(),
        ))
        .await
        .expect("send resize");
        ws.send(WsMessage::Text(
            json!({ "type": "input", "data": format!("oauth-code-{idx}\r") })
                .to_string()
                .into(),
        ))
        .await
        .expect("send input");

        // Wait until exit frame arrives
        let mut saw_exit = false;
        while let Some(Ok(frame)) = ws.next().await {
            if let Ok(txt) = frame.into_text()
                && let Ok(v) = serde_json::from_str::<Value>(&txt)
                && v["type"] == "exit"
            {
                assert_eq!(v["code"], 0);
                saw_exit = true;
                break;
            }
        }
        assert!(saw_exit, "expected exit frame for account {}", acc.id);
    }

    let invocations = h.cli_runner.recorded_pty_invocations();
    assert_eq!(invocations.len(), 3);

    assert_eq!(invocations[0].provider, CliProvider::ClaudeCode);
    assert_eq!(invocations[0].program, "claude");
    assert_eq!(invocations[0].args, vec!["login".to_string()]);
    assert_eq!(invocations[0].cols, 100);
    assert_eq!(invocations[0].rows, 30);
    assert_eq!(
        invocations[0].env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
        Some(accounts[0].profile_dir.as_str())
    );

    assert_eq!(invocations[1].provider, CliProvider::Codex);
    assert_eq!(invocations[1].program, "codex");
    assert_eq!(invocations[1].args, vec!["login".to_string()]);
    assert_eq!(
        invocations[1].env.get("CODEX_HOME").map(String::as_str),
        Some(accounts[1].profile_dir.as_str())
    );

    assert_eq!(invocations[2].provider, CliProvider::Antigravity);
    assert_eq!(invocations[2].program, "agy");
    assert!(invocations[2].args.is_empty());
    assert_eq!(
        invocations[2]
            .env
            .get("ANTIGRAVITY_CONFIG_DIR")
            .map(String::as_str),
        Some(accounts[2].profile_dir.as_str())
    );

    let inputs = h.cli_runner.recorded_pty_inputs();
    assert!(inputs.iter().any(|b| b == b"oauth-code-0\r"));
    assert!(inputs.iter().any(|b| b == b"oauth-code-1\r"));
    assert!(inputs.iter().any(|b| b == b"oauth-code-2\r"));

    let resizes = h.cli_runner.recorded_pty_resizes();
    assert_eq!(resizes.len(), 3);
    assert_eq!(resizes[0], (120, 40));

    server_task.abort();
}

#[tokio::test]
async fn developer_account_acls_project_cli_switching_and_viewer_readonly() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;
    let alpha = h.create_project(&admin.access_token, "Alpha").await;
    let beta = h.create_project(&admin.access_token, "Beta").await;
    let dev = h
        .invite_and_redeem(&admin.access_token, "developer", "dev1", vec![alpha.id.clone()])
        .await;
    let viewer = h
        .invite_and_redeem(&admin.access_token, "viewer", "viewer1", vec![alpha.id.clone()])
        .await;

    let (_, body) = h
        .request_json(
            Method::POST,
            "/api/admin/accounts",
            Some(&admin.access_token),
            Some(json!({ "provider": "claude_code", "name": "Claude A" })),
        )
        .await;
    let claude_a: CliAccountDto = TestHarness::decode(body).await;

    let (_, body) = h
        .request_json(
            Method::POST,
            "/api/admin/accounts",
            Some(&admin.access_token),
            Some(json!({ "provider": "claude_code", "name": "Claude B" })),
        )
        .await;
    let claude_b: CliAccountDto = TestHarness::decode(body).await;

    let (_, body) = h
        .request_json(
            Method::POST,
            "/api/admin/accounts",
            Some(&admin.access_token),
            Some(json!({ "provider": "codex", "name": "Codex A" })),
        )
        .await;
    let codex_a: CliAccountDto = TestHarness::decode(body).await;

    // Non-admins cannot configure ACLs
    let (status, _) = h
        .request_json(
            Method::PUT,
            &format!(
                "/api/admin/projects/{}/members/{}/accounts",
                alpha.id, dev.user.id
            ),
            Some(&dev.access_token),
            Some(json!({ "allowed_accounts": [claude_a.id] })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Admin restricts Developer on Alpha to [Claude A, Codex A]
    let (status, body) = h
        .request_json(
            Method::PUT,
            &format!(
                "/api/admin/projects/{}/members/{}/accounts",
                alpha.id, dev.user.id
            ),
            Some(&admin.access_token),
            Some(json!({ "allowed_accounts": [claude_a.id, codex_a.id] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let acl: AccountAclDto = TestHarness::decode(body).await;
    assert!(acl.restricted);
    assert_eq!(acl.allowed_accounts.len(), 2);

    let (status, body) = h
        .request_json(
            Method::GET,
            &format!(
                "/api/admin/projects/{}/members/{}/accounts",
                alpha.id, dev.user.id
            ),
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let fetched_acl: AccountAclDto = TestHarness::decode(body).await;
    assert!(fetched_acl.restricted);
    assert_eq!(fetched_acl.allowed_accounts, acl.allowed_accounts);

    // Developer sees only allowed accounts on Alpha
    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/cli", alpha.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let cli_state: ProjectCliStateDto = TestHarness::decode(body).await;
    assert_eq!(cli_state.active_provider, CliProvider::ClaudeCode);
    assert!(cli_state.can_switch);
    let visible_ids: Vec<String> = cli_state
        .available_accounts
        .iter()
        .map(|a| a.id.clone())
        .collect();
    assert!(visible_ids.contains(&claude_a.id));
    assert!(visible_ids.contains(&codex_a.id));
    assert!(!visible_ids.contains(&claude_b.id));

    // Developer switches to allowed Claude A
    let (status, body) = h
        .request_json(
            Method::PUT,
            &format!("/api/projects/{}/cli", alpha.id),
            Some(&dev.access_token),
            Some(json!({ "provider": "claude_code", "account_id": claude_a.id })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let switched: ProjectCliStateDto = TestHarness::decode(body).await;
    assert_eq!(switched.active_provider, CliProvider::ClaudeCode);
    assert_eq!(switched.active_account_id.as_deref(), Some(claude_a.id.as_str()));

    // Developer blocked from disallowed Claude B
    let (status, _) = h
        .request_json(
            Method::PUT,
            &format!("/api/projects/{}/cli", alpha.id),
            Some(&dev.access_token),
            Some(json!({ "provider": "claude_code", "account_id": claude_b.id })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Provider/account mismatch rejected with 400 Bad Request
    let (status, _) = h
        .request_json(
            Method::PUT,
            &format!("/api/projects/{}/cli", alpha.id),
            Some(&dev.access_token),
            Some(json!({ "provider": "codex", "account_id": claude_a.id })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Developer switches provider to Codex with allowed Codex A
    let (status, body) = h
        .request_json(
            Method::PUT,
            &format!("/api/projects/{}/cli", alpha.id),
            Some(&dev.access_token),
            Some(json!({ "provider": "codex", "account_id": codex_a.id })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let switched_codex: ProjectCliStateDto = TestHarness::decode(body).await;
    assert_eq!(switched_codex.active_provider, CliProvider::Codex);
    assert_eq!(
        switched_codex.active_account_id.as_deref(),
        Some(codex_a.id.as_str())
    );

    // Unassigned project Beta is forbidden for Developer
    let (status, _) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/cli", beta.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Viewer can see active provider on Alpha, cannot see account details or switch
    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/cli", alpha.id),
            Some(&viewer.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let viewer_state: ProjectCliStateDto = TestHarness::decode(body).await;
    assert_eq!(viewer_state.active_provider, CliProvider::Codex);
    assert!(!viewer_state.can_switch);
    assert!(viewer_state.active_account_id.is_none());
    assert!(viewer_state.active_account_name.is_none());
    assert!(viewer_state.available_accounts.is_empty());

    let (status, _) = h
        .request_json(
            Method::PUT,
            &format!("/api/projects/{}/cli", alpha.id),
            Some(&viewer.access_token),
            Some(json!({ "provider": "claude_code", "account_id": claude_a.id })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Admin clears ACL restriction -> Developer can now switch to Claude B
    let (status, body) = h
        .request_json(
            Method::PUT,
            &format!(
                "/api/admin/projects/{}/members/{}/accounts",
                alpha.id, dev.user.id
            ),
            Some(&admin.access_token),
            Some(json!({ "allowed_accounts": null })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let unrestricted: AccountAclDto = TestHarness::decode(body).await;
    assert!(!unrestricted.restricted);

    let (status, body) = h
        .request_json(
            Method::PUT,
            &format!("/api/projects/{}/cli", alpha.id),
            Some(&dev.access_token),
            Some(json!({ "provider": "claude_code", "account_id": claude_b.id })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let switched_b: ProjectCliStateDto = TestHarness::decode(body).await;
    assert_eq!(
        switched_b.active_account_id.as_deref(),
        Some(claude_b.id.as_str())
    );
}

#[tokio::test]
async fn embedded_ui_serves_account_manager_oauth_xterm_and_header_switcher() {
    let h = TestHarness::new().await;

    let html_resp = h
        .server
        .router()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(html_resp.status(), StatusCode::OK);
    let html_bytes = html_resp.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8_lossy(&html_bytes);
    assert!(html.contains("Claude Web UI"));
    assert!(html.contains("xterm"));

    let js_resp = h
        .server
        .router()
        .oneshot(Request::builder().uri("/app.js").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(js_resp.status(), StatusCode::OK);
    let js_bytes = js_resp.into_body().collect().await.unwrap().to_bytes();
    let js = String::from_utf8_lossy(&js_bytes);
    assert!(js.contains("leptos.mount_cli_switcher"));
    assert!(js.contains("cwu:switch-project-cli"));
    assert!(js.contains("/login-pty"));
    // The shared attach routine focuses the terminal and reports liveness to the view.
    assert!(js.contains("term.focus()"));
    assert!(js.contains("set_oauth_terminal_live"));
    assert!(js.contains("/credentials"));
    assert!(js.contains("leptos.mount_accounts_admin"));
    assert!(js.contains("leptos.mount_account_modal"));

    let wasm_resp = h
        .server
        .router()
        .oneshot(
            Request::builder()
                .uri("/leptos_gateway_bg.wasm")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wasm_resp.status(), StatusCode::OK);
    let wasm_bytes = wasm_resp.into_body().collect().await.unwrap().to_bytes();
    let wasm = String::from_utf8_lossy(&wasm_bytes);
    assert!(wasm.contains("+ Add Account"));
    assert!(wasm.contains("Launch OAuth Terminal"));
    assert!(wasm.contains("Restart terminal"));
    assert!(wasm.contains("oauth-xterm-host"));
}
