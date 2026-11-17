//! Telegram and Discord pairing DTOs.

use serde::{Deserialize, Serialize};

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

    pub fn from_storage(value: &str) -> Option<Self> {
        match value {
            "telegram" => Some(Self::Telegram),
            "discord" => Some(Self::Discord),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingCode {
    pub code: String,
    pub expires_in_secs: u64,
}
