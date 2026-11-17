use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliProvider {
    #[serde(alias = "ClaudeCode", alias = "claude")]
    ClaudeCode,
    #[serde(alias = "Codex", alias = "codex")]
    Codex,
    #[serde(alias = "Antigravity", alias = "agy")]
    Antigravity,
}

impl CliProvider {
    pub const ALL: [CliProvider; 3] = [
        CliProvider::ClaudeCode,
        CliProvider::Codex,
        CliProvider::Antigravity,
    ];

    pub const SHARED_SUBDIRS: [&'static str; 4] = ["projects", "sessions", "skills", "agents"];

    pub fn as_str(&self) -> &'static str {
        match self {
            CliProvider::ClaudeCode => "claude_code",
            CliProvider::Codex => "codex",
            CliProvider::Antigravity => "antigravity",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            CliProvider::ClaudeCode => "Claude Code",
            CliProvider::Codex => "Codex",
            CliProvider::Antigravity => "Antigravity",
        }
    }

    pub fn cli_binary(&self) -> &'static str {
        match self {
            CliProvider::ClaudeCode => "claude",
            CliProvider::Codex => "codex",
            CliProvider::Antigravity => "agy",
        }
    }

    pub fn login_command(&self) -> (&'static str, &'static [&'static str]) {
        match self {
            CliProvider::ClaudeCode => ("claude", &["login"]),
            CliProvider::Codex => ("codex", &["login"]),
            CliProvider::Antigravity => ("agy", &[]),
        }
    }

    pub fn config_env_var(&self) -> &'static str {
        match self {
            CliProvider::ClaudeCode => "CLAUDE_CONFIG_DIR",
            CliProvider::Codex => "CODEX_HOME",
            CliProvider::Antigravity => "ANTIGRAVITY_CONFIG_DIR",
        }
    }
}

impl fmt::Display for CliProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CliProvider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude_code" | "claudecode" | "claude" => Ok(CliProvider::ClaudeCode),
            "codex" => Ok(CliProvider::Codex),
            "antigravity" | "agy" => Ok(CliProvider::Antigravity),
            other => Err(format!("unknown cli provider: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CliAccountDto {
    pub id: String,
    pub provider: CliProvider,
    pub name: String,
    pub slug: String,
    pub profile_dir: String,
    #[serde(default)]
    pub credential_files: Vec<String>,
    #[serde(default)]
    pub has_credentials: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAccountRequest {
    pub provider: CliProvider,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateAccountRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialFileDto {
    pub filename: String,
    pub size_bytes: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadCredentialRequest {
    pub filename: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountSummaryDto {
    pub id: String,
    pub provider: CliProvider,
    pub name: String,
    pub slug: String,
    pub has_credentials: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectCliStateDto {
    pub project_id: String,
    pub active_provider: CliProvider,
    pub active_account_id: Option<String>,
    pub active_account_name: Option<String>,
    pub can_switch: bool,
    pub available_accounts: Vec<AccountSummaryDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwitchProjectCliRequest {
    pub provider: CliProvider,
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountAclDto {
    pub user_id: String,
    pub project_id: String,
    pub restricted: bool,
    pub allowed_accounts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateAccountAclRequest {
    pub allowed_accounts: Option<Vec<String>>,
}
