use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use claude_web_ui_server::chat::{
    AdapterTurnRequest, AntigravityAdapter, ClaudeCodeAdapter, CliAdapter, CodexAdapter,
};
use claude_web_ui_server::fakes::{FakeCliRunner, FakeMessengerTransport};
use claude_web_ui_server::traits::{
    CliInvocation, CliRunner, PtyInvocation, SpawnedPty, TurnEvent,
};
use claude_web_ui_server::{GatewayConfig, GatewayServer};
use claude_web_ui_shared::accounts::{CliAccountDto, CliProvider};
use claude_web_ui_shared::auth::{AuthTokensResponse, InviteDto, ProjectDto};
use claude_web_ui_shared::chat::{
    ApprovalMode, ChatSessionDetailDto, ChatSessionDto, ChatStreamEnvelope, ChatTurnDto,
    ChatTurnStatus, SchedulerStatusDto, SendChatTurnResponse,
};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::{mpsc, Notify};
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

    async fn with_runner(runner: Arc<dyn CliRunner>) -> (TempDir, PathBuf, GatewayServer) {
        let tmp = TempDir::new().expect("tempdir");
        let data_dir = tmp.path().join("data");
        let mut config = GatewayConfig::for_data_dir(&data_dir);
        config.jwt_secret = "test-jwt-secret-key-at-least-32-bytes-long".to_string();
        let server = GatewayServer::new(
            config,
            runner,
            Arc::new(FakeMessengerTransport::new()),
        )
        .await
        .expect("gateway server init");
        (tmp, data_dir, server)
    }

    async fn request_json(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        request_json_on(&self.server, method, path, token, body).await
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
        let ws = self
            ._tmp
            .path()
            .join(format!("ws-{}", name.to_lowercase().replace(' ', "-")));
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

    async fn create_account(
        &self,
        admin_token: &str,
        provider: CliProvider,
        name: &str,
    ) -> CliAccountDto {
        let (status, body) = self
            .request_json(
                Method::POST,
                "/api/admin/accounts",
                Some(admin_token),
                Some(json!({ "provider": provider, "name": name })),
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
        spawn_tcp_on(&self.server).await
    }
}

async fn spawn_tcp_on(server: &GatewayServer) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    let router = server.router();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (addr, handle)
}

async fn request_json_on(
    server: &GatewayServer,
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
    let resp = server
        .router()
        .oneshot(req)
        .await
        .expect("router response");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let val = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, val)
}

struct BlockingConcurrencyRunner {
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    invocations: std::sync::Mutex<Vec<CliInvocation>>,
    release_first: Arc<Notify>,
    first_started: Arc<Notify>,
}

#[async_trait::async_trait]
impl CliRunner for BlockingConcurrencyRunner {
    async fn run_turn(
        &self,
        invocation: CliInvocation,
        event_tx: mpsc::UnboundedSender<TurnEvent>,
        _approval_rx: mpsc::UnboundedReceiver<bool>,
    ) -> Result<(), String> {
        let count = {
            let mut guard = self.invocations.lock().unwrap();
            guard.push(invocation);
            guard.len()
        };
        let active = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(active, Ordering::SeqCst);

        if count == 1 {
            self.first_started.notify_one();
            self.release_first.notified().await;
        }

        let _ = event_tx.send(TurnEvent::TextDelta {
            text: format!("reply-{count}"),
        });
        let _ = event_tx.send(TurnEvent::TurnComplete {
            session_id: Some(format!("native-{count}")),
            result_text: format!("reply-{count}"),
        });

        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }

    async fn spawn_pty(&self, _invocation: PtyInvocation) -> Result<SpawnedPty, String> {
        Err("unused".to_string())
    }
}

#[tokio::test]
async fn provider_adapters_and_single_active_cli_scheduler_queue() {
    // 1. Verify provider adapters produce exact CLI flags and env vars
    let mut env = BTreeMap::new();
    env.insert("CLAUDE_CONFIG_DIR".to_string(), "/tmp/acc-a".to_string());
    let claude_inv = ClaudeCodeAdapter.build_invocation(&AdapterTurnRequest {
        prompt: "hello".to_string(),
        resume_session_id: Some("sess-123".to_string()),
        working_dir: PathBuf::from("/ws"),
        env: env.clone(),
        approval_mode: ApprovalMode::FullAuto,
    });
    assert_eq!(claude_inv.program, "claude");
    assert!(claude_inv.args.contains(&"-p".to_string()));
    assert!(claude_inv
        .args
        .windows(2)
        .any(|w| w == ["--resume", "sess-123"]));
    assert!(claude_inv
        .args
        .windows(2)
        .any(|w| w == ["--output-format", "stream-json"]));
    assert!(claude_inv
        .args
        .windows(2)
        .any(|w| w == ["--input-format", "stream-json"]));
    assert!(claude_inv.args.contains(&"--verbose".to_string()));
    assert!(claude_inv
        .args
        .contains(&"--dangerously-skip-permissions".to_string()));
    assert_eq!(claude_inv.env.get("CLAUDE_CONFIG_DIR").map(String::as_str), Some("/tmp/acc-a"));

    let claude_interactive = ClaudeCodeAdapter.build_invocation(&AdapterTurnRequest {
        prompt: "hello".to_string(),
        resume_session_id: None,
        working_dir: PathBuf::from("/ws"),
        env: env.clone(),
        approval_mode: ApprovalMode::Interactive,
    });
    assert!(!claude_interactive
        .args
        .contains(&"--dangerously-skip-permissions".to_string()));

    let codex_inv = CodexAdapter.build_invocation(&AdapterTurnRequest {
        prompt: "fix bug".to_string(),
        resume_session_id: Some("codex-99".to_string()),
        working_dir: PathBuf::from("/ws"),
        env: BTreeMap::from([("CODEX_HOME".to_string(), "/tmp/codex-a".to_string())]),
        approval_mode: ApprovalMode::FullAuto,
    });
    assert_eq!(codex_inv.program, "codex");
    assert!(codex_inv.args.contains(&"exec".to_string()));
    assert!(codex_inv.args.contains(&"--json".to_string()));
    assert!(codex_inv
        .args
        .windows(2)
        .any(|w| w == ["resume", "codex-99"]));

    let agy_inv = AntigravityAdapter.build_invocation(&AdapterTurnRequest {
        prompt: "inspect".to_string(),
        resume_session_id: None,
        working_dir: PathBuf::from("/ws"),
        env: BTreeMap::from([(
            "ANTIGRAVITY_CONFIG_DIR".to_string(),
            "/tmp/agy-a".to_string(),
        )]),
        approval_mode: ApprovalMode::FullAuto,
    });
    assert_eq!(agy_inv.program, "agy");
    assert_eq!(
        agy_inv.env.get("ANTIGRAVITY_CONFIG_DIR").map(String::as_str),
        Some("/tmp/agy-a")
    );

    // 2. Verify single-active-CLI queueing through GatewayServer seam
    let runner = Arc::new(BlockingConcurrencyRunner {
        in_flight: AtomicUsize::new(0),
        max_in_flight: AtomicUsize::new(0),
        invocations: std::sync::Mutex::new(Vec::new()),
        release_first: Arc::new(Notify::new()),
        first_started: Arc::new(Notify::new()),
    });
    let (_tmp, _data_dir, server) = TestHarness::with_runner(runner.clone()).await;

    let (_, admin_val) = request_json_on(
        &server,
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
    let admin: AuthTokensResponse = serde_json::from_value(admin_val).unwrap();
    let ws = _tmp.path().join("proj-ws");
    std::fs::create_dir_all(&ws).unwrap();
    let (_, proj_val) = request_json_on(
        &server,
        Method::POST,
        "/api/projects",
        Some(&admin.access_token),
        Some(json!({ "name": "Core", "workspace_path": ws.to_string_lossy() })),
    )
    .await;
    let project: ProjectDto = serde_json::from_value(proj_val).unwrap();

    let (status, sess_val) = request_json_on(
        &server,
        Method::POST,
        &format!("/api/projects/{}/sessions", project.id),
        Some(&admin.access_token),
        Some(json!({ "title": "First Session" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let session: ChatSessionDto = serde_json::from_value(sess_val).unwrap();

    let (status, t1_val) = request_json_on(
        &server,
        Method::POST,
        &format!("/api/projects/{}/sessions/{}/turns", project.id, session.id),
        Some(&admin.access_token),
        Some(json!({ "prompt": "Turn 1" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let t1: SendChatTurnResponse = serde_json::from_value(t1_val).unwrap();

    runner.first_started.notified().await;

    let (status, t2_val) = request_json_on(
        &server,
        Method::POST,
        &format!("/api/projects/{}/sessions/{}/turns", project.id, session.id),
        Some(&admin.access_token),
        Some(json!({ "prompt": "Turn 2" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let t2: SendChatTurnResponse = serde_json::from_value(t2_val).unwrap();
    assert!(t2.queue_position >= 1);

    // Release Turn 1 and wait briefly for Turn 2 to finish
    runner.release_first.notify_one();
    for _ in 0..40 {
        let (_, detail_val) = request_json_on(
            &server,
            Method::GET,
            &format!("/api/projects/{}/sessions/{}", project.id, session.id),
            Some(&admin.access_token),
            None,
        )
        .await;
        let detail: ChatSessionDetailDto = serde_json::from_value(detail_val).unwrap();
        if detail.turns.len() == 2
            && detail.turns.iter().all(|t| t.status == ChatTurnStatus::Completed)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    assert_eq!(runner.max_in_flight.load(Ordering::SeqCst), 1);
    let (_, detail_val) = request_json_on(
        &server,
        Method::GET,
        &format!("/api/projects/{}/sessions/{}", project.id, session.id),
        Some(&admin.access_token),
        None,
    )
    .await;
    let detail: ChatSessionDetailDto = serde_json::from_value(detail_val).unwrap();
    assert_eq!(detail.turns.len(), 2);
    assert_eq!(detail.turns[0].id, t1.turn.id);
    assert_eq!(detail.turns[0].status, ChatTurnStatus::Completed);
    assert_eq!(detail.turns[1].id, t2.turn.id);
    assert_eq!(detail.turns[1].status, ChatTurnStatus::Completed);
}

struct HangUntilCancelledRunner {
    started: Arc<Notify>,
    call_count: AtomicUsize,
}

#[async_trait::async_trait]
impl CliRunner for HangUntilCancelledRunner {
    async fn run_turn(
        &self,
        _invocation: CliInvocation,
        event_tx: mpsc::UnboundedSender<TurnEvent>,
        _approval_rx: mpsc::UnboundedReceiver<bool>,
    ) -> Result<(), String> {
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
        if idx == 1 {
            let _ = event_tx.send(TurnEvent::TextDelta {
                text: "working...".to_string(),
            });
            self.started.notify_one();
            std::future::pending::<()>().await;
        } else {
            let _ = event_tx.send(TurnEvent::TurnComplete {
                session_id: Some("after-stop".to_string()),
                result_text: "recovered".to_string(),
            });
        }
        Ok(())
    }

    async fn spawn_pty(&self, _invocation: PtyInvocation) -> Result<SpawnedPty, String> {
        Err("unused".to_string())
    }
}

#[tokio::test]
async fn websocket_streams_all_turn_events_and_stop_cancels_active_turn() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;
    let project = h.create_project(&admin.access_token, "StreamProj").await;

    let (status, sess_val) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/sessions", project.id),
            Some(&admin.access_token),
            Some(json!({ "title": "WS Stream" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let session: ChatSessionDto = TestHarness::decode(sess_val).await;

    h.cli_runner.push_turn(vec![
        TurnEvent::ThinkingDelta {
            text: "analyzing repo".to_string(),
        },
        TurnEvent::SubagentStart {
            id: "sub-1".to_string(),
            name: "explorer".to_string(),
        },
        TurnEvent::ToolCallStart {
            id: "tool-1".to_string(),
            name: "Read".to_string(),
            input: json!({ "path": "Cargo.toml" }),
        },
        TurnEvent::ToolCallResult {
            id: "tool-1".to_string(),
            output: "[workspace]".to_string(),
            is_error: false,
        },
        TurnEvent::SubagentEnd {
            id: "sub-1".to_string(),
        },
        TurnEvent::TextDelta {
            text: "Done reading.".to_string(),
        },
        TurnEvent::TurnComplete {
            session_id: Some("native-ws-1".to_string()),
            result_text: "Done reading.".to_string(),
        },
    ]);

    let (addr, server_task) = h.spawn_tcp().await;
    let ws_url = format!(
        "ws://{}/api/projects/{}/chat/ws?token={}&session_id={}",
        addr, project.id, admin.access_token, session.id
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("connect chat ws");

    // Read initial scheduler_status frame
    let init_frame = ws.next().await.unwrap().unwrap();
    let init_json: Value = serde_json::from_str(&init_frame.into_text().unwrap()).unwrap();
    assert_eq!(init_json["type"], "scheduler_status");

    let (status, _) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/sessions/{}/turns", project.id, session.id),
            Some(&admin.access_token),
            Some(json!({ "prompt": "Inspect Cargo.toml" })),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    let mut received = Vec::new();
    for _ in 0..7 {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
            .await
            .expect("ws event timeout")
            .unwrap()
            .unwrap();
        let env: ChatStreamEnvelope = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
        received.push(env.event);
    }

    assert!(matches!(received[0], TurnEvent::ThinkingDelta { .. }));
    assert!(matches!(received[1], TurnEvent::SubagentStart { .. }));
    assert!(matches!(received[2], TurnEvent::ToolCallStart { .. }));
    assert!(matches!(received[3], TurnEvent::ToolCallResult { .. }));
    assert!(matches!(received[4], TurnEvent::SubagentEnd { .. }));
    assert!(matches!(received[5], TurnEvent::TextDelta { .. }));
    assert!(matches!(
        received[6],
        TurnEvent::TurnComplete {
            session_id: Some(ref sid),
            ..
        } if sid == "native-ws-1"
    ));
    server_task.abort();

    // Now test [Stop] immediate cancellation on a hanging turn
    let hang_runner = Arc::new(HangUntilCancelledRunner {
        started: Arc::new(Notify::new()),
        call_count: AtomicUsize::new(0),
    });
    let (tmp2, _data2, server2) = TestHarness::with_runner(hang_runner.clone()).await;
    let (_, admin2_val) = request_json_on(
        &server2,
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
    let admin2: AuthTokensResponse = serde_json::from_value(admin2_val).unwrap();
    let ws_dir = tmp2.path().join("stop-ws");
    std::fs::create_dir_all(&ws_dir).unwrap();
    let (_, proj2_val) = request_json_on(
        &server2,
        Method::POST,
        "/api/projects",
        Some(&admin2.access_token),
        Some(json!({ "name": "StopProj", "workspace_path": ws_dir.to_string_lossy() })),
    )
    .await;
    let proj2: ProjectDto = serde_json::from_value(proj2_val).unwrap();
    let (_, s2_val) = request_json_on(
        &server2,
        Method::POST,
        &format!("/api/projects/{}/sessions", proj2.id),
        Some(&admin2.access_token),
        Some(json!({ "title": "Stoppable" })),
    )
    .await;
    let s2: ChatSessionDto = serde_json::from_value(s2_val).unwrap();

    let (addr2, task2) = spawn_tcp_on(&server2).await;
    let (mut ws2, _) = tokio_tungstenite::connect_async(&format!(
        "ws://{}/api/projects/{}/chat/ws?token={}&session_id={}",
        addr2, proj2.id, admin2.access_token, s2.id
    ))
    .await
    .unwrap();
    let _ = ws2.next().await;

    let (status, t_stop_val) = request_json_on(
        &server2,
        Method::POST,
        &format!("/api/projects/{}/sessions/{}/turns", proj2.id, s2.id),
        Some(&admin2.access_token),
        Some(json!({ "prompt": "Run forever" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let t_stop: SendChatTurnResponse = serde_json::from_value(t_stop_val).unwrap();

    hang_runner.started.notified().await;

    let (status, cancel_val) = request_json_on(
        &server2,
        Method::POST,
        &format!("/api/projects/{}/sessions/{}/stop", proj2.id, s2.id),
        Some(&admin2.access_token),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cancel_val["cancelled"], true);

    let mut saw_cancelled_err = false;
    for _ in 0..5 {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws2.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let env: ChatStreamEnvelope = serde_json::from_str(&msg.into_text().unwrap()).unwrap();
        if env.status == ChatTurnStatus::Cancelled
            && matches!(env.event, TurnEvent::TurnError { .. })
        {
            saw_cancelled_err = true;
            break;
        }
    }
    assert!(saw_cancelled_err);

    // Next turn runs normally after cancellation
    let (status, t_next_val) = request_json_on(
        &server2,
        Method::POST,
        &format!("/api/projects/{}/sessions/{}/turns", proj2.id, s2.id),
        Some(&admin2.access_token),
        Some(json!({ "prompt": "Follow up" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let t_next: SendChatTurnResponse = serde_json::from_value(t_next_val).unwrap();

    for _ in 0..30 {
        let (_, detail_val) = request_json_on(
            &server2,
            Method::GET,
            &format!("/api/projects/{}/sessions/{}", proj2.id, s2.id),
            Some(&admin2.access_token),
            None,
        )
        .await;
        let detail: ChatSessionDetailDto = serde_json::from_value(detail_val).unwrap();
        if detail
            .turns
            .iter()
            .any(|t| t.id == t_next.turn.id && t.status == ChatTurnStatus::Completed)
        {
            assert_eq!(
                detail
                    .turns
                    .iter()
                    .find(|t| t.id == t_stop.turn.id)
                    .unwrap()
                    .status,
                ChatTurnStatus::Cancelled
            );
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    task2.abort();
}

#[tokio::test]
async fn native_jsonl_session_discovery_and_cross_account_resume_continuity() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;
    let project = h.create_project(&admin.access_token, "Alpha App").await;

    // Create two Claude accounts and one Codex account
    let claude_a = h
        .create_account(&admin.access_token, CliProvider::ClaudeCode, "Claude A")
        .await;
    let claude_b = h
        .create_account(&admin.access_token, CliProvider::ClaudeCode, "Claude B")
        .await;
    let _codex_a = h
        .create_account(&admin.access_token, CliProvider::Codex, "Codex A")
        .await;

    // Write native SSH-created Claude Code JSONL transcript under shared/claude_code/projects/<slug>/
    let claude_proj_dir = h
        .data_dir
        .join("shared/claude_code/projects")
        .join(&project.slug);
    std::fs::create_dir_all(&claude_proj_dir).unwrap();
    let claude_jsonl = [
        json!({
            "type": "user",
            "sessionId": "ssh-claude-42",
            "cwd": project.workspace_path,
            "message": { "role": "user", "content": "Fix flaky SSH migration test" },
            "timestamp": "2026-11-17T10:00:00Z"
        })
        .to_string(),
        json!({
            "type": "assistant",
            "sessionId": "ssh-claude-42",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "thinking", "thinking": "Checking migration SQL" },
                    { "type": "text", "text": "Applied fix to migration." }
                ]
            },
            "timestamp": "2026-11-17T10:00:05Z"
        })
        .to_string(),
    ]
    .join("\n");
    std::fs::write(claude_proj_dir.join("ssh-claude-42.jsonl"), claude_jsonl).unwrap();

    // Write native Codex JSONL transcript under shared/codex/sessions/2026/11/17/
    let codex_sess_dir = h.data_dir.join("shared/codex/sessions/2026/11/17");
    std::fs::create_dir_all(&codex_sess_dir).unwrap();
    let codex_jsonl = [
        json!({
            "type": "session_meta",
            "payload": { "id": "codex-ssh-99", "cwd": project.workspace_path }
        })
        .to_string(),
        json!({
            "type": "response_item",
            "payload": { "role": "user", "content": "Audit codex rollout" }
        })
        .to_string(),
    ]
    .join("\n");
    std::fs::write(
        codex_sess_dir.join("rollout-2026-codex-ssh-99.jsonl"),
        codex_jsonl,
    )
    .unwrap();

    // Unified session list discovers both native sessions and supports search
    let (status, list_val) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/sessions", project.id),
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let sessions: Vec<ChatSessionDto> = TestHarness::decode(list_val).await;
    assert_eq!(sessions.len(), 2);
    let discovered_claude = sessions
        .iter()
        .find(|s| s.native_session_id.as_deref() == Some("ssh-claude-42"))
        .expect("discovered claude session");
    assert_eq!(discovered_claude.source, "native");
    assert_eq!(discovered_claude.title, "Fix flaky SSH migration test");

    let (_, filtered_val) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/sessions?q=rollout", project.id),
            Some(&admin.access_token),
            None,
        )
        .await;
    let filtered: Vec<ChatSessionDto> = TestHarness::decode(filtered_val).await;
    assert_eq!(filtered.len(), 1);
    assert_eq!(
        filtered[0].native_session_id.as_deref(),
        Some("codex-ssh-99")
    );

    // Inspecting the discovered Claude session exposes parsed native JSONL messages
    let (status, detail_val) = h
        .request_json(
            Method::GET,
            &format!(
                "/api/projects/{}/sessions/{}",
                project.id, discovered_claude.id
            ),
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let detail: ChatSessionDetailDto = TestHarness::decode(detail_val).await;
    assert_eq!(detail.native_messages.len(), 3);
    assert_eq!(detail.native_messages[0].role, "user");
    assert_eq!(detail.native_messages[1].role, "thinking");
    assert_eq!(detail.native_messages[2].text, "Applied fix to migration.");

    // Cross-account --resume continuity:
    // Start a fresh Gateway session on Account A
    let (status, _) = h
        .request_json(
            Method::PUT,
            &format!("/api/projects/{}/cli", project.id),
            Some(&admin.access_token),
            Some(json!({ "provider": "claude_code", "account_id": claude_a.id })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, gw_sess_val) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/sessions", project.id),
            Some(&admin.access_token),
            Some(json!({ "title": "Account Switch Test" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let gw_sess: ChatSessionDto = TestHarness::decode(gw_sess_val).await;

    h.cli_runner.push_turn(vec![
        TurnEvent::TextDelta {
            text: "Part 1".to_string(),
        },
        TurnEvent::TurnComplete {
            session_id: Some("claude-conv-777".to_string()),
            result_text: "Part 1".to_string(),
        },
    ]);

    let (status, _) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/sessions/{}/turns", project.id, gw_sess.id),
            Some(&admin.access_token),
            Some(json!({ "prompt": "Start work on Account A" })),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    for _ in 0..30 {
        let (_, d) = h
            .request_json(
                Method::GET,
                &format!("/api/projects/{}/sessions/{}", project.id, gw_sess.id),
                Some(&admin.access_token),
                None,
            )
            .await;
        let parsed: ChatSessionDetailDto = TestHarness::decode(d).await;
        if parsed.session.native_session_id.as_deref() == Some("claude-conv-777") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // Switch project account to Claude Account B and send follow-up prompt
    let (status, _) = h
        .request_json(
            Method::PUT,
            &format!("/api/projects/{}/cli", project.id),
            Some(&admin.access_token),
            Some(json!({ "provider": "claude_code", "account_id": claude_b.id })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    h.cli_runner.push_turn(vec![
        TurnEvent::TextDelta {
            text: "Part 2".to_string(),
        },
        TurnEvent::TurnComplete {
            session_id: Some("claude-conv-777".to_string()),
            result_text: "Part 2".to_string(),
        },
    ]);

    let (status, _) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/sessions/{}/turns", project.id, gw_sess.id),
            Some(&admin.access_token),
            Some(json!({ "prompt": "Continue on Account B" })),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    for _ in 0..30 {
        if h.cli_runner.recorded_invocations().len() >= 2 {
            let (_, d) = h
                .request_json(
                    Method::GET,
                    &format!("/api/projects/{}/sessions/{}", project.id, gw_sess.id),
                    Some(&admin.access_token),
                    None,
                )
                .await;
            let parsed: ChatSessionDetailDto = TestHarness::decode(d).await;
            if parsed.turns.len() == 2
                && parsed.turns[1].status == ChatTurnStatus::Completed
            {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let invocations = h.cli_runner.recorded_invocations();
    assert_eq!(invocations.len(), 2);
    assert_eq!(
        invocations[0].env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
        Some(claude_a.profile_dir.as_str())
    );
    assert!(!invocations[0].args.contains(&"--resume".to_string()));

    assert_eq!(
        invocations[1].env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
        Some(claude_b.profile_dir.as_str())
    );
    assert!(invocations[1]
        .args
        .windows(2)
        .any(|w| w == ["--resume", "claude-conv-777"]));
}

#[tokio::test]
async fn interactive_approval_pauses_turn_and_resolves_via_allow_and_deny() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;
    let project = h.create_project(&admin.access_token, "ApprovalProj").await;

    let (status, sess_val) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/sessions", project.id),
            Some(&admin.access_token),
            Some(json!({ "title": "Approval Session" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let session: ChatSessionDto = TestHarness::decode(sess_val).await;
    assert_eq!(session.approval_mode, ApprovalMode::FullAuto);

    // Toggle to Interactive Approval mode
    let (status, patched_val) = h
        .request_json(
            Method::PATCH,
            &format!("/api/projects/{}/sessions/{}", project.id, session.id),
            Some(&admin.access_token),
            Some(json!({ "approval_mode": "interactive" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let patched: ChatSessionDto = TestHarness::decode(patched_val).await;
    assert_eq!(patched.approval_mode, ApprovalMode::Interactive);

    // Turn 1: Allow permission
    h.cli_runner.push_turn(vec![
        TurnEvent::PermissionRequest {
            request_id: "req-1".to_string(),
            tool_name: "Bash".to_string(),
            input: json!({ "command": "cargo test" }),
        },
        TurnEvent::TextDelta {
            text: "Tests ran".to_string(),
        },
        TurnEvent::TurnComplete {
            session_id: Some("sess-app-1".to_string()),
            result_text: "Tests ran".to_string(),
        },
    ]);

    let (status, turn1_val) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/sessions/{}/turns", project.id, session.id),
            Some(&admin.access_token),
            Some(json!({ "prompt": "Run cargo test" })),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let turn1: SendChatTurnResponse = TestHarness::decode(turn1_val).await;

    // Wait until scheduler reports WaitingApproval with pending_permission
    let mut paused = false;
    for _ in 0..40 {
        let (_, sched_val) = h
            .request_json(
                Method::GET,
                &format!("/api/projects/{}/scheduler", project.id),
                Some(&admin.access_token),
                None,
            )
            .await;
        let sched: SchedulerStatusDto = TestHarness::decode(sched_val).await;
        if sched
            .pending_permission
            .as_ref()
            .is_some_and(|p| p.request_id == "req-1" && p.tool_name == "Bash")
        {
            paused = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(paused, "turn should pause on PermissionRequest");

    let (status, resolved_val) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/turns/{}/permission", project.id, turn1.turn.id),
            Some(&admin.access_token),
            Some(json!({ "approved": true, "request_id": "req-1" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let _: ChatTurnDto = TestHarness::decode(resolved_val).await;

    for _ in 0..40 {
        let (_, d_val) = h
            .request_json(
                Method::GET,
                &format!("/api/projects/{}/sessions/{}", project.id, session.id),
                Some(&admin.access_token),
                None,
            )
            .await;
        let d: ChatSessionDetailDto = TestHarness::decode(d_val).await;
        if d.turns[0].status == ChatTurnStatus::Completed {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // Turn 2: Deny permission
    h.cli_runner.push_turn(vec![TurnEvent::PermissionRequest {
        request_id: "req-2".to_string(),
        tool_name: "Bash".to_string(),
        input: json!({ "command": "rm -rf ." }),
    }]);

    let (status, turn2_val) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/sessions/{}/turns", project.id, session.id),
            Some(&admin.access_token),
            Some(json!({ "prompt": "Delete files" })),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let turn2: SendChatTurnResponse = TestHarness::decode(turn2_val).await;

    for _ in 0..40 {
        let (_, sched_val) = h
            .request_json(
                Method::GET,
                &format!("/api/projects/{}/scheduler", project.id),
                Some(&admin.access_token),
                None,
            )
            .await;
        let sched: SchedulerStatusDto = TestHarness::decode(sched_val).await;
        if sched
            .pending_permission
            .as_ref()
            .is_some_and(|p| p.request_id == "req-2")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let (status, _) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/turns/{}/permission", project.id, turn2.turn.id),
            Some(&admin.access_token),
            Some(json!({ "approved": false, "request_id": "req-2" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    for _ in 0..40 {
        let (_, d_val) = h
            .request_json(
                Method::GET,
                &format!("/api/projects/{}/sessions/{}", project.id, session.id),
                Some(&admin.access_token),
                None,
            )
            .await;
        let d: ChatSessionDetailDto = TestHarness::decode(d_val).await;
        if d.turns.len() == 2 && d.turns[1].status == ChatTurnStatus::Error {
            assert_eq!(d.turns[0].status, ChatTurnStatus::Completed);
            assert_eq!(
                d.turns[1].error_message.as_deref(),
                Some("permission denied")
            );
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let decisions = h.cli_runner.approval_decisions.lock().unwrap().clone();
    assert_eq!(decisions, vec![true, false]);
}

#[tokio::test]
async fn project_terminal_pty_and_rbac_enforces_viewer_readonly() {
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

    let claude_acc = h
        .create_account(&admin.access_token, CliProvider::ClaudeCode, "Dev Claude")
        .await;
    let (status, _) = h
        .request_json(
            Method::PUT,
            &format!("/api/projects/{}/cli", alpha.id),
            Some(&admin.access_token),
            Some(json!({ "provider": "claude_code", "account_id": claude_acc.id })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, sess_val) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/sessions", alpha.id),
            Some(&dev.access_token),
            Some(json!({ "title": "Dev Session", "native_session_id": "resume-pty-88" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let session: ChatSessionDto = TestHarness::decode(sess_val).await;

    // Viewer can read sessions & connect read-only to /chat/ws, but gets 403 on mutations and PTY
    let (status, _) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/sessions", alpha.id),
            Some(&viewer.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/sessions", alpha.id),
            Some(&viewer.access_token),
            Some(json!({ "title": "Blocked" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/sessions/{}/turns", alpha.id, session.id),
            Some(&viewer.access_token),
            Some(json!({ "prompt": "Blocked" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/terminal/ws", alpha.id),
            Some(&viewer.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Unassigned project Beta is forbidden for dev
    let (status, _) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/sessions", beta.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Spawn TCP server to test real WebSocket upgrade for Viewer read-only chat WS and Developer PTY WS
    let (addr, server_task) = h.spawn_tcp().await;

    let (mut viewer_ws, _) = tokio_tungstenite::connect_async(&format!(
        "ws://{}/api/projects/{}/chat/ws?token={}",
        addr, alpha.id, viewer.access_token
    ))
    .await
    .expect("viewer can subscribe to read-only chat ws");
    let first_msg = viewer_ws.next().await.unwrap().unwrap();
    assert!(first_msg.into_text().unwrap().contains("scheduler_status"));

    // Developer opens PTY terminal resuming session
    h.cli_runner
        .push_pty_output(b"Resumed Claude session\r\n".to_vec());
    let (mut pty_ws, _) = tokio_tungstenite::connect_async(&format!(
        "ws://{}/api/projects/{}/terminal/ws?token={}&session_id={}&cols=100&rows=30",
        addr, alpha.id, dev.access_token, session.id
    ))
    .await
    .expect("developer can open project PTY");

    let out_msg = pty_ws.next().await.unwrap().unwrap();
    let out_json: Value = serde_json::from_str(&out_msg.into_text().unwrap()).unwrap();
    assert_eq!(out_json["type"], "output");
    assert_eq!(out_json["data"], "Resumed Claude session\r\n");

    pty_ws
        .send(WsMessage::Text(
            json!({ "type": "resize", "cols": 120, "rows": 40 })
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    pty_ws
        .send(WsMessage::Text(
            json!({ "type": "input", "data": "status\r" })
                .to_string()
                .into(),
        ))
        .await
        .unwrap();

    for _ in 0..3 {
        let _ = pty_ws.next().await;
    }

    let pty_invs = h.cli_runner.recorded_pty_invocations();
    assert_eq!(pty_invs.len(), 1);
    assert_eq!(pty_invs[0].program, "claude");
    assert_eq!(pty_invs[0].args, vec!["--resume", "resume-pty-88"]);
    assert_eq!(
        pty_invs[0].working_dir,
        PathBuf::from(&alpha.workspace_path)
    );
    assert_eq!(
        pty_invs[0].env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
        Some(claude_acc.profile_dir.as_str())
    );
    assert_eq!(h.cli_runner.recorded_pty_resizes(), vec![(120, 40)]);
    assert!(!h.cli_runner.recorded_pty_inputs().is_empty());

    server_task.abort();
}

#[tokio::test]
async fn embedded_ui_serves_chat_workspace_approvals_and_project_terminal() {
    let h = TestHarness::new().await;
    let resp = h
        .server
        .router()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/app.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let js = String::from_utf8_lossy(&body);
    assert!(js.contains("chat-workspace"));
    assert!(js.contains("toggle-approval-mode-btn"));
    assert!(js.contains("stop-active-turn-btn"));
    assert!(js.contains("data-allow-turn"));
    assert!(js.contains("data-deny-turn"));
    assert!(js.contains("/terminal/ws"));
    assert!(js.contains("/chat/ws"));
}


