use super::{Column, create_card, list_cards, move_inner, update_card};
use crate::{AppState, auth::AppError};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use claude_web_ui_shared::kanban::{MoveKanbanCardRequest, UpdateKanbanCardRequest};
use serde_json::{Value, json};
use sqlx::Row;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub(super) async fn request(
    State(state): State<AppState>,
    Path(project): Path<String>,
    headers: HeaderMap,
    Json(rpc): Json<Value>,
) -> Result<Response, AppError> {
    let token = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| AppError::unauthorized("missing run token"))?;
    let run = sqlx::query(
        "SELECT c.run_user_id, t.account_id FROM kanban_cards c JOIN chat_turns t ON t.id = c.turn_id
         WHERE c.project_id = ? AND c.run_token = ? AND c.active_run = 1
         AND t.status IN ('running','waiting_approval')",
    )
    .bind(&project)
    .bind(token)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::unauthorized("invalid or expired run token"))?;
    let user: String = run.try_get("run_user_id")?;
    let account: Option<String> = run.try_get("account_id")?;
    super::worker::authorize(&state.db, &project, &user, account.as_deref()).await?;
    if rpc.get("id").is_none() {
        return Ok(StatusCode::ACCEPTED.into_response());
    }
    let result = match rpc["method"].as_str() {
        Some("initialize") => json!({"result": {
            "protocolVersion":"2024-11-05", "capabilities":{"tools":{}},
            "serverInfo":{"name":"gateway-kanban","version":env!("CARGO_PKG_VERSION")}
        }}),
        Some("ping") => json!({"result":{}}),
        Some("tools/list") => json!({"result":{"tools":tools()}}),
        Some("tools/call") => match call_tool(&state, &project, &user, &rpc["params"]).await {
            Ok(value) => json!({"result":{"content":[{"type":"text","text":value.to_string()}]}}),
            Err(error) => {
                json!({"result":{"isError":true,"content":[{"type":"text","text":error.message}]}})
            }
        },
        _ => json!({"error":{"code":-32601,"message":"Method not found"}}),
    };
    let mut response = json!({"jsonrpc":"2.0", "id":rpc["id"]});
    response
        .as_object_mut()
        .unwrap()
        .extend(result.as_object().unwrap().clone());
    Ok(Json(response).into_response())
}

fn tools() -> Value {
    let string = json!({"type":"string"});
    let dependencies = json!({"type":"array","items":{"type":"string"}});
    let columns: Vec<_> = Column::ALL.into_iter().map(Column::as_str).collect();
    let column = json!({"type":"string","enum":columns});
    let tool = |name: &str, description: &str, properties: Value, required: &[&str]| {
        json!({"name":name,"description":description,
               "inputSchema":{"type":"object","properties":properties,"required":required}})
    };
    json!([
        tool(
            "kanban_show",
            "List all cards in the current project",
            json!({}),
            &[]
        ),
        tool(
            "kanban_create",
            "Create a project card, optionally as a child or with dependencies",
            json!({
                "title":string,"description":string,"column":column,"parent_id":string,"depends_on":dependencies,
                "create_git_branch":{"type":"boolean"},"git_branch_name":string
            }),
            &["title"]
        ),
        tool(
            "kanban_move",
            "Move or reorder a card",
            json!({
                "card_id":string,"column":column,"position":{"type":"integer","minimum":0}
            }),
            &["card_id", "column"]
        ),
        tool(
            "kanban_update",
            "Edit a card",
            json!({
                "card_id":string,"title":string,"description":string,"depends_on":dependencies,
                "create_git_branch":{"type":"boolean"},"git_branch_name":string
            }),
            &["card_id"]
        ),
        tool(
            "kanban_complete",
            "Move the card to Review with a result summary",
            json!({
                "card_id":string,"result_summary":string
            }),
            &["card_id", "result_summary"]
        ),
        tool(
            "kanban_block",
            "Move a card to Blocked and give the reason",
            json!({
                "card_id":string,"blocked_reason":string
            }),
            &["card_id", "blocked_reason"]
        )
    ])
}

async fn call_tool(
    state: &AppState,
    project: &str,
    user: &str,
    params: &Value,
) -> Result<Value, AppError> {
    fn parse<T: serde::de::DeserializeOwned>(data: &Value) -> Result<T, AppError> {
        serde_json::from_value(data.clone())
            .map_err(|error| AppError::bad_request(error.to_string()))
    }
    let args = &params["arguments"];
    let property = |name: &str| {
        args[name]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AppError::bad_request(format!("{name} is required")))
    };
    let card_id = || property("card_id");
    let card = match params["name"].as_str() {
        Some("kanban_show") => return Ok(json!(list_cards(&state.db, project).await?)),
        Some("kanban_create") => create_card(state, project, user, parse(args)?).await?,
        Some("kanban_move") => move_inner(state, project, card_id()?, user, parse(args)?).await?,
        Some("kanban_update") => {
            let req: UpdateKanbanCardRequest = parse(args)?;
            update_card(state, project, card_id()?, req).await?
        }
        Some("kanban_complete") => {
            let req = MoveKanbanCardRequest {
                column: Column::Review,
                position: None,
                blocked_reason: None,
                result_summary: Some(property("result_summary")?.to_owned()),
            };
            move_inner(state, project, card_id()?, user, req).await?
        }
        Some("kanban_block") => {
            let req = MoveKanbanCardRequest {
                column: Column::Blocked,
                position: None,
                result_summary: None,
                blocked_reason: Some(property("blocked_reason")?.to_owned()),
            };
            move_inner(state, project, card_id()?, user, req).await?
        }
        _ => return Err(AppError::bad_request("unknown Kanban tool")),
    };
    Ok(json!(card))
}

pub async fn stdio_bridge(
    args: impl IntoIterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = None;
    let mut server = None;
    let mut token = std::env::var("CLAUDE_WEB_UI_KANBAN_RUN_TOKEN").ok();
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--project-id" => project = args.next(),
            "--server-url" => server = args.next(),
            "--token" => token = args.next(),
            _ => return Err(format!("unknown option {flag}").into()),
        }
    }
    let project = project.ok_or("--project-id is required")?;
    if Uuid::parse_str(&project).is_err() {
        return Err("invalid project id".into());
    }
    let url: hyper::Uri = server.ok_or("--server-url is required")?.parse()?;
    let token = token.ok_or("a run token is required")?;
    if url.scheme_str() != Some("http")
        || url
            .host()
            .is_none_or(|host| !matches!(host, "localhost" | "127.0.0.1" | "[::1]"))
    {
        return Err("the Kanban stdio bridge requires a loopback HTTP server".into());
    }

    use axum::body::Body;
    use http_body_util::BodyExt;
    use hyper_util::rt::TokioIo;
    use tokio::net::TcpStream;
    use uuid::Uuid;

    let mut input = BufReader::new(tokio::io::stdin()).lines();
    let mut output = tokio::io::stdout();
    let host = url.host().unwrap().trim_matches(['[', ']']);
    let port = url.port_u16().unwrap_or(80);
    while let Some(line) = input.next_line().await? {
        let rpc: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => {
                output.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32700,\"message\":\"Parse error\"}}\n").await?;
                output.flush().await?;
                continue;
            }
        };
        let stream = TcpStream::connect((host, port)).await?;
        let (mut client, conn) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let response = client
            .send_request(
                hyper::Request::post(format!("/api/mcp/kanban/{project}"))
                    .header("host", url.authority().unwrap().as_str())
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(line))?,
            )
            .await?;
        let status = response.status();
        let bytes = response.into_body().collect().await?.to_bytes();
        if rpc.get("id").is_some() {
            let result = if status.is_success() {
                serde_json::from_slice::<Value>(&bytes)?
            } else {
                json!({"jsonrpc":"2.0","id":rpc["id"],"error":{"code":-32000,"message":format!("Kanban gateway returned {status}")}})
            };
            output.write_all(format!("{result}\n").as_bytes()).await?;
            output.flush().await?;
        }
    }
    Ok(())
}
