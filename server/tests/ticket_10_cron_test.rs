use axum::{
    body::Body,
    http::{Request, StatusCode},
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
    method: &str,
    path: &str,
    token: &str,
    data: Value,
    expected: StatusCode,
) -> Value {
    let response = server
        .router()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(data.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(status, expected, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).unwrap_or(Value::Null)
}

#[tokio::test]
async fn cron_definitions_are_editable_and_persist_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let config = GatewayConfig::for_data_dir(temp.path().join("data"));
    let make_gateway = || {
        GatewayServer::new(
            config.clone(),
            Arc::new(FakeCliRunner::new()),
            Arc::new(FakeMessengerTransport::new()),
        )
    };
    let server = make_gateway().await.unwrap();
    let auth = request(
        &server,
        "POST",
        "/api/auth/bootstrap",
        "",
        json!({
            "username": "admin", "email": "admin@example.com", "password": "test-password-long"
        }),
        StatusCode::CREATED,
    )
    .await;
    let token = auth["access_token"].as_str().unwrap();
    let project = request(
        &server,
        "POST",
        "/api/projects",
        token,
        json!({
            "name": "Cron test", "workspace_path": temp.path().to_string_lossy()
        }),
        StatusCode::CREATED,
    )
    .await;
    let path = format!("/api/projects/{}/cron", project["id"].as_str().unwrap());
    let body =
        json!({"expression": "0 0 1 1 *", "prompt": "Review build artifacts", "enabled": true});
    let job = request(
        &server,
        "POST",
        &path,
        token,
        body.clone(),
        StatusCode::CREATED,
    )
    .await;
    let job_path = format!("{path}/{}", job["id"].as_str().unwrap());
    let mut edited = body;
    edited["enabled"] = json!(false);
    edited["prompt"] = json!("Review the build log");
    request(&server, "PUT", &job_path, token, edited, StatusCode::OK).await;
    drop(server);
    let server = make_gateway().await.unwrap();
    let jobs = request(&server, "GET", &path, token, Value::Null, StatusCode::OK).await;
    assert_eq!(jobs["jobs"][0]["prompt"], "Review the build log");
    assert_eq!(jobs["jobs"][0]["enabled"], false);
    request(
        &server,
        "DELETE",
        &job_path,
        token,
        Value::Null,
        StatusCode::NO_CONTENT,
    )
    .await;
    let jobs = request(&server, "GET", &path, token, Value::Null, StatusCode::OK).await;
    assert_eq!(jobs["jobs"], json!([]));
}
async fn member(server: &GatewayServer, admin: &str, project: &str, role: &str) -> String {
    let invite = request(
        server,
        "POST",
        "/api/admin/invites",
        admin,
        json!({"role": role, "project_ids": [project]}),
        StatusCode::CREATED,
    )
    .await;
    let auth = request(
        server,
        "POST",
        "/api/auth/invites/redeem",
        "",
        json!({
            "code": invite["code"], "username": role, "email": format!("{role}@example.com"),
            "password": "test-password-long"
        }),
        StatusCode::CREATED,
    )
    .await;
    auth["access_token"].as_str().unwrap().to_owned()
}

async fn poll(mut predicate: impl AsyncFnMut() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(12), async {
        while !predicate().await {
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }
    })
    .await
    .expect("condition should become true");
}

#[tokio::test]
async fn manual_and_scheduled_runs_use_the_chat_queue_and_report_after_completion() {
    use claude_web_ui_server::traits::{IncomingMessengerEvent, MessengerPlatform, TurnEvent};
    let temp = tempfile::tempdir().unwrap();
    let config = GatewayConfig::for_data_dir(temp.path().join("data"));
    let runner = Arc::new(FakeCliRunner::new());
    let messenger = Arc::new(FakeMessengerTransport::new());
    let make_gateway = || GatewayServer::new(config.clone(), runner.clone(), messenger.clone());
    let server = make_gateway().await.unwrap();
    let auth = request(
        &server,
        "POST",
        "/api/auth/bootstrap",
        "",
        json!({
            "username": "admin", "email": "admin@example.com", "password": "test-password-long"
        }),
        StatusCode::CREATED,
    )
    .await;
    let token = auth["access_token"].as_str().unwrap();
    let project = request(
        &server,
        "POST",
        "/api/projects",
        token,
        json!({
            "name": "Testing", "workspace_path": temp.path().to_string_lossy()
        }),
        StatusCode::CREATED,
    )
    .await;
    let project = project["id"].as_str().unwrap();
    let project_path = format!("/api/projects/{project}");
    let path = format!("{project_path}/cron");
    let history = format!("{path}/runs");
    let viewer = member(&server, token, project, "viewer").await;
    let developer = member(&server, token, project, "developer").await;

    let code = request(
        &server,
        "POST",
        "/api/messenger/pairing-code",
        token,
        Value::Null,
        StatusCode::CREATED,
    )
    .await;
    server
        .handle_messenger_event(IncomingMessengerEvent {
            platform: MessengerPlatform::Telegram,
            chat_id: "42".into(),
            thread_id: None,
            external_user_id: "42".into(),
            is_private: true,
            text: Some(format!("/pair {}", code["code"].as_str().unwrap())),
            callback_data: None,
        })
        .await
        .unwrap();
    server
        .handle_messenger_event(IncomingMessengerEvent {
            platform: MessengerPlatform::Telegram,
            chat_id: "-1000".into(),
            thread_id: Some("17".into()),
            external_user_id: "42".into(),
            is_private: false,
            text: Some(format!("/bind {project}")),
            callback_data: None,
        })
        .await
        .unwrap();
    server
        .handle_messenger_event(IncomingMessengerEvent {
            platform: MessengerPlatform::Telegram,
            chat_id: "-1000".into(),
            thread_id: Some("17".into()),
            external_user_id: "42".into(),
            is_private: false,
            text: Some("/notify on".into()),
            callback_data: None,
        })
        .await
        .unwrap();

    let account = request(
        &server,
        "POST",
        "/api/admin/accounts",
        token,
        json!({"provider": "codex", "name": "Cron override"}),
        StatusCode::CREATED,
    )
    .await;
    let sessions = format!("{project_path}/sessions");
    let session = request(
        &server,
        "POST",
        &sessions,
        token,
        json!({
            "provider": "codex", "native_session_id": "pinned-native-session"
        }),
        StatusCode::CREATED,
    )
    .await;
    let pinned = session["id"].as_str().unwrap();
    let interactive = request(
        &server,
        "POST",
        &sessions,
        token,
        json!({
            "approval_mode": "interactive"
        }),
        StatusCode::CREATED,
    )
    .await;
    runner.push_turn(vec![
        TurnEvent::PermissionRequest {
            request_id: "approval".into(),
            tool_name: "Bash".into(),
            input: json!({"command": "cargo test"}),
        },
        TurnEvent::TurnComplete {
            session_id: None,
            result_text: "Interactive complete".into(),
        },
    ]);
    runner.push_turn(vec![TurnEvent::TurnComplete {
        session_id: Some("pinned-native-session".into()),
        result_text: "Checks are green".into(),
    }]);
    let session_turns = format!("{sessions}/{}/turns", interactive["id"].as_str().unwrap());
    let turn = request(
        &server,
        "POST",
        &session_turns,
        token,
        json!({"prompt": "Run the tests"}),
        StatusCode::ACCEPTED,
    )
    .await;
    let turn_id = turn["turn"]["id"].as_str().unwrap();
    let scheduler_path = format!("{project_path}/scheduler");
    poll(async || {
        let scheduler = request(
            &server,
            "GET",
            &scheduler_path,
            token,
            Value::Null,
            StatusCode::OK,
        )
        .await;
        scheduler["pending_permission"]["request_id"] == "approval"
    })
    .await;

    let config = json!({
        "enabled": true, "expression": "* * * * * *", "prompt": "Summarize the tests",
        "provider": "codex", "account_id": account["id"], "pinned_session_id": pinned,
        "notification": {"platform": "telegram", "chat_id": "-1000", "thread_id": "17"}
    });
    request(
        &server,
        "POST",
        &path,
        &viewer,
        config.clone(),
        StatusCode::FORBIDDEN,
    )
    .await;
    let job = request(
        &server,
        "POST",
        &path,
        &developer,
        config,
        StatusCode::CREATED,
    )
    .await;
    let job_path = format!("{path}/{}", job["id"].as_str().unwrap());
    let run_path = format!("{job_path}/run");
    let options = request(&server, "GET", &path, &viewer, Value::Null, StatusCode::OK).await;
    assert_eq!(options["writable"], false);
    request(
        &server,
        "POST",
        &run_path,
        &viewer,
        Value::Null,
        StatusCode::FORBIDDEN,
    )
    .await;
    let manual = request(
        &server,
        "POST",
        &run_path,
        &developer,
        Value::Null,
        StatusCode::ACCEPTED,
    )
    .await;
    assert_eq!(manual["status"], "queued");
    assert_eq!(manual["session_id"], pinned);
    poll(async || {
        let rows = request(
            &server,
            "GET",
            &history,
            &viewer,
            Value::Null,
            StatusCode::OK,
        )
        .await;
        rows.as_array()
            .unwrap()
            .iter()
            .any(|run| run["trigger"] == "scheduled")
    })
    .await;
    request(
        &server,
        "PATCH",
        &job_path,
        &developer,
        json!({"enabled": false}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        runner.recorded_invocations().len(),
        1,
        "both cron triggers must wait behind the chat"
    );
    let active = request(
        &server,
        "GET",
        &scheduler_path,
        token,
        Value::Null,
        StatusCode::OK,
    )
    .await;
    assert!(active["queued_count"].as_u64().unwrap() >= 2);

    request(
        &server,
        "POST",
        &format!("{project_path}/turns/{turn_id}/permission"),
        token,
        json!({"approved": true, "request_id": "approval"}),
        StatusCode::OK,
    )
    .await;
    poll(async || {
        let rows = request(
            &server,
            "GET",
            &history,
            &viewer,
            Value::Null,
            StatusCode::OK,
        )
        .await;
        rows.as_array()
            .unwrap()
            .iter()
            .all(|row| row["status"] == "completed")
    })
    .await;
    poll(async || {
        messenger.sent_messages().iter().any(|sent| {
            sent.message.chat_id == "-1000"
                && sent.message.thread_id.as_deref() == Some("17")
                && sent.message.text.contains("Checks are green")
        })
    })
    .await;
    let invocations = runner.recorded_invocations();
    assert!(invocations.len() >= 3);
    assert_eq!(invocations[0].program, "claude");
    for invocation in &invocations[1..] {
        assert_eq!(invocation.program, "codex");
        assert!(
            invocation
                .args
                .iter()
                .any(|arg| arg == "pinned-native-session")
        );
        assert_eq!(
            invocation.env["CODEX_HOME"],
            account["profile_dir"].as_str().unwrap()
        );
    }
    let selected = request(
        &server,
        "GET",
        &format!("{project_path}/cli"),
        &developer,
        Value::Null,
        StatusCode::OK,
    )
    .await;
    assert_eq!(selected["active_provider"], "claude_code");

    request(
        &server,
        "DELETE",
        &job_path,
        &developer,
        Value::Null,
        StatusCode::NO_CONTENT,
    )
    .await;
    drop(server);
    let server = make_gateway().await.unwrap();
    let rows = request(
        &server,
        "GET",
        &history,
        &viewer,
        Value::Null,
        StatusCode::OK,
    )
    .await;
    let saved = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|run| run["id"] == manual["id"])
        .unwrap();
    assert_eq!(saved["status"], "completed");
    assert_eq!(saved["session_id"], pinned);
    assert!(saved["started_at"].is_string());
    assert!(saved["completed_at"].is_string());
    assert!(saved["summary"].is_string() || saved["summary"].is_null());
}
