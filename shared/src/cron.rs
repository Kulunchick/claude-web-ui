use crate::accounts::CliProvider;
use crate::chat::ChatTurnStatus;
use crate::messenger::MessengerPlatform;
pub use crate::messenger::MessengerPlatform as CronMessengerPlatform;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CronDestination {
    pub platform: MessengerPlatform,
    pub chat_id: String,
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobConfig {
    pub expression: String,
    pub prompt: String,
    pub provider: Option<CliProvider>,
    pub account_id: Option<String>,
    pub pinned_session_id: Option<String>,
    pub notification: Option<CronDestination>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobDto {
    pub id: String,
    pub owner_id: Option<String>,
    #[serde(flatten)]
    pub config: CronJobConfig,
    pub next_run_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronRunDto {
    pub id: String,
    pub job_id: String,
    pub trigger: CronTrigger,
    pub prompt: String,
    pub actor_id: Option<String>,
    pub status: ChatTurnStatus,
    pub queued_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub session_id: Option<String>,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub notification_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CronTrigger {
    Scheduled,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobsResponse {
    pub writable: bool,
    pub jobs: Vec<CronJobDto>,
    pub destinations: Vec<CronDestination>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetCronEnabledRequest {
    pub enabled: bool,
}
