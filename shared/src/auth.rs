use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    #[serde(alias = "Admin")]
    Admin,
    #[serde(alias = "Developer")]
    Developer,
    #[serde(alias = "Viewer")]
    Viewer,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Developer => "developer",
            Role::Viewer => "viewer",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "admin" => Ok(Role::Admin),
            "developer" => Ok(Role::Developer),
            "viewer" => Ok(Role::Viewer),
            other => Err(format!("unknown role: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthStatusResponse {
    pub initialized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapAdminRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    #[serde(alias = "username")]
    pub identifier: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshTokenRequest {
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserDto {
    pub id: String,
    pub username: String,
    pub email: String,
    pub role: Role,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthTokensResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in_secs: u64,
    pub user: UserDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeResponse {
    pub user: UserDto,
    pub projects: Vec<ProjectDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateInviteRequest {
    pub role: Role,
    #[serde(default)]
    pub project_ids: Vec<String>,
    pub expires_in_hours: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InviteDto {
    pub id: String,
    pub code: String,
    pub role: Role,
    pub project_ids: Vec<String>,
    pub created_by: String,
    pub used_by: Option<String>,
    pub used_at: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedeemInviteRequest {
    pub code: String,
    pub username: String,
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    pub workspace_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateProjectRequest {
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectDto {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub workspace_path: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMemberDto {
    pub user_id: String,
    pub username: String,
    pub email: String,
    pub role: Role,
    pub assigned_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignProjectMemberRequest {
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevokeSessionsResponse {
    pub revoked_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiErrorResponse {
    pub error: String,
    pub code: String,
}
