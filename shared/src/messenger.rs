//! Telegram and Discord pairing DTOs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingCode {
    pub code: String,
    pub expires_in_secs: u64,
}
