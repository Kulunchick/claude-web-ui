use crate::accounts::CliProvider;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigScope {
    Global,
    Project,
}

impl ConfigScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConfigScope::Global => "global",
            ConfigScope::Project => "project",
        }
    }
}

pub fn provider_project_dir_name(provider: CliProvider) -> &'static str {
    match provider {
        CliProvider::ClaudeCode => ".claude",
        CliProvider::Codex => ".codex",
        CliProvider::Antigravity => ".agents",
    }
}

pub fn provider_model_tiers(provider: CliProvider) -> &'static [&'static str] {
    match provider {
        CliProvider::ClaudeCode => &["opus", "sonnet", "haiku", "inherit"],
        CliProvider::Codex => &["gpt-5-codex", "o3", "o4-mini", "codex-mini", "inherit"],
        CliProvider::Antigravity => &["pro", "flash", "flash_lite", "inherit"],
    }
}

pub const PERMISSION_MODES: &[&str] = &[
    "default",
    "acceptEdits",
    "bypassPermissions",
    "plan",
    "dontAsk",
];

pub const CLAUDE_BUILTIN_TOOLS: &[&str] = &[
    "Read",
    "Edit",
    "Write",
    "MultiEdit",
    "Bash",
    "Grep",
    "Glob",
    "LS",
    "WebFetch",
    "WebSearch",
    "Task",
    "TodoWrite",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentDto {
    pub name: String,
    pub provider: CliProvider,
    pub scope: ConfigScope,
    pub description: String,
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
    pub permission_mode: Option<String>,
    pub system_prompt: String,
    pub raw_markdown: String,
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertSubagentRequest {
    pub name: String,
    pub description: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
    pub permission_mode: Option<String>,
    pub system_prompt: Option<String>,
    pub raw_markdown: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillCompanionFileDto {
    pub relative_path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillDto {
    pub name: String,
    pub provider: CliProvider,
    pub scope: ConfigScope,
    pub description: String,
    pub skill_md: String,
    #[serde(default)]
    pub companion_files: Vec<SkillCompanionFileDto>,
    pub is_shared: bool,
    #[serde(default)]
    pub shared_providers: Vec<CliProvider>,
    pub dir_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertSkillRequest {
    pub name: String,
    pub skill_md: String,
    #[serde(default)]
    pub companion_files: Vec<SkillCompanionFileDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteSkillCompanionRequest {
    pub relative_path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareSkillRequest {
    pub shared: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerDto {
    pub name: String,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub url: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpPresetDto {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub server: McpServerDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderMcpConfigDto {
    pub provider: CliProvider,
    pub scope: ConfigScope,
    pub config_path: String,
    #[serde(default)]
    pub servers: Vec<McpServerDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertMcpServerRequest {
    pub name: String,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub url: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncMcpServerRequest {
    #[serde(default)]
    pub target_providers: Vec<CliProvider>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncMcpServerResponse {
    pub server: McpServerDto,
    pub synced_providers: Vec<CliProvider>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphifyIndexResponse {
    pub project_id: String,
    pub graph_path: String,
    pub status: String,
    pub output: String,
}
