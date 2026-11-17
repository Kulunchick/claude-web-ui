use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliProvider {
    ClaudeCode,
    Codex,
    Antigravity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CliInvocation {
    pub provider: CliProvider,
    pub program: String,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
    pub env: BTreeMap<String, String>,
}

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

#[async_trait]
pub trait CliRunner: Send + Sync {
    async fn run_turn(
        &self,
        invocation: CliInvocation,
        event_tx: mpsc::UnboundedSender<TurnEvent>,
        approval_rx: mpsc::UnboundedReceiver<bool>,
    ) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessengerPlatform {
    Telegram,
    Discord,
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
