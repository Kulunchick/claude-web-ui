use async_trait::async_trait;
pub use claude_web_ui_shared::accounts::CliProvider;
pub use claude_web_ui_shared::chat::TurnEvent;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CliInvocation {
    pub provider: CliProvider,
    pub program: String,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PtyInvocation {
    pub provider: CliProvider,
    pub program: String,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
    pub env: BTreeMap<String, String>,
    pub cols: u16,
    pub rows: u16,
}

#[async_trait]
pub trait PtySession: Send + Sync {
    async fn write_input(&self, data: &[u8]) -> Result<(), String>;
    async fn resize(&self, cols: u16, rows: u16) -> Result<(), String>;
    async fn close(&self) -> Result<(), String>;
}

pub struct SpawnedPty {
    pub session: Box<dyn PtySession>,
    pub output_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    pub exit_rx: oneshot::Receiver<i32>,
}

#[async_trait]
pub trait CliRunner: Send + Sync {
    async fn run_turn(
        &self,
        invocation: CliInvocation,
        event_tx: mpsc::UnboundedSender<TurnEvent>,
        approval_rx: mpsc::UnboundedReceiver<bool>,
    ) -> Result<(), String>;

    async fn spawn_pty(&self, invocation: PtyInvocation) -> Result<SpawnedPty, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessengerPlatform {
    Telegram,
    Discord,
}

impl MessengerPlatform {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Discord => "discord",
        }
    }

    pub(crate) fn from_storage(value: &str) -> Option<Self> {
        match value {
            "telegram" => Some(Self::Telegram),
            "discord" => Some(Self::Discord),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InlineButton {
    pub label: String,
    pub callback_data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutboundMessage {
    pub platform: MessengerPlatform,
    pub chat_id: String,
    pub thread_id: Option<String>,
    pub text: String,
    pub buttons: Vec<Vec<InlineButton>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IncomingMessengerEvent {
    pub platform: MessengerPlatform,
    pub chat_id: String,
    pub thread_id: Option<String>,
    pub external_user_id: String,
    pub is_private: bool,
    pub text: Option<String>,
    pub callback_data: Option<String>,
}

#[async_trait]
pub trait MessengerTransport: Send + Sync {
    async fn send_message(&self, message: OutboundMessage) -> Result<String, String>;
    async fn edit_message(
        &self,
        platform: MessengerPlatform,
        chat_id: String,
        message_id: String,
        text: String,
        buttons: Vec<Vec<InlineButton>>,
    ) -> Result<(), String>;
}
