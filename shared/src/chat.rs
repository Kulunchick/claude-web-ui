use crate::accounts::CliProvider;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnEvent {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolCallStart {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolCallResult {
        id: String,
        output: String,
        is_error: bool,
    },
    SubagentStart {
        id: String,
        name: String,
    },
    SubagentEnd {
        id: String,
    },
    PermissionRequest {
        request_id: String,
        tool_name: String,
        input: serde_json::Value,
    },
    TurnComplete {
        session_id: Option<String>,
        result_text: String,
    },
    TurnError {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    #[default]
    FullAuto,
    Interactive,
}

impl ApprovalMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalMode::FullAuto => "full_auto",
            ApprovalMode::Interactive => "interactive",
        }
    }
}

impl fmt::Display for ApprovalMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ApprovalMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "full_auto" | "fullauto" | "auto" => Ok(ApprovalMode::FullAuto),
            "interactive" | "manual" => Ok(ApprovalMode::Interactive),
            other => Err(format!("unknown approval mode: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatTurnStatus {
    Queued,
    Running,
    WaitingApproval,
    Completed,
    Error,
    Cancelled,
}

impl ChatTurnStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChatTurnStatus::Queued => "queued",
            ChatTurnStatus::Running => "running",
            ChatTurnStatus::WaitingApproval => "waiting_approval",
            ChatTurnStatus::Completed => "completed",
            ChatTurnStatus::Error => "error",
            ChatTurnStatus::Cancelled => "cancelled",
        }
    }
}

impl FromStr for ChatTurnStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "queued" => Ok(ChatTurnStatus::Queued),
            "running" => Ok(ChatTurnStatus::Running),
            "waiting_approval" => Ok(ChatTurnStatus::WaitingApproval),
            "completed" => Ok(ChatTurnStatus::Completed),
            "error" => Ok(ChatTurnStatus::Error),
            "cancelled" => Ok(ChatTurnStatus::Cancelled),
            other => Err(format!("unknown turn status: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingPermissionDto {
    pub request_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatSessionDto {
    pub id: String,
    pub project_id: String,
    pub provider: CliProvider,
    pub native_session_id: Option<String>,
    pub title: String,
    pub approval_mode: ApprovalMode,
    pub source: String,
    pub transcript_path: Option<String>,
    pub message_count: usize,
    pub last_message_preview: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatTurnDto {
    pub id: String,
    pub session_id: String,
    pub project_id: String,
    pub user_id: String,
    pub provider: CliProvider,
    pub account_id: Option<String>,
    pub prompt: String,
    pub status: ChatTurnStatus,
    pub result_text: Option<String>,
    pub error_message: Option<String>,
    pub pending_permission: Option<PendingPermissionDto>,
    pub events: Vec<TurnEvent>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeTranscriptEntryDto {
    pub role: String,
    pub text: String,
    pub tool_name: Option<String>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatSessionDetailDto {
    pub session: ChatSessionDto,
    pub turns: Vec<ChatTurnDto>,
    pub native_messages: Vec<NativeTranscriptEntryDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreateChatSessionRequest {
    pub title: Option<String>,
    pub provider: Option<CliProvider>,
    pub native_session_id: Option<String>,
    pub approval_mode: Option<ApprovalMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateChatSessionRequest {
    pub title: Option<String>,
    pub approval_mode: Option<ApprovalMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendChatTurnRequest {
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SendChatTurnResponse {
    pub turn: ChatTurnDto,
    pub queue_position: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvePermissionRequest {
    pub approved: bool,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CancelTurnResponse {
    pub turn_id: String,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchedulerStatusDto {
    pub active_turn_id: Option<String>,
    pub active_session_id: Option<String>,
    pub active_project_id: Option<String>,
    pub queued_count: usize,
    pub pending_permission: Option<PendingPermissionDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatStreamEnvelope {
    pub seq: u64,
    pub project_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub status: ChatTurnStatus,
    pub event: TurnEvent,
    pub timestamp: String,
}
