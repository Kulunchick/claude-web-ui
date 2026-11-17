use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use claude_web_ui_server::{
    GatewayConfig, GatewayServer,
    fakes::{FakeCliRunner, FakeMessengerTransport},
    traits::{IncomingMessengerEvent, MessengerPlatform, TurnEvent},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

async fn post(
    server: &GatewayServer,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    request(server, Method::POST, path, token, body).await
}

async fn request(
    server: &GatewayServer,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = server
        .router()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn telegram(user: &str, text: &str) -> IncomingMessengerEvent {
    IncomingMessengerEvent {
        platform: MessengerPlatform::Telegram,
        chat_id: user.into(),
        thread_id: None,
        external_user_id: user.into(),
        is_private: true,
        text: Some(text.into()),
        callback_data: None,
    }
}

#[tokio::test]
async fn pairing_requires_a_web_session_and_a_single_use_code() {
    let tmp = TempDir::new().unwrap();
    let transport = Arc::new(FakeMessengerTransport::new());
    let server = GatewayServer::new(
        GatewayConfig::for_data_dir(tmp.path()),
        Arc::new(FakeCliRunner::new()),
        transport.clone(),
    )
    .await
    .unwrap();
    let (_, admin) = post(
        &server,
        "/api/auth/bootstrap",
        None,
        json!({
            "username": "admin", "email": "admin@example.com", "password": "correct-horse-battery"
        }),
    )
    .await;
    assert_eq!(
        post(&server, "/api/messenger/pairing-code", None, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert!(
        server
            .handle_messenger_event(telegram("123", "/project"))
            .await
            .is_err()
    );

    let (status, pair) = post(
        &server,
        "/api/messenger/pairing-code",
        admin["access_token"].as_str(),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let code = pair["code"].as_str().unwrap();
    assert_eq!(code.len(), 6);
    assert!(code.bytes().all(|byte| byte.is_ascii_digit()));
    server
        .handle_messenger_event(telegram("123", &format!("/pair {code}")))
        .await
        .unwrap();
    assert!(
        server
            .handle_messenger_event(telegram("456", &format!("/pair {code}")))
            .await
            .is_err()
    );
    server
        .handle_messenger_event(telegram("123", "/project"))
        .await
        .unwrap();
    assert!(
        transport
            .sent_messages()
            .iter()
            .any(|message| message.message.text.contains("paired"))
    );
}

struct Harness {
    tmp: TempDir,
    server: GatewayServer,
    runner: Arc<FakeCliRunner>,
    messenger: Arc<FakeMessengerTransport>,
    admin_token: String,
    project: String,
}

impl Harness {
    async fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let runner = Arc::new(FakeCliRunner::new());
        let messenger = Arc::new(FakeMessengerTransport::new());
        let server = GatewayServer::new(
            GatewayConfig::for_data_dir(tmp.path()),
            runner.clone(),
            messenger.clone(),
        )
        .await
        .unwrap();
        let (status, admin) = post(&server, "/api/auth/bootstrap", None, json!({
            "username": "admin", "email": "admin@example.com", "password": "correct-horse-battery"
        })).await;
        assert_eq!(status, StatusCode::CREATED);
        let admin_token = admin["access_token"].as_str().unwrap().to_owned();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let (status, project) = post(
            &server,
            "/api/projects",
            Some(&admin_token),
            json!({
                "name": "Messaging Project", "workspace_path": workspace
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        Self {
            tmp,
            server,
            runner,
            messenger,
            admin_token,
            project: project["id"].as_str().unwrap().to_owned(),
        }
    }

    async fn pair(&self, token: &str, mut event: IncomingMessengerEvent) {
        let (status, code) = post(
            &self.server,
            "/api/messenger/pairing-code",
            Some(token),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        event.text = Some(format!("/pair {}", code["code"].as_str().unwrap()));
        self.server.handle_messenger_event(event).await.unwrap();
    }

    async fn viewer(&self) -> String {
        let (status, invite) = post(
            &self.server,
            "/api/admin/invites",
            Some(&self.admin_token),
            json!({
                "role": "viewer", "project_ids": [&self.project]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, auth) = post(&self.server, "/api/auth/invites/redeem", None, json!({
            "code": invite["code"], "username": "reader", "email": "reader@example.com", "password": "long-enough-password"
        })).await;
        assert_eq!(status, StatusCode::CREATED);
        auth["access_token"].as_str().unwrap().to_owned()
    }

    async fn send(&self, event: IncomingMessengerEvent) {
        self.server.handle_messenger_event(event).await.unwrap();
    }
}

fn thread(
    platform: MessengerPlatform,
    user: &str,
    channel: &str,
    id: Option<&str>,
    text: &str,
) -> IncomingMessengerEvent {
    IncomingMessengerEvent {
        platform,
        chat_id: channel.into(),
        thread_id: id.map(str::to_owned),
        external_user_id: user.into(),
        is_private: false,
        text: Some(text.into()),
        callback_data: None,
    }
}

fn discord_dm(user: &str, text: &str) -> IncomingMessengerEvent {
    IncomingMessengerEvent {
        platform: MessengerPlatform::Discord,
        chat_id: format!("dm-{user}"),
        ..telegram(user, text)
    }
}

async fn until<T>(mut check: impl FnMut() -> Option<T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(result) = check() {
                return result;
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("observable messenger response")
}

#[tokio::test]
async fn topic_and_thread_assign_one_persistent_session_per_platform() {
    let mut h = Harness::new().await;
    h.pair(&h.admin_token, telegram("42", "")).await;
    h.pair(&h.admin_token, discord_dm("42", "")).await;
    let bind = format!("/bind {}", h.project);
    let telegram_group = |id, text| thread(MessengerPlatform::Telegram, "42", "public", id, text);
    let discord_group = |id, text| thread(MessengerPlatform::Discord, "42", "public", id, text);
    h.send(telegram_group(None, &bind)).await;
    h.send(discord_group(None, &bind)).await;
    let (first, second) = tokio::join!(
        h.server
            .handle_messenger_event(telegram_group(Some("topic1"), "First prompt")),
        h.server
            .handle_messenger_event(telegram_group(Some("topic1"), "Second prompt"))
    );
    first.unwrap();
    second.unwrap();
    h.send(discord_group(Some("topic1"), "Discord prompt"))
        .await;
    let path = format!("/api/projects/{}/sessions", h.project);
    let (status, before) = request(
        &h.server,
        Method::GET,
        &path,
        Some(&h.admin_token),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sessions = before.as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    let original = sessions
        .iter()
        .find(|session| session["title"] == "telegram thread topic1")
        .unwrap()["id"]
        .clone();
    until(|| {
        (h.runner.recorded_invocations().len() == 3
            && h.server.scheduler().status().active_turn_id.is_none())
        .then_some(())
    })
    .await;

    h.server = GatewayServer::new(
        GatewayConfig::for_data_dir(h.tmp.path()),
        h.runner.clone(),
        h.messenger.clone(),
    )
    .await
    .unwrap();
    h.send(telegram_group(Some("topic1"), "Third prompt after restart"))
        .await;
    let (_, after) = request(
        &h.server,
        Method::GET,
        &path,
        Some(&h.admin_token),
        Value::Null,
    )
    .await;
    assert_eq!(after.as_array().unwrap().len(), 2);
    let detail_path = format!("{path}/{}", original.as_str().unwrap());
    let (_, detail) = request(
        &h.server,
        Method::GET,
        &detail_path,
        Some(&h.admin_token),
        Value::Null,
    )
    .await;
    assert_eq!(detail["turns"].as_array().unwrap().len(), 3);
}

async fn buttons_for(h: &Harness, label: &str) -> (String, String, String) {
    until(|| {
        let sent = h.messenger.sent_messages();
        let edited = h.messenger.edited_messages();
        edited
            .iter()
            .rev()
            .find_map(|message| {
                message
                    .buttons
                    .iter()
                    .flatten()
                    .find(|b| b.label.contains(label))
                    .map(|b| {
                        (
                            message.message_id.clone(),
                            b.callback_data.clone(),
                            message.text.clone(),
                        )
                    })
            })
            .or_else(|| {
                sent.iter().rev().find_map(|message| {
                    message
                        .message
                        .buttons
                        .iter()
                        .flatten()
                        .find(|b| b.label.contains(label))
                        .map(|b| {
                            (
                                message.message_id.clone(),
                                b.callback_data.clone(),
                                message.message.text.clone(),
                            )
                        })
                })
            })
    })
    .await
}

fn callback(event: IncomingMessengerEvent, key: &str) -> IncomingMessengerEvent {
    IncomingMessengerEvent {
        text: None,
        callback_data: Some(key.into()),
        ..event
    }
}

#[tokio::test]
async fn interactive_status_coalesces_events_and_permission_actions_stay_scoped() {
    let h = Harness::new().await;
    h.pair(&h.admin_token, telegram("42", "")).await;
    h.pair(&h.viewer().await, telegram("99", "")).await;
    let choose = format!("/project {}", h.project);
    h.send(telegram("42", &choose)).await;
    h.send(telegram("99", &choose)).await;
    assert!(
        h.server
            .handle_messenger_event(telegram("99", "/new"))
            .await
            .is_err()
    );
    assert!(
        h.server
            .handle_messenger_event(telegram("99", "Run a prompt"))
            .await
            .is_err()
    );
    h.send(telegram("42", "/new interactive Permission fixture"))
        .await;

    let events = |request_id: &str| {
        vec![
            TurnEvent::ToolCallStart {
                id: "tool".into(),
                name: "Run checks".into(),
                input: json!({}),
            },
            TurnEvent::SubagentStart {
                id: "agent".into(),
                name: "Reviewer".into(),
            },
            TurnEvent::PermissionRequest {
                request_id: request_id.into(),
                tool_name: "Check files".into(),
                input: json!({}),
            },
            TurnEvent::TurnComplete {
                session_id: Some("local-session".into()),
                result_text: "**Final answer**".into(),
            },
        ]
    };
    h.runner.push_turn(events("request-one"));
    h.send(telegram("42", "First interactive input")).await;
    let (status_id, allow, text) = buttons_for(&h, "Allow").await;
    assert!(text.contains("Run checks"));
    assert!(text.contains("Reviewer"));
    assert!(
        h.server
            .handle_messenger_event(callback(telegram("99", ""), &allow))
            .await
            .is_err()
    );
    h.send(callback(telegram("42", ""), &allow)).await;
    until(|| {
        h.messenger
            .sent_messages()
            .iter()
            .any(|m| m.message.text == "**Final answer**")
            .then_some(())
    })
    .await;
    assert!(
        h.messenger
            .edited_messages()
            .iter()
            .all(|m| m.message_id == status_id)
    );
    assert!(h.messenger.edited_messages().len() <= 2);

    h.runner.push_turn(events("request-two"));
    h.send(telegram("42", "Second interactive input")).await;
    let (second_status_id, deny, _) = until(|| {
        let sent = h.messenger.sent_messages();
        let edited = h.messenger.edited_messages();
        let matching = |rows: &[Vec<claude_web_ui_server::traits::InlineButton>]| {
            rows.iter()
                .flatten()
                .find(|b| b.label.contains("Deny"))
                .map(|b| b.callback_data.clone())
        };
        edited
            .iter()
            .filter(|m| m.message_id != status_id)
            .find_map(|m| {
                matching(&m.buttons).map(|key| (m.message_id.clone(), key, m.text.clone()))
            })
            .or_else(|| {
                sent.iter()
                    .filter(|m| m.message_id != status_id)
                    .find_map(|m| {
                        matching(&m.message.buttons)
                            .map(|key| (m.message_id.clone(), key, m.message.text.clone()))
                    })
            })
    })
    .await;
    assert!(
        h.server
            .handle_messenger_event(callback(telegram("42", ""), &allow))
            .await
            .is_err()
    );
    h.send(callback(telegram("42", ""), &deny)).await;
    until(|| {
        h.messenger
            .sent_messages()
            .iter()
            .any(|m| m.message.text == "permission denied")
            .then_some(())
    })
    .await;
    assert_eq!(*h.runner.approval_decisions.lock().unwrap(), [true, false]);

    h.runner.push_turn(events("request-three"));
    h.send(telegram("42", "Third interactive input")).await;
    let stop = until(|| {
        h.messenger
            .sent_messages()
            .iter()
            .rev()
            .find(|m| {
                m.message
                    .buttons
                    .iter()
                    .flatten()
                    .any(|b| b.label.contains("Stop"))
            })
            .filter(|m| m.message_id != status_id && m.message_id != second_status_id)
            .and_then(|m| {
                m.message
                    .buttons
                    .iter()
                    .flatten()
                    .find(|b| b.label.contains("Stop"))
            })
            .map(|b| b.callback_data.clone())
            .filter(|_| {
                h.server
                    .scheduler()
                    .status()
                    .pending_permission
                    .as_ref()
                    .is_some_and(|p| p.request_id == "request-three")
            })
    })
    .await;
    h.send(callback(telegram("42", ""), &stop)).await;
    until(|| {
        h.messenger
            .edited_messages()
            .iter()
            .any(|m| m.text.starts_with("Stopped"))
            .then_some(())
    })
    .await;
}

#[tokio::test]
async fn kanban_transition_notifies_linked_dm_with_project_checked_quick_action() {
    let h = Harness::new().await;
    h.pair(&h.admin_token, discord_dm("42", "")).await;
    h.send(discord_dm("42", &format!("/project {}", h.project)))
        .await;
    h.send(discord_dm("42", "/notify on")).await;
    let path = format!("/api/projects/{}/kanban", h.project);
    let (status, card) = post(
        &h.server,
        &path,
        Some(&h.admin_token),
        json!({
            "title": "Review this work", "description": "Implemented a fixture"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let move_path = format!("{path}/{}/move", card["id"].as_str().unwrap());
    let (status, _) = post(
        &h.server,
        &move_path,
        Some(&h.admin_token),
        json!({
            "column": "review", "result_summary": "Fixture tests pass"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, done, text) = buttons_for(&h, "Mark done").await;
    assert!(text.contains("Review this work"));
    assert!(text.contains("Fixture tests pass"));
    assert!(
        h.server
            .handle_messenger_event(callback(telegram("42", ""), &done))
            .await
            .is_err()
    );
    h.send(callback(discord_dm("42", ""), &done)).await;
    let (_, board) = request(
        &h.server,
        Method::GET,
        &path,
        Some(&h.admin_token),
        Value::Null,
    )
    .await;
    assert_eq!(board[0]["column"], "done");
}
