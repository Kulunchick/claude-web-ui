use claude_web_ui_shared::chat::{
    ChatStreamEnvelope, ChatTurnStatus, SchedulerStatusDto, TurnEvent,
};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Approval { tool: String },
    TurnCompleted,
    CardReview { title: String },
    CardBlocked { title: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopTarget {
    pub project_id: String,
    pub session_id: String,
    pub turn_id: String,
}

#[derive(Debug, Default)]
pub struct TrayState {
    reachable: bool,
    streams: HashMap<String, Option<u64>>,
    turn: Option<(StopTarget, bool)>,
    last_approval: Option<(String, String)>,
}

#[derive(Deserialize)]
struct InitialStatus {
    #[serde(rename = "type")]
    kind: String,
    status: SchedulerStatusDto,
}

impl TrayState {
    pub fn set_reachable(&mut self, reachable: bool) {
        self.reachable = reachable;
    }

    pub fn connect(&mut self, project: &str) {
        self.streams.insert(project.to_owned(), None);
    }

    pub fn disconnect(&mut self, project: &str) {
        self.streams.remove(project);
        self.clear_turn(project);
    }

    pub fn label(&self) -> &'static str {
        match &self.turn {
            Some((_, true)) => "Connected \u{00b7} Waiting for approval",
            Some((_, false)) => "Connected \u{00b7} Running",
            None if self.reachable || !self.streams.is_empty() => "Connected \u{00b7} Idle",
            None => "Disconnected",
        }
    }

    pub fn stop_target(&self) -> Option<&StopTarget> {
        self.turn.as_ref().map(|(target, _)| target)
    }

    pub fn on_frame(&mut self, project: &str, frame: &str) -> Option<Notice> {
        let last_seq = self.streams.get_mut(project)?;
        if let Ok(initial) = serde_json::from_str::<InitialStatus>(frame) {
            if initial.kind != "scheduler_status" {
                return None;
            }
            return self.on_scheduler_status(project, initial.status);
        }
        let message = serde_json::from_str::<ChatStreamEnvelope>(frame).ok()?;
        if message.project_id != project || last_seq.is_some_and(|seq| seq >= message.seq) {
            return None;
        }
        *last_seq = Some(message.seq);
        let target = StopTarget {
            project_id: message.project_id,
            session_id: message.session_id,
            turn_id: message.turn_id,
        };
        match message.status {
            ChatTurnStatus::WaitingApproval => self.turn = Some((target.clone(), true)),
            ChatTurnStatus::Running => self.turn = Some((target.clone(), false)),
            ChatTurnStatus::Completed | ChatTurnStatus::Error | ChatTurnStatus::Cancelled => {
                if self.stop_target() == Some(&target) {
                    self.turn = None;
                }
            }
            ChatTurnStatus::Queued => {}
        }
        match (message.status, message.event) {
            (
                ChatTurnStatus::WaitingApproval,
                TurnEvent::PermissionRequest {
                    tool_name,
                    request_id,
                    ..
                },
            ) => self.approval_notice(target.turn_id, request_id, tool_name),
            (ChatTurnStatus::Completed, TurnEvent::TurnComplete { .. }) => {
                Some(Notice::TurnCompleted)
            }
            _ => None,
        }
    }

    pub fn on_scheduler_status(
        &mut self,
        project: &str,
        status: SchedulerStatusDto,
    ) -> Option<Notice> {
        if !self.streams.contains_key(project) {
            return None;
        }
        if status.active_project_id.as_deref() == Some(project)
            && let (Some(session_id), Some(turn_id)) =
                (status.active_session_id, status.active_turn_id)
        {
            let target = StopTarget {
                project_id: project.to_owned(),
                session_id,
                turn_id,
            };
            self.turn = Some((target.clone(), status.pending_permission.is_some()));
            return status.pending_permission.and_then(|request| {
                self.approval_notice(target.turn_id, request.request_id, request.tool_name)
            });
        }
        self.clear_turn(project);
        None
    }

    fn approval_notice(
        &mut self,
        turn_id: String,
        request_id: String,
        tool: String,
    ) -> Option<Notice> {
        let key = (turn_id, request_id);
        if self.last_approval.as_ref() == Some(&key) {
            return None;
        }
        self.last_approval = Some(key);
        Some(Notice::Approval { tool })
    }

    fn clear_turn(&mut self, project: &str) {
        if self
            .stop_target()
            .is_some_and(|target| target.project_id == project)
        {
            self.turn = None;
        }
    }
}
