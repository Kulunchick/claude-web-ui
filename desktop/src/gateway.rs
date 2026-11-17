use crate::desktop_app::{Credentials, replace_credentials, update_tray};
use claude_web_ui_desktop::{GatewayProfile, Notice};
use claude_web_ui_shared::auth::{AuthTokensResponse, MeResponse, RefreshTokenRequest, Role};
use claude_web_ui_shared::chat::SchedulerStatusDto;
use futures_util::{SinkExt, StreamExt};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::{collections::HashMap, time::Duration};
use tauri::AppHandle;
use tokio::task::JoinSet;
use tokio::time::{interval, sleep, timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

const SYNC_INTERVAL: Duration = Duration::from_secs(15);
const RETRY_DELAY: Duration = Duration::from_secs(5);

pub async fn monitor(
    app: AppHandle,
    generation: u64,
    client: Client,
    profile: GatewayProfile,
    mut credentials: Option<Credentials>,
) {
    let mut projects = HashMap::<String, tokio::task::AbortHandle>::new();
    let mut watchers = JoinSet::new();
    let mut sync = interval(SYNC_INTERVAL);
    loop {
        sync.tick().await;
        let Some(current) = credentials.as_mut() else {
            let reachable = client
                .get(profile.endpoint(&["api", "auth", "status"]))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success());
            update_tray(&app, generation, |tray, _| {
                tray.set_reachable(reachable);
                None
            });
            continue;
        };
        let me_endpoint = profile.endpoint(&["api", "auth", "me"]);
        let response = client
            .get(me_endpoint)
            .bearer_auth(&current.access_token)
            .send()
            .await;
        if response
            .as_ref()
            .is_ok_and(|response| response.status() == StatusCode::UNAUTHORIZED)
            && let Some(refresh_token) = current.refresh_token.as_ref()
            && let Ok(tokens) = refresh(&client, &profile, refresh_token).await
        {
            let new = Credentials {
                access_token: tokens.access_token,
                refresh_token: Some(tokens.refresh_token),
            };
            if let Err(error) = replace_credentials(&app, generation, &profile, new.clone()) {
                eprintln!("Could not save refreshed Gateway credentials: {error}");
            }
            *current = new;
            for (id, task) in projects.drain() {
                task.abort();
                update_tray(&app, generation, |tray, _| {
                    tray.disconnect(&id);
                    None
                });
            }
            sync.reset_immediately();
            continue;
        }
        let me = match response {
            Ok(response) if response.status().is_success() => {
                response.json::<MeResponse>().await.ok()
            }
            _ => None,
        };
        let reachable = me.is_some();
        let can_stop = me.as_ref().is_some_and(|me| me.user.role != Role::Viewer);
        update_tray(&app, generation, |tray, stop| {
            tray.set_reachable(reachable);
            *stop = can_stop;
            None
        });
        let desired = me.map_or_else(Vec::new, |me| me.projects);
        projects.retain(|id, task| {
            if desired.iter().any(|project| &project.id == id) {
                return true;
            }
            task.abort();
            update_tray(&app, generation, |tray, _| {
                tray.disconnect(id);
                None
            });
            false
        });
        while watchers.try_join_next().is_some() {}
        for project in desired {
            projects.entry(project.id.clone()).or_insert_with(|| {
                watchers.spawn(watch_project(
                    app.clone(),
                    generation,
                    client.clone(),
                    profile.clone(),
                    project.id,
                    current.access_token.clone(),
                ))
            });
        }
    }
}

async fn refresh(
    client: &Client,
    profile: &GatewayProfile,
    refresh_token: &str,
) -> Result<AuthTokensResponse, reqwest::Error> {
    client
        .post(profile.endpoint(&["api", "auth", "refresh"]))
        .json(&RefreshTokenRequest {
            refresh_token: refresh_token.into(),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
}

#[derive(Deserialize)]
struct Card {
    id: String,
    title: String,
    column: String,
}

async fn poll_board(
    app: &AppHandle,
    generation: u64,
    client: &Client,
    profile: &GatewayProfile,
    project: &str,
    token: &str,
    previous: &mut Option<HashMap<String, String>>,
) {
    let endpoint = profile.endpoint(&["api", "projects", project, "kanban"]);
    let Ok(response) = client.get(endpoint).bearer_auth(token).send().await else {
        return;
    };
    let Ok(cards) = response.json::<Vec<Card>>().await else {
        return;
    };
    let mut next = HashMap::new();
    for Card { id, title, column } in cards {
        if let Some(snapshot) = previous.as_ref()
            && snapshot.get(&id).is_none_or(|old| old != &column)
        {
            let notice = match column.as_str() {
                "review" => Some(Notice::CardReview { title }),
                "blocked" => Some(Notice::CardBlocked { title }),
                _ => None,
            };
            if notice.is_some() {
                update_tray(app, generation, |_, _| notice);
            }
        }
        next.insert(id, column);
    }
    *previous = Some(next);
}

async fn watch_project(
    app: AppHandle,
    generation: u64,
    client: Client,
    profile: GatewayProfile,
    project: String,
    token: String,
) {
    let mut board = None;
    let mut board_timer = interval(SYNC_INTERVAL);
    let mut scheduler_timer = interval(RETRY_DELAY);
    let scheduler = profile.endpoint(&["api", "projects", &project, "scheduler"]);
    loop {
        let mut request = profile
            .project_stream(&project)
            .as_str()
            .into_client_request()
            .expect("Gateway WebSocket URL");
        let Ok(bearer) = format!("Bearer {token}").parse() else {
            return;
        };
        request.headers_mut().insert("authorization", bearer);
        let Ok(Ok((mut socket, _))) = timeout(SYNC_INTERVAL, connect_async(request)).await else {
            poll_board(
                &app, generation, &client, &profile, &project, &token, &mut board,
            )
            .await;
            sleep(RETRY_DELAY).await;
            continue;
        };
        update_tray(&app, generation, |tray, _| {
            tray.connect(&project);
            None
        });
        let mut heartbeat = interval(SYNC_INTERVAL);
        let mut heard_server = true;
        loop {
            tokio::select! {
                message = socket.next() => match message {
                    Some(Ok(Message::Text(frame))) => {
                        heard_server = true;
                        update_tray(&app, generation, |tray, _| tray.on_frame(&project, &frame));
                    }
                    Some(Ok(Message::Pong(_))) => heard_server = true,
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    _ => {}
                },
                _ = heartbeat.tick() => {
                    if !heard_server || socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                    heard_server = false;
                }
                _ = scheduler_timer.tick() => {
                    if let Ok(response) = client.get(scheduler.clone()).bearer_auth(&token).send().await
                        && response.status().is_success()
                        && let Ok(status) = response.json::<SchedulerStatusDto>().await
                    {
                        update_tray(&app, generation, |tray, _| tray.on_scheduler_status(&project, status));
                    }
                }
                _ = board_timer.tick() => {
                    poll_board(&app, generation, &client, &profile, &project, &token, &mut board).await;
                }
            }
        }
        update_tray(&app, generation, |tray, _| {
            tray.disconnect(&project);
            None
        });
        sleep(RETRY_DELAY).await;
    }
}
