use super::{Action, Destination, channel_project, kanban_action, messenger_user};
use crate::{
    AppState,
    auth::{self, AppError},
    chat::load_chat_turn,
    traits::{InlineButton, MessengerPlatform},
};
use claude_web_ui_shared::{
    chat::{ChatStreamEnvelope, ChatTurnStatus, TurnEvent},
    kanban::{KanbanCardDto, KanbanColumn},
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use tokio::sync::broadcast;

const EDIT_INTERVAL: Duration = Duration::from_millis(1500);
const TURN_BUTTON_TTL: Duration = Duration::from_secs(60 * 60 * 24);

#[derive(Default)]
struct Activity {
    tools: BTreeMap<String, String>,
    subagents: BTreeMap<String, String>,
}

impl Activity {
    fn record(&mut self, event: TurnEvent) {
        match event {
            TurnEvent::ToolCallStart { id, name, .. } => {
                self.tools.insert(id, name);
            }
            TurnEvent::ToolCallResult { id, .. } => {
                self.tools.remove(&id);
            }
            TurnEvent::SubagentStart { id, name } => {
                self.subagents.insert(id, name);
            }
            TurnEvent::SubagentEnd { id } => {
                self.subagents.remove(&id);
            }
            _ => {}
        }
    }

    fn status(&self, elapsed: u64, status: ChatTurnStatus) -> String {
        let mut text = match status {
            ChatTurnStatus::Queued => format!("Queued · {elapsed}s"),
            ChatTurnStatus::Running => format!("Running · {elapsed}s"),
            ChatTurnStatus::WaitingApproval => format!("Waiting for approval · {elapsed}s"),
            ChatTurnStatus::Completed => format!("Completed · {elapsed}s"),
            ChatTurnStatus::Error => format!("Failed · {elapsed}s"),
            ChatTurnStatus::Cancelled => format!("Stopped · {elapsed}s"),
        };
        if matches!(
            status,
            ChatTurnStatus::Running | ChatTurnStatus::WaitingApproval
        ) {
            for (label, items) in [("Tools", &self.tools), ("Subagents", &self.subagents)] {
                if !items.is_empty() {
                    let names = items
                        .values()
                        .take(8)
                        .map(|value| value.chars().take(60).collect::<String>())
                        .collect::<Vec<_>>()
                        .join(", ");
                    text.push_str(&format!("\n{label}: {names}"));
                }
            }
        }
        text
    }
}

pub(super) async fn follow(
    state: AppState,
    dest: Destination,
    project: String,
    turn_id: String,
    external_user: String,
    mut events: broadcast::Receiver<ChatStreamEnvelope>,
) -> Result<(), AppError> {
    let started = Instant::now();
    let mut activity = Activity::default();
    let mut interval = tokio::time::interval(EDIT_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let stop = state.messenger_callbacks.button(
        &dest,
        &project,
        "⏹ Stop",
        Action::Stop(turn_id.clone()),
        TURN_BUTTON_TTL,
    );
    let mut message_id = None;
    let mut previous = None;
    let mut permission_buttons: Option<(String, Vec<InlineButton>)> = None;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                for _ in 0..256 {
                    match events.try_recv() {
                        Ok(envelope) if envelope.turn_id == turn_id => activity.record(envelope.event),
                        Ok(_) => {}
                        Err(broadcast::error::TryRecvError::Lagged(_)) => activity = Activity::default(),
                        Err(_) => break,
                    }
                }
                let actor = messenger_user(&state, dest.platform, &external_user).await?;
                let turn = load_chat_turn(&state.db, &project, &turn_id).await?;
                if actor.user.id != turn.user_id {
                    return Err(AppError::forbidden("messenger identity changed"));
                }
                auth::authorize_project(&state, actor, &project, false).await?;
                if dest.private_user.is_none()
                    && channel_project(&mut *state.db.acquire().await?, &dest).await?.as_deref() != Some(&project)
                {
                    return Err(AppError::forbidden("channel project changed"));
                }
                let status = turn.status;
                let terminal = matches!(status, ChatTurnStatus::Completed | ChatTurnStatus::Error | ChatTurnStatus::Cancelled);
                let mut text = activity.status(started.elapsed().as_secs(), status);
                let mut buttons = Vec::new();
                if !terminal {
                    buttons.push(vec![stop.clone()]);
                }
                if let Some(permission) = turn.pending_permission.filter(|_| status == ChatTurnStatus::WaitingApproval) {
                    let details: String = permission.input.to_string().chars().take(500).collect();
                    text.push_str(&format!("\nAllow {}?\n{details}", permission.tool_name));

                    if permission_buttons.as_ref().is_none_or(|(id, _)| *id != permission.request_id) {
                        let row = [("✅ Allow", true), ("❌ Deny", false)].into_iter().map(|(label, allow)| {
                            state.messenger_callbacks.button(&dest, &project, label, Action::Approval {
                                turn: turn_id.clone(), request: permission.request_id.clone(), allow,
                            }, TURN_BUTTON_TTL)
                        }).collect();
                        permission_buttons = Some((permission.request_id, row));
                    }
                    if let Some((_, row)) = &permission_buttons {
                        buttons.push(row.clone());
                    }
                }
                let snapshot = (text, buttons);
                if previous.as_ref() != Some(&snapshot) {
                    let (text, buttons) = &snapshot;
                    let result = if let Some(id) = &message_id {
                        let edit_chat = match dest.platform {
                            MessengerPlatform::Telegram => dest.chat.clone(),
                            MessengerPlatform::Discord => dest.thread.clone().unwrap_or_else(|| dest.chat.clone()),
                        };
                        state.messenger.edit_message(dest.platform, edit_chat, String::clone(id), text.clone(), buttons.clone())
                            .await.map_err(AppError::internal)
                    } else {
                        dest.send(&state, text.clone(), buttons.clone()).await.map(|id| {
                            message_id = Some(id);
                        })
                    };
                    match result {
                        Ok(()) => previous = Some(snapshot),
                        Err(error) => tracing::warn!(code = error.code, "messenger status message failed"),
                    }
                }
                if terminal {
                    let answer = match status {
                        ChatTurnStatus::Completed => turn.result_text.filter(|text| !text.trim().is_empty()),
                        ChatTurnStatus::Error => turn.error_message,
                        _ => None,
                    };
                    if let Some(answer) = answer {
                        dest.reply(&state, answer).await?;
                    }
                    return Ok(());
                }
            }
            event = events.recv() => {
                match event {
                    Ok(envelope) if envelope.turn_id == turn_id => activity.record(envelope.event),
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        activity = Activity::default();
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
        }
    }
}

pub(crate) fn start_notifications(state: AppState) -> tokio::task::JoinHandle<()> {
    let mut events = state.kanban_events.subscribe();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(card) if matches!(card.column, KanbanColumn::Review | KanbanColumn::Blocked) => {
                    if let Err(error) = notify(&state, &card).await {
                        tracing::warn!(code = error.code, "could not route Kanban notification");
                    }
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

async fn notify(state: &AppState, card: &KanbanCardDto) -> Result<(), AppError> {
    let text = format!(
        "Kanban · {}\n{}\n{}",
        card.column.as_str(),
        card.title,
        card.blocked_reason
            .as_deref()
            .or(card.result_summary.as_deref())
            .unwrap_or(&card.description)
    );
    let Some(next) = kanban_action(card.column) else {
        return Ok(());
    };
    let destinations: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT platform, chat_id, NULL FROM messenger_channels
         WHERE project_id = ? AND notify = 1
         UNION ALL
         SELECT n.platform, l.dm_chat_id, n.external_user_id
         FROM messenger_dm_notifications n JOIN user_messenger_links l
           ON l.platform = n.platform AND l.external_user_id = n.external_user_id
         WHERE n.project_id = ?",
    )
    .bind(&card.project_id)
    .bind(&card.project_id)
    .fetch_all(&state.db)
    .await?;
    for (platform, chat, private_user) in destinations {
        let Some(platform) = MessengerPlatform::from_storage(&platform) else {
            continue;
        };
        let dest = Destination {
            platform,
            chat,
            thread: None,
            private_user,
        };
        let can_move = if let Some(external) = &dest.private_user {
            let Ok(actor) = messenger_user(state, platform, external).await else {
                continue;
            };
            let Ok(ctx) = auth::authorize_project(state, actor, &card.project_id, false).await
            else {
                continue;
            };
            ctx.effective_role != claude_web_ui_shared::auth::Role::Viewer
        } else {
            true
        };
        let buttons = if can_move {
            vec![vec![state.messenger_callbacks.button(
                &dest,
                &card.project_id,
                next.0,
                Action::CardMove {
                    card: card.id.clone(),
                    from: card.column,
                    to: next.1,
                },
                TURN_BUTTON_TTL,
            )]]
        } else {
            Vec::new()
        };
        if let Err(error) = dest.send(state, &text, buttons).await {
            tracing::warn!(code = error.code, "Kanban notification delivery failed");
        }
    }
    Ok(())
}
