use crate::{accounts::CliProvider, chat::ChatTurnStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KanbanColumn {
    #[default]
    Triage,
    Ready,
    Running,
    Blocked,
    Review,
    Done,
    Archived,
}

impl KanbanColumn {
    pub const ALL: [Self; 7] = [
        Self::Triage,
        Self::Ready,
        Self::Running,
        Self::Blocked,
        Self::Review,
        Self::Done,
        Self::Archived,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Triage => "triage",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Review => "review",
            Self::Done => "done",
            Self::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanCardDto {
    pub id: String,
    pub project_id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub description: String,
    pub column: KanbanColumn,
    pub position: i64,
    pub depends_on: Vec<String>,
    pub unresolved_blockers: Vec<String>,
    pub assigned_cli: Option<CliProvider>,
    pub create_git_branch: bool,
    pub git_branch_name: Option<String>,
    pub session_id: Option<String>,
    pub session_status: Option<ChatTurnStatus>,
    pub blocked_reason: Option<String>,
    pub result_summary: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateKanbanCardRequest {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub column: KanbanColumn,
    pub parent_id: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub create_git_branch: bool,
    pub git_branch_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateKanbanCardRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub depends_on: Option<Vec<String>>,
    pub create_git_branch: Option<bool>,
    pub git_branch_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveKanbanCardRequest {
    pub column: KanbanColumn,
    pub position: Option<i64>,
    pub blocked_reason: Option<String>,
    pub result_summary: Option<String>,
}
