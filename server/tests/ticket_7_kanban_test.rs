use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use claude_web_ui_server::{
    GatewayConfig, GatewayServer,
    fakes::{FakeCliRunner, FakeMessengerTransport},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn request(
    server: &GatewayServer,
    method: Method,
    path: &str,
    token: &str,
    value: Value,
) -> (StatusCode, Value) {
    let response = server
        .router()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn project_board_creates_and_moves_cards() {
    let tmp = tempfile::tempdir().unwrap();
    let server = GatewayServer::new(
        GatewayConfig::for_data_dir(tmp.path()),
        Arc::new(FakeCliRunner::new()),
        Arc::new(FakeMessengerTransport::new()),
    )
    .await
    .unwrap();
    let (_, auth) = request(
        &server,
        Method::POST,
        "/api/auth/bootstrap",
        "",
        json!({
            "username":"admin", "email":"admin@example.com", "password":"correct-horse-battery"
        }),
    )
    .await;
    let token = auth["access_token"].as_str().unwrap();
    let (_, project) = request(
        &server,
        Method::POST,
        "/api/projects",
        token,
        json!({
            "name":"Project", "workspace_path":tmp.path()
        }),
    )
    .await;
    let board = format!("/api/projects/{}/kanban", project["id"].as_str().unwrap());
    let (status, card) = request(
        &server,
        Method::POST,
        &board,
        token,
        json!({"title":"Write help text"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(card["column"], "triage");
    let move_url = format!("{}/{}/move", board, card["id"].as_str().unwrap());
    let (status, card) = request(
        &server,
        Method::POST,
        &move_url,
        token,
        json!({"column":"archived"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(card["column"], "archived");
    let (_, cards) = request(&server, Method::GET, &board, token, Value::Null).await;
    assert_eq!(cards, json!([card]));
}

use claude_web_ui_server::traits::{
    CliInvocation, CliRunner, PtyInvocation, SpawnedPty, TurnEvent,
};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

struct ControlledRunner(mpsc::UnboundedSender<(CliInvocation, oneshot::Sender<String>)>);

#[async_trait::async_trait]
impl CliRunner for ControlledRunner {
    async fn run_turn(
        &self,
        invocation: CliInvocation,
        events: mpsc::UnboundedSender<TurnEvent>,
        _approval: mpsc::UnboundedReceiver<bool>,
    ) -> Result<(), String> {
        let (sender, receiver) = oneshot::channel();
        self.0.send((invocation, sender)).unwrap();
        let result_text = receiver.await.unwrap();
        events
            .send(TurnEvent::TurnComplete {
                session_id: Some(uuid::Uuid::new_v4().to_string()),
                result_text,
            })
            .unwrap();
        Ok(())
    }

    async fn spawn_pty(&self, _: PtyInvocation) -> Result<SpawnedPty, String> {
        Err("test does not launch terminals".into())
    }
}

#[tokio::test]
async fn decomposition_mcp_links_cards_to_sequential_cli_turns() {
    let tmp = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(tmp.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    git(&["init"]);
    std::fs::write(tmp.path().join("help.txt"), "Old help\n").unwrap();
    git(&["add", "help.txt"]);
    git(&[
        "-c",
        "user.name=Tester",
        "-c",
        "user.email=test@example.com",
        "commit",
        "-m",
        "Add help file",
    ]);

    let (sender, mut turns) = mpsc::unbounded_channel();
    let server = GatewayServer::new(
        GatewayConfig::for_data_dir(tmp.path()),
        Arc::new(ControlledRunner(sender)),
        Arc::new(FakeMessengerTransport::new()),
    )
    .await
    .unwrap();
    let (_, auth) = request(
        &server,
        Method::POST,
        "/api/auth/bootstrap",
        "",
        json!({
            "username":"admin", "email":"admin@example.com", "password":"correct-horse-battery"
        }),
    )
    .await;
    let token = auth["access_token"].as_str().unwrap();
    let (_, project) = request(
        &server,
        Method::POST,
        "/api/projects",
        token,
        json!({
            "name":"Project", "workspace_path":tmp.path()
        }),
    )
    .await;
    let project_id = project["id"].as_str().unwrap();
    let board = format!("/api/projects/{project_id}/kanban");
    let mcp = format!("/api/mcp/kanban/{project_id}");
    let (_, parent) = request(
        &server,
        Method::POST,
        &board,
        token,
        json!({"title":"Update help"}),
    )
    .await;
    let parent_id = parent["id"].as_str().unwrap();
    let (status, _) = request(
        &server,
        Method::POST,
        &format!("{board}/{parent_id}/decompose"),
        token,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    let (invocation, finish_parent) = tokio::time::timeout(Duration::from_secs(5), turns.recv())
        .await
        .unwrap()
        .unwrap();
    let run_token = &invocation.env["CLAUDE_WEB_UI_KANBAN_RUN_TOKEN"];
    assert!(invocation.args.last().unwrap().contains("Plan this card"));
    let (status, tools) = request(
        &server,
        Method::POST,
        &mcp,
        run_token,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 6);

    let create_child = |title: &str, depends_on: Value| {
        json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"kanban_create","arguments":{"title":title,"column":"ready",
                    "parent_id":parent_id,"depends_on":depends_on,
                    "create_git_branch":title == "Write help text","git_branch_name":"help-update"}
            }
        })
    };
    let (_, result) = request(
        &server,
        Method::POST,
        &mcp,
        run_token,
        create_child("Write help text", json!([])),
    )
    .await;
    let first: Value =
        serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let (_, result) = request(
        &server,
        Method::POST,
        &mcp,
        run_token,
        create_child("Proofread help text", json!([first["id"]])),
    )
    .await;
    let second: Value =
        serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(second["unresolved_blockers"], json!([first["id"]]));
    assert!(turns.try_recv().is_err());
    finish_parent.send("Plan ready".into()).unwrap();

    let (invocation, finish_first) = tokio::time::timeout(Duration::from_secs(5), turns.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(invocation.args.last().unwrap().contains("Write help text"));
    let (status, _) = request(
        &server,
        Method::POST,
        &mcp,
        run_token,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (_, cards) = request(&server, Method::GET, &board, token, Value::Null).await;
    let first_running = cards
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == first["id"])
        .unwrap();
    assert_eq!(first_running["column"], "running");
    let session_id = first_running["session_id"].as_str().unwrap();
    let (status, detail) = request(
        &server,
        Method::GET,
        &format!("/api/projects/{project_id}/sessions/{session_id}"),
        token,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["session"]["id"], session_id);
    assert!(turns.try_recv().is_err());
    assert_eq!(git(&["branch", "--show-current"]), "help-update");
    std::fs::write(tmp.path().join("help.txt"), "Updated help\n").unwrap();
    finish_first.send("Draft written".into()).unwrap();

    let (invocation, finish_second) = tokio::time::timeout(Duration::from_secs(5), turns.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        invocation
            .args
            .last()
            .unwrap()
            .contains("Proofread help text")
    );
    let (_, cards) = request(&server, Method::GET, &board, token, Value::Null).await;
    let first_review = cards
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == first["id"])
        .unwrap();
    assert_eq!(first_review["column"], "review");
    assert_eq!(first_review["result_summary"], "Draft written");
    let first_id = first["id"].as_str().unwrap();
    let (status, diff) = request(
        &server,
        Method::GET,
        &format!("{board}/{first_id}/diff"),
        token,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(diff["unified"].as_str().unwrap().contains("+Updated help"));
    let (_, blocked) = request(
        &server,
        Method::POST,
        &mcp,
        &invocation.env["CLAUDE_WEB_UI_KANBAN_RUN_TOKEN"],
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"kanban_block",
            "arguments":{"card_id":second["id"],"blocked_reason":"Need product terminology"}}}),
    )
    .await;
    assert!(blocked["result"]["isError"].is_null());
    finish_second.send("Waiting".into()).unwrap();
    let (_, cards) = request(&server, Method::GET, &board, token, Value::Null).await;
    let second_blocked = cards
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == second["id"])
        .unwrap();
    assert_eq!(second_blocked["column"], "blocked");
    assert_eq!(second_blocked["blocked_reason"], "Need product terminology");
}
