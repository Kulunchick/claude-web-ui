use crate::auth::{
    project_rbac_middleware, record_audit_log, AdminUser, AppError, AuthUser, ProjectContext,
};
use crate::traits::{CliInvocation, TurnEvent};
use crate::{fs_link, AppState};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use claude_web_ui_shared::accounts::CliProvider;
use claude_web_ui_shared::agents::{
    provider_project_dir_name, ConfigScope, GraphifyIndexResponse, McpPresetDto, McpServerDto,
    ProviderMcpConfigDto, ShareSkillRequest, SkillCompanionFileDto, SkillDto, SubagentDto,
    SyncMcpServerRequest, SyncMcpServerResponse, UpsertMcpServerRequest, UpsertSkillRequest,
    UpsertSubagentRequest, WriteSkillCompanionRequest,
};
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};
use sqlx::Row;
use std::collections::BTreeMap;
use std::path::{Path as StdPath, PathBuf};
use tokio::sync::mpsc;

#[derive(Debug, Deserialize)]
pub struct CompanionPathQuery {
    pub path: String,
}

pub fn router(state: AppState) -> Router<AppState> {
    let project_scoped = Router::new()
        .route(
            "/api/projects/{project_id}/agents/{provider}",
            get(list_project_agents).post(create_project_agent),
        )
        .route(
            "/api/projects/{project_id}/agents/{provider}/{name}",
            get(get_project_agent)
                .put(upsert_project_agent)
                .delete(delete_project_agent),
        )
        .route(
            "/api/projects/{project_id}/skills/{provider}",
            get(list_project_skills).post(create_project_skill),
        )
        .route(
            "/api/projects/{project_id}/skills/{provider}/{name}",
            get(get_project_skill)
                .put(upsert_project_skill)
                .delete(delete_project_skill),
        )
        .route(
            "/api/projects/{project_id}/skills/{provider}/{name}/share",
            put(share_project_skill),
        )
        .route(
            "/api/projects/{project_id}/skills/{provider}/{name}/files",
            put(write_project_skill_companion).delete(delete_project_skill_companion),
        )
        .route(
            "/api/projects/{project_id}/mcp/{provider}",
            get(get_project_mcp_config),
        )
        .route(
            "/api/projects/{project_id}/mcp/{provider}/{name}",
            put(upsert_project_mcp_server).delete(delete_project_mcp_server),
        )
        .route(
            "/api/projects/{project_id}/mcp/{provider}/{name}/sync",
            post(sync_project_mcp_server),
        )
        .route(
            "/api/projects/{project_id}/mcp/{provider}/presets/{preset_id}",
            post(apply_project_mcp_preset),
        )
        .route(
            "/api/projects/{project_id}/mcp/graphify/index",
            post(run_project_graphify_index),
        )
        .layer(middleware::from_fn_with_state(
            state,
            project_rbac_middleware,
        ));

    Router::new()
        .route("/api/mcp/presets", get(list_mcp_presets))
        .route(
            "/api/global/agents/{provider}",
            get(list_global_agents).post(create_global_agent),
        )
        .route(
            "/api/global/agents/{provider}/{name}",
            get(get_global_agent)
                .put(upsert_global_agent)
                .delete(delete_global_agent),
        )
        .route(
            "/api/global/skills/{provider}",
            get(list_global_skills).post(create_global_skill),
        )
        .route(
            "/api/global/skills/{provider}/{name}",
            get(get_global_skill)
                .put(upsert_global_skill)
                .delete(delete_global_skill),
        )
        .route(
            "/api/global/skills/{provider}/{name}/share",
            put(share_global_skill),
        )
        .route(
            "/api/global/skills/{provider}/{name}/files",
            put(write_global_skill_companion).delete(delete_global_skill_companion),
        )
        .route(
            "/api/global/mcp/{provider}",
            get(get_global_mcp_config),
        )
        .route(
            "/api/global/mcp/{provider}/{name}",
            put(upsert_global_mcp_server).delete(delete_global_mcp_server),
        )
        .route(
            "/api/global/mcp/{provider}/{name}/sync",
            post(sync_global_mcp_server),
        )
        .route(
            "/api/global/mcp/{provider}/presets/{preset_id}",
            post(apply_global_mcp_preset),
        )
        .merge(project_scoped)
}

fn validate_entry_name(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed != raw {
        return Err(AppError::bad_request("invalid entry name"));
    }
    if trimmed == "."
        || trimmed == ".."
        || trimmed.starts_with('.')
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains(':')
        || trimmed.contains('\0')
        || trimmed.contains("..")
    {
        return Err(AppError::bad_request(
            "entry name cannot contain path separators or traversal segments",
        ));
    }
    Ok(trimmed.to_string())
}

fn agents_dir(
    data_dir: &StdPath,
    workspace_path: Option<&StdPath>,
    provider: CliProvider,
) -> PathBuf {
    match workspace_path {
        Some(ws) => ws.join(provider_project_dir_name(provider)).join("agents"),
        None => data_dir
            .join("shared")
            .join(provider.as_str())
            .join("agents"),
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SubagentFrontmatter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_string_or_vec",
        skip_serializing_if = "Vec::is_empty"
    )]
    tools: Vec<String>,
    #[serde(
        default,
        rename = "disallowedTools",
        alias = "disallowed_tools",
        deserialize_with = "deserialize_string_or_vec",
        skip_serializing_if = "Vec::is_empty"
    )]
    disallowed_tools: Vec<String>,
    #[serde(
        default,
        rename = "permissionMode",
        alias = "permission_mode",
        skip_serializing_if = "Option::is_none"
    )]
    permission_mode: Option<String>,
}

fn deserialize_string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrVec {
        Null,
        Str(String),
        Vec(Vec<String>),
    }

    match Option::<StringOrVec>::deserialize(deserializer)? {
        None | Some(StringOrVec::Null) => Ok(Vec::new()),
        Some(StringOrVec::Str(s)) => Ok(s
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect()),
        Some(StringOrVec::Vec(v)) => Ok(v
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()),
    }
}

fn split_frontmatter(raw: &str) -> Result<(SubagentFrontmatter, String), AppError> {
    let normalized = raw.replace("\r\n", "\n");
    if let Some(after_open) = normalized.strip_prefix("---\n") {
        if let Some(idx) = after_open.find("\n---") {
            let yaml_part = &after_open[..idx];
            let rest = &after_open[idx + 4..];
            let body = rest.strip_prefix('\n').unwrap_or(rest).to_string();
            let fm: SubagentFrontmatter = serde_yaml_ng::from_str(yaml_part)
                .map_err(|e| AppError::bad_request(format!("invalid YAML frontmatter: {e}")))?;
            return Ok((fm, body));
        }
        return Err(AppError::bad_request(
            "unclosed YAML frontmatter delimiter '---'",
        ));
    }

    if normalized.trim() == "---" {
        return Err(AppError::bad_request(
            "unclosed YAML frontmatter delimiter '---'",
        ));
    }

    Ok((SubagentFrontmatter::default(), normalized))
}

fn render_subagent_markdown(fm: &SubagentFrontmatter, system_prompt: &str) -> Result<String, AppError> {
    let yaml_raw = serde_yaml_ng::to_string(fm)
        .map_err(|e| AppError::internal(format!("failed to serialize YAML frontmatter: {e}")))?;
    let yaml_clean = yaml_raw
        .strip_prefix("---\n")
        .unwrap_or(&yaml_raw)
        .trim_end();
    let body = system_prompt.replace("\r\n", "\n");
    let body_trimmed = body.trim_end();
    if body_trimmed.is_empty() {
        Ok(format!("---\n{yaml_clean}\n---\n"))
    } else {
        Ok(format!("---\n{yaml_clean}\n---\n{body_trimmed}\n"))
    }
}

fn parse_subagent_file(
    path: &StdPath,
    fallback_name: &str,
    provider: CliProvider,
    scope: ConfigScope,
) -> Result<SubagentDto, AppError> {
    let raw_markdown = std::fs::read_to_string(path)
        .map_err(|e| AppError::internal(format!("failed to read subagent file: {e}")))?;
    let (fm, system_prompt) = split_frontmatter(&raw_markdown)?;
    Ok(SubagentDto {
        name: fm.name.unwrap_or_else(|| fallback_name.to_string()),
        provider,
        scope,
        description: fm.description.unwrap_or_default(),
        model: fm.model,
        tools: fm.tools,
        disallowed_tools: fm.disallowed_tools,
        permission_mode: fm.permission_mode,
        system_prompt,
        raw_markdown,
        file_path: path.to_string_lossy().into_owned(),
    })
}

fn list_agents_in_dir(
    dir: &StdPath,
    provider: CliProvider,
    scope: ConfigScope,
) -> Result<Vec<SubagentDto>, AppError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(dir)
        .map_err(|e| AppError::internal(format!("failed to read agents dir: {e}")))?;
    let mut items = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path.extension().and_then(|s| s.to_str()) == Some("md")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            items.push(parse_subagent_file(&path, stem, provider, scope)?);
        }
    }
    items.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(items)
}

fn write_subagent(
    dir: &StdPath,
    name: &str,
    provider: CliProvider,
    scope: ConfigScope,
    req: UpsertSubagentRequest,
) -> Result<SubagentDto, AppError> {
    let safe_name = validate_entry_name(name)?;
    std::fs::create_dir_all(dir)
        .map_err(|e| AppError::internal(format!("failed to create agents dir: {e}")))?;

    let target = dir.join(format!("{safe_name}.md"));
    let content = if let Some(raw) = req.raw_markdown {
        let (mut fm, body) = split_frontmatter(&raw)?;
        if fm.name.as_deref().map(str::trim).unwrap_or("").is_empty() {
            fm.name = Some(safe_name.clone());
            render_subagent_markdown(&fm, &body)?
        } else if raw.ends_with('\n') {
            raw.replace("\r\n", "\n")
        } else {
            format!("{}\n", raw.replace("\r\n", "\n"))
        }
    } else {
        let fm = SubagentFrontmatter {
            name: Some(safe_name.clone()),
            description: req
                .description
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            model: req
                .model
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            tools: req
                .tools
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            disallowed_tools: req
                .disallowed_tools
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            permission_mode: req
                .permission_mode
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        };
        render_subagent_markdown(&fm, req.system_prompt.as_deref().unwrap_or(""))?
    };

    std::fs::write(&target, content.as_bytes())
        .map_err(|e| AppError::internal(format!("failed to write subagent file: {e}")))?;

    parse_subagent_file(&target, &safe_name, provider, scope)
}

fn delete_subagent_file(dir: &StdPath, name: &str) -> Result<(), AppError> {
    let safe_name = validate_entry_name(name)?;
    let target = dir.join(format!("{safe_name}.md"));
    if !target.exists() {
        return Err(AppError::not_found("subagent not found"));
    }
    std::fs::remove_file(&target)
        .map_err(|e| AppError::internal(format!("failed to delete subagent file: {e}")))?;
    Ok(())
}

async fn list_global_agents(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(provider): Path<CliProvider>,
) -> Result<Json<Vec<SubagentDto>>, AppError> {
    let dir = agents_dir(&state.config.data_dir, None, provider);
    Ok(Json(list_agents_in_dir(&dir, provider, ConfigScope::Global)?))
}

async fn get_global_agent(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((provider, name)): Path<(CliProvider, String)>,
) -> Result<Json<SubagentDto>, AppError> {
    let safe_name = validate_entry_name(&name)?;
    let path = agents_dir(&state.config.data_dir, None, provider).join(format!("{safe_name}.md"));
    if !path.is_file() {
        return Err(AppError::not_found("subagent not found"));
    }
    Ok(Json(parse_subagent_file(
        &path,
        &safe_name,
        provider,
        ConfigScope::Global,
    )?))
}

async fn create_global_agent(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(provider): Path<CliProvider>,
    Json(req): Json<UpsertSubagentRequest>,
) -> Result<(StatusCode, Json<SubagentDto>), AppError> {
    let safe_name = validate_entry_name(&req.name)?;
    let dir = agents_dir(&state.config.data_dir, None, provider);
    let dto = write_subagent(&dir, &safe_name, provider, ConfigScope::Global, req)?;
    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "upsert_global_subagent",
        Some("subagent"),
        Some(&dto.name),
        Some(provider.as_str()),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(dto)))
}

async fn upsert_global_agent(
    State(state): State<AppState>,
    admin: AdminUser,
    Path((provider, name)): Path<(CliProvider, String)>,
    Json(req): Json<UpsertSubagentRequest>,
) -> Result<Json<SubagentDto>, AppError> {
    let safe_name = validate_entry_name(&name)?;
    let dir = agents_dir(&state.config.data_dir, None, provider);
    let dto = write_subagent(&dir, &safe_name, provider, ConfigScope::Global, req)?;
    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "upsert_global_subagent",
        Some("subagent"),
        Some(&dto.name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(Json(dto))
}

async fn delete_global_agent(
    State(state): State<AppState>,
    admin: AdminUser,
    Path((provider, name)): Path<(CliProvider, String)>,
) -> Result<StatusCode, AppError> {
    let dir = agents_dir(&state.config.data_dir, None, provider);
    delete_subagent_file(&dir, &name)?;
    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "delete_global_subagent",
        Some("subagent"),
        Some(&name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_project_agents(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider)): Path<(String, CliProvider)>,
) -> Result<Json<Vec<SubagentDto>>, AppError> {
    let dir = agents_dir(
        &state.config.data_dir,
        Some(StdPath::new(&ctx.project.workspace_path)),
        provider,
    );
    Ok(Json(list_agents_in_dir(
        &dir,
        provider,
        ConfigScope::Project,
    )?))
}

async fn get_project_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider, name)): Path<(String, CliProvider, String)>,
) -> Result<Json<SubagentDto>, AppError> {
    let safe_name = validate_entry_name(&name)?;
    let path = agents_dir(
        &state.config.data_dir,
        Some(StdPath::new(&ctx.project.workspace_path)),
        provider,
    )
    .join(format!("{safe_name}.md"));
    if !path.is_file() {
        return Err(AppError::not_found("subagent not found"));
    }
    Ok(Json(parse_subagent_file(
        &path,
        &safe_name,
        provider,
        ConfigScope::Project,
    )?))
}

async fn create_project_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider)): Path<(String, CliProvider)>,
    Json(req): Json<UpsertSubagentRequest>,
) -> Result<(StatusCode, Json<SubagentDto>), AppError> {
    let safe_name = validate_entry_name(&req.name)?;
    let dir = agents_dir(
        &state.config.data_dir,
        Some(StdPath::new(&ctx.project.workspace_path)),
        provider,
    );
    let dto = write_subagent(&dir, &safe_name, provider, ConfigScope::Project, req)?;
    record_audit_log(
        &state,
        Some(&ctx.user.user.id),
        "upsert_project_subagent",
        Some("subagent"),
        Some(&dto.name),
        Some(provider.as_str()),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(dto)))
}

async fn upsert_project_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider, name)): Path<(String, CliProvider, String)>,
    Json(req): Json<UpsertSubagentRequest>,
) -> Result<Json<SubagentDto>, AppError> {
    let safe_name = validate_entry_name(&name)?;
    let dir = agents_dir(
        &state.config.data_dir,
        Some(StdPath::new(&ctx.project.workspace_path)),
        provider,
    );
    let dto = write_subagent(&dir, &safe_name, provider, ConfigScope::Project, req)?;
    record_audit_log(
        &state,
        Some(&ctx.user.user.id),
        "upsert_project_subagent",
        Some("subagent"),
        Some(&dto.name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(Json(dto))
}

async fn delete_project_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider, name)): Path<(String, CliProvider, String)>,
) -> Result<StatusCode, AppError> {
    let dir = agents_dir(
        &state.config.data_dir,
        Some(StdPath::new(&ctx.project.workspace_path)),
        provider,
    );
    delete_subagent_file(&dir, &name)?;
    record_audit_log(
        &state,
        Some(&ctx.user.user.id),
        "delete_project_subagent",
        Some("subagent"),
        Some(&name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ============================================================================
// Skills CRUD, Companion Files & Cross-CLI Symlink/Junction Sharing
// ============================================================================

const SKILL_COMPANION_SUBDIRS: [&str; 3] = ["scripts", "references", "examples"];

fn skills_dir(
    data_dir: &StdPath,
    workspace_path: Option<&StdPath>,
    provider: CliProvider,
) -> PathBuf {
    match workspace_path {
        Some(ws) => ws.join(provider_project_dir_name(provider)).join("skills"),
        None => data_dir
            .join("shared")
            .join(provider.as_str())
            .join("skills"),
    }
}

fn validate_companion_rel_path(raw: &str) -> Result<PathBuf, AppError> {
    let normalized = raw.trim().replace('\\', "/");
    if normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.contains(':')
        || normalized.contains('\0')
    {
        return Err(AppError::bad_request("invalid companion file path"));
    }
    let parts: Vec<&str> = normalized.split('/').collect();
    if parts.len() < 2 || !SKILL_COMPANION_SUBDIRS.contains(&parts[0]) {
        return Err(AppError::bad_request(
            "companion files must live under scripts/, references/, or examples/",
        ));
    }
    let mut rel = PathBuf::new();
    for seg in parts {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err(AppError::bad_request("invalid path segment in companion file"));
        }
        rel.push(seg);
    }
    Ok(rel)
}

#[derive(Debug, Default, Deserialize)]
struct SkillFrontmatter {
    #[serde(default)]
    description: Option<String>,
}

fn extract_skill_description(skill_md: &str) -> String {
    let normalized = skill_md.replace("\r\n", "\n");
    if let Some(after_open) = normalized.strip_prefix("---\n")
        && let Some(idx) = after_open.find("\n---")
        && let Ok(fm) = serde_yaml_ng::from_str::<SkillFrontmatter>(&after_open[..idx])
        && let Some(desc) = fm.description
        && !desc.trim().is_empty()
    {
        return desc.trim().to_string();
    }
    normalized
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#') && *line != "---")
        .unwrap_or("")
        .to_string()
}

fn collect_companion_files_recursive(
    base_dir: &StdPath,
    subdir_root: &StdPath,
    out: &mut Vec<SkillCompanionFileDto>,
) -> Result<(), AppError> {
    if !subdir_root.exists() {
        return Ok(());
    }
    let entries = std::fs::read_dir(subdir_root)
        .map_err(|e| AppError::internal(format!("failed to read skill subdir: {e}")))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_companion_files_recursive(base_dir, &path, out)?;
        } else if meta.is_file() {
            let rel = path
                .strip_prefix(base_dir)
                .map_err(|e| AppError::internal(format!("strip_prefix failed: {e}")))?;
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            let bytes = std::fs::read(&path)
                .map_err(|e| AppError::internal(format!("failed to read companion file: {e}")))?;
            let content = String::from_utf8_lossy(&bytes).into_owned();
            out.push(SkillCompanionFileDto {
                relative_path: rel_str,
                content,
            });
        }
    }
    Ok(())
}

fn detect_shared_providers(
    data_dir: &StdPath,
    workspace_path: Option<&StdPath>,
    skill_name: &str,
    skill_dir: &StdPath,
) -> Vec<CliProvider> {
    let Ok(canonical_current) = std::fs::canonicalize(skill_dir) else {
        return Vec::new();
    };
    let mut providers = Vec::new();
    for prov in CliProvider::ALL {
        let candidate = skills_dir(data_dir, workspace_path, prov).join(skill_name);
        if candidate.exists()
            && let Ok(canon) = std::fs::canonicalize(&candidate)
            && canon == canonical_current
        {
            providers.push(prov);
        }
    }
    providers
}

fn load_skill_dto(
    data_dir: &StdPath,
    workspace_path: Option<&StdPath>,
    provider: CliProvider,
    scope: ConfigScope,
    name: &str,
) -> Result<SkillDto, AppError> {
    let safe_name = validate_entry_name(name)?;
    let skill_dir = skills_dir(data_dir, workspace_path, provider).join(&safe_name);
    if !skill_dir.is_dir() {
        return Err(AppError::not_found("skill not found"));
    }
    let skill_md_path = skill_dir.join("SKILL.md");
    if !skill_md_path.is_file() {
        return Err(AppError::not_found("SKILL.md missing in skill directory"));
    }
    let skill_md = std::fs::read_to_string(&skill_md_path)
        .map_err(|e| AppError::internal(format!("failed to read SKILL.md: {e}")))?;
    let description = extract_skill_description(&skill_md);

    let mut companion_files = Vec::new();
    for sub in SKILL_COMPANION_SUBDIRS {
        collect_companion_files_recursive(&skill_dir, &skill_dir.join(sub), &mut companion_files)?;
    }
    companion_files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

    let shared_providers = detect_shared_providers(data_dir, workspace_path, &safe_name, &skill_dir);
    let is_shared = shared_providers.len() > 1;

    Ok(SkillDto {
        name: safe_name,
        provider,
        scope,
        description,
        skill_md,
        companion_files,
        is_shared,
        shared_providers,
        dir_path: skill_dir.to_string_lossy().into_owned(),
    })
}

fn list_skills_in_scope(
    data_dir: &StdPath,
    workspace_path: Option<&StdPath>,
    provider: CliProvider,
    scope: ConfigScope,
) -> Result<Vec<SkillDto>, AppError> {
    let root = skills_dir(data_dir, workspace_path, provider);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&root)
        .map_err(|e| AppError::internal(format!("failed to read skills dir: {e}")))?;
    let mut skills = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir()
            && path.join("SKILL.md").is_file()
            && let Some(name) = path.file_name().and_then(|s| s.to_str())
        {
            skills.push(load_skill_dto(
                data_dir,
                workspace_path,
                provider,
                scope,
                name,
            )?);
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

fn write_skill_companion_file(
    skill_dir: &StdPath,
    relative_path: &str,
    content: &str,
) -> Result<(), AppError> {
    let rel = validate_companion_rel_path(relative_path)?;
    let target = skill_dir.join(&rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::internal(format!("failed to create companion dir: {e}")))?;
    }
    if fs_link::is_dir_link(&target) {
        return Err(AppError::bad_request("companion path cannot be a symlink"));
    }
    std::fs::write(&target, content.as_bytes())
        .map_err(|e| AppError::internal(format!("failed to write companion file: {e}")))?;
    Ok(())
}

fn upsert_skill_files(
    data_dir: &StdPath,
    workspace_path: Option<&StdPath>,
    provider: CliProvider,
    scope: ConfigScope,
    name: &str,
    req: UpsertSkillRequest,
) -> Result<SkillDto, AppError> {
    let safe_name = validate_entry_name(name)?;
    let skill_dir = skills_dir(data_dir, workspace_path, provider).join(&safe_name);
    std::fs::create_dir_all(&skill_dir)
        .map_err(|e| AppError::internal(format!("failed to create skill dir: {e}")))?;

    std::fs::write(skill_dir.join("SKILL.md"), req.skill_md.as_bytes())
        .map_err(|e| AppError::internal(format!("failed to write SKILL.md: {e}")))?;

    for comp in req.companion_files {
        write_skill_companion_file(&skill_dir, &comp.relative_path, &comp.content)?;
    }

    load_skill_dto(data_dir, workspace_path, provider, scope, &safe_name)
}

fn toggle_skill_share(
    data_dir: &StdPath,
    workspace_path: Option<&StdPath>,
    provider: CliProvider,
    scope: ConfigScope,
    name: &str,
    shared: bool,
) -> Result<SkillDto, AppError> {
    let safe_name = validate_entry_name(name)?;
    let skill_dir = skills_dir(data_dir, workspace_path, provider).join(&safe_name);
    if !skill_dir.is_dir() {
        return Err(AppError::not_found("skill not found"));
    }
    let canonical_source = std::fs::canonicalize(&skill_dir)
        .map_err(|e| AppError::internal(format!("canonicalize skill dir failed: {e}")))?;

    if shared {
        for other in CliProvider::ALL {
            let target_link = skills_dir(data_dir, workspace_path, other).join(&safe_name);
            if std::fs::symlink_metadata(&target_link).is_ok() {
                let canon_other = std::fs::canonicalize(&target_link).ok();
                if canon_other.as_ref() != Some(&canonical_source) {
                    return Err(AppError::conflict(format!(
                        "skill '{}' already exists independently for {}",
                        safe_name,
                        other.display_name()
                    )));
                }
            } else {
                fs_link::link_dir(&canonical_source, &target_link).map_err(|e| {
                    AppError::internal(format!(
                        "failed to link skill to {}: {e}",
                        other.display_name()
                    ))
                })?;
            }
        }
    } else {
        for other in CliProvider::ALL {
            let target_link = skills_dir(data_dir, workspace_path, other).join(&safe_name);
            if fs_link::is_dir_link(&target_link)
                && std::fs::canonicalize(&target_link).ok().as_ref() == Some(&canonical_source)
            {
                fs_link::unlink_dir(&target_link).map_err(|e| {
                    AppError::internal(format!(
                        "failed to unlink skill from {}: {e}",
                        other.display_name()
                    ))
                })?;
            }
        }
    }

    load_skill_dto(data_dir, workspace_path, provider, scope, &safe_name)
}

fn delete_skill_dir(
    data_dir: &StdPath,
    workspace_path: Option<&StdPath>,
    provider: CliProvider,
    name: &str,
) -> Result<(), AppError> {
    let safe_name = validate_entry_name(name)?;
    let skill_dir = skills_dir(data_dir, workspace_path, provider).join(&safe_name);
    if std::fs::symlink_metadata(&skill_dir).is_err() {
        return Err(AppError::not_found("skill not found"));
    }
    if fs_link::is_dir_link(&skill_dir) {
        fs_link::unlink_dir(&skill_dir)
            .map_err(|e| AppError::internal(format!("failed to unlink skill: {e}")))?;
        return Ok(());
    }

    if let Ok(canonical_source) = std::fs::canonicalize(&skill_dir) {
        for other in CliProvider::ALL {
            if other == provider {
                continue;
            }
            let other_link = skills_dir(data_dir, workspace_path, other).join(&safe_name);
            if fs_link::is_dir_link(&other_link)
                && std::fs::canonicalize(&other_link).ok().as_ref() == Some(&canonical_source)
            {
                fs_link::unlink_dir(&other_link).map_err(|e| {
                    AppError::internal(format!("failed to unlink shared skill copy: {e}"))
                })?;
            }
        }
    }

    std::fs::remove_dir_all(&skill_dir)
        .map_err(|e| AppError::internal(format!("failed to delete skill dir: {e}")))?;
    Ok(())
}

async fn list_global_skills(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(provider): Path<CliProvider>,
) -> Result<Json<Vec<SkillDto>>, AppError> {
    Ok(Json(list_skills_in_scope(
        &state.config.data_dir,
        None,
        provider,
        ConfigScope::Global,
    )?))
}

async fn get_global_skill(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((provider, name)): Path<(CliProvider, String)>,
) -> Result<Json<SkillDto>, AppError> {
    Ok(Json(load_skill_dto(
        &state.config.data_dir,
        None,
        provider,
        ConfigScope::Global,
        &name,
    )?))
}

async fn create_global_skill(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(provider): Path<CliProvider>,
    Json(req): Json<UpsertSkillRequest>,
) -> Result<(StatusCode, Json<SkillDto>), AppError> {
    let name = req.name.clone();
    let dto = upsert_skill_files(
        &state.config.data_dir,
        None,
        provider,
        ConfigScope::Global,
        &name,
        req,
    )?;
    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "upsert_global_skill",
        Some("skill"),
        Some(&dto.name),
        Some(provider.as_str()),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(dto)))
}

async fn upsert_global_skill(
    State(state): State<AppState>,
    admin: AdminUser,
    Path((provider, name)): Path<(CliProvider, String)>,
    Json(req): Json<UpsertSkillRequest>,
) -> Result<Json<SkillDto>, AppError> {
    let dto = upsert_skill_files(
        &state.config.data_dir,
        None,
        provider,
        ConfigScope::Global,
        &name,
        req,
    )?;
    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "upsert_global_skill",
        Some("skill"),
        Some(&dto.name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(Json(dto))
}

async fn delete_global_skill(
    State(state): State<AppState>,
    admin: AdminUser,
    Path((provider, name)): Path<(CliProvider, String)>,
) -> Result<StatusCode, AppError> {
    delete_skill_dir(&state.config.data_dir, None, provider, &name)?;
    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "delete_global_skill",
        Some("skill"),
        Some(&name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn share_global_skill(
    State(state): State<AppState>,
    admin: AdminUser,
    Path((provider, name)): Path<(CliProvider, String)>,
    Json(req): Json<ShareSkillRequest>,
) -> Result<Json<SkillDto>, AppError> {
    let dto = toggle_skill_share(
        &state.config.data_dir,
        None,
        provider,
        ConfigScope::Global,
        &name,
        req.shared,
    )?;
    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        if req.shared {
            "share_global_skill"
        } else {
            "unshare_global_skill"
        },
        Some("skill"),
        Some(&dto.name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(Json(dto))
}

async fn write_global_skill_companion(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path((provider, name)): Path<(CliProvider, String)>,
    Json(req): Json<WriteSkillCompanionRequest>,
) -> Result<Json<SkillDto>, AppError> {
    let safe_name = validate_entry_name(&name)?;
    let skill_dir = skills_dir(&state.config.data_dir, None, provider).join(&safe_name);
    if !skill_dir.is_dir() {
        return Err(AppError::not_found("skill not found"));
    }
    write_skill_companion_file(&skill_dir, &req.relative_path, &req.content)?;
    Ok(Json(load_skill_dto(
        &state.config.data_dir,
        None,
        provider,
        ConfigScope::Global,
        &safe_name,
    )?))
}

async fn delete_global_skill_companion(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path((provider, name)): Path<(CliProvider, String)>,
    Query(query): Query<CompanionPathQuery>,
) -> Result<Json<SkillDto>, AppError> {
    let safe_name = validate_entry_name(&name)?;
    let skill_dir = skills_dir(&state.config.data_dir, None, provider).join(&safe_name);
    if !skill_dir.is_dir() {
        return Err(AppError::not_found("skill not found"));
    }
    let rel = validate_companion_rel_path(&query.path)?;
    let target = skill_dir.join(&rel);
    if !target.is_file() {
        return Err(AppError::not_found("companion file not found"));
    }
    std::fs::remove_file(&target)
        .map_err(|e| AppError::internal(format!("failed to delete companion file: {e}")))?;
    Ok(Json(load_skill_dto(
        &state.config.data_dir,
        None,
        provider,
        ConfigScope::Global,
        &safe_name,
    )?))
}

async fn list_project_skills(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider)): Path<(String, CliProvider)>,
) -> Result<Json<Vec<SkillDto>>, AppError> {
    Ok(Json(list_skills_in_scope(
        &state.config.data_dir,
        Some(StdPath::new(&ctx.project.workspace_path)),
        provider,
        ConfigScope::Project,
    )?))
}

async fn get_project_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider, name)): Path<(String, CliProvider, String)>,
) -> Result<Json<SkillDto>, AppError> {
    Ok(Json(load_skill_dto(
        &state.config.data_dir,
        Some(StdPath::new(&ctx.project.workspace_path)),
        provider,
        ConfigScope::Project,
        &name,
    )?))
}

async fn create_project_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider)): Path<(String, CliProvider)>,
    Json(req): Json<UpsertSkillRequest>,
) -> Result<(StatusCode, Json<SkillDto>), AppError> {
    let name = req.name.clone();
    let dto = upsert_skill_files(
        &state.config.data_dir,
        Some(StdPath::new(&ctx.project.workspace_path)),
        provider,
        ConfigScope::Project,
        &name,
        req,
    )?;
    record_audit_log(
        &state,
        Some(&ctx.user.user.id),
        "upsert_project_skill",
        Some("skill"),
        Some(&dto.name),
        Some(provider.as_str()),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(dto)))
}

async fn upsert_project_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider, name)): Path<(String, CliProvider, String)>,
    Json(req): Json<UpsertSkillRequest>,
) -> Result<Json<SkillDto>, AppError> {
    let dto = upsert_skill_files(
        &state.config.data_dir,
        Some(StdPath::new(&ctx.project.workspace_path)),
        provider,
        ConfigScope::Project,
        &name,
        req,
    )?;
    record_audit_log(
        &state,
        Some(&ctx.user.user.id),
        "upsert_project_skill",
        Some("skill"),
        Some(&dto.name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(Json(dto))
}

async fn delete_project_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider, name)): Path<(String, CliProvider, String)>,
) -> Result<StatusCode, AppError> {
    delete_skill_dir(
        &state.config.data_dir,
        Some(StdPath::new(&ctx.project.workspace_path)),
        provider,
        &name,
    )?;
    record_audit_log(
        &state,
        Some(&ctx.user.user.id),
        "delete_project_skill",
        Some("skill"),
        Some(&name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn share_project_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider, name)): Path<(String, CliProvider, String)>,
    Json(req): Json<ShareSkillRequest>,
) -> Result<Json<SkillDto>, AppError> {
    let dto = toggle_skill_share(
        &state.config.data_dir,
        Some(StdPath::new(&ctx.project.workspace_path)),
        provider,
        ConfigScope::Project,
        &name,
        req.shared,
    )?;
    record_audit_log(
        &state,
        Some(&ctx.user.user.id),
        if req.shared {
            "share_project_skill"
        } else {
            "unshare_project_skill"
        },
        Some("skill"),
        Some(&dto.name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(Json(dto))
}

async fn write_project_skill_companion(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider, name)): Path<(String, CliProvider, String)>,
    Json(req): Json<WriteSkillCompanionRequest>,
) -> Result<Json<SkillDto>, AppError> {
    let safe_name = validate_entry_name(&name)?;
    let ws = StdPath::new(&ctx.project.workspace_path);
    let skill_dir = skills_dir(&state.config.data_dir, Some(ws), provider).join(&safe_name);
    if !skill_dir.is_dir() {
        return Err(AppError::not_found("skill not found"));
    }
    write_skill_companion_file(&skill_dir, &req.relative_path, &req.content)?;
    Ok(Json(load_skill_dto(
        &state.config.data_dir,
        Some(ws),
        provider,
        ConfigScope::Project,
        &safe_name,
    )?))
}

async fn delete_project_skill_companion(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider, name)): Path<(String, CliProvider, String)>,
    Query(query): Query<CompanionPathQuery>,
) -> Result<Json<SkillDto>, AppError> {
    let safe_name = validate_entry_name(&name)?;
    let ws = StdPath::new(&ctx.project.workspace_path);
    let skill_dir = skills_dir(&state.config.data_dir, Some(ws), provider).join(&safe_name);
    if !skill_dir.is_dir() {
        return Err(AppError::not_found("skill not found"));
    }
    let rel = validate_companion_rel_path(&query.path)?;
    let target = skill_dir.join(&rel);
    if !target.is_file() {
        return Err(AppError::not_found("companion file not found"));
    }
    std::fs::remove_file(&target)
        .map_err(|e| AppError::internal(format!("failed to delete companion file: {e}")))?;
    Ok(Json(load_skill_dto(
        &state.config.data_dir,
        Some(ws),
        provider,
        ConfigScope::Project,
        &safe_name,
    )?))
}

// ============================================================================
// MCP Server Manager, Presets, Cross-CLI Sync & Graphify Indexing
// ============================================================================

const GRAPHIFY_PRESET_ID: &str = "graphify-mcp";
const GRAPHIFY_GRAPH_PATH: &str = "graphify-out/graph.json";

fn built_in_mcp_presets() -> Vec<McpPresetDto> {
    vec![
        McpPresetDto {
            id: GRAPHIFY_PRESET_ID.to_string(),
            display_name: "Graphify Knowledge Graph (graphify-mcp)".to_string(),
            description: "Expose graphify-out/graph.json knowledge graph as an MCP server."
                .to_string(),
            server: McpServerDto {
                name: "graphify".to_string(),
                command: Some("graphify-mcp".to_string()),
                args: vec![GRAPHIFY_GRAPH_PATH.to_string()],
                env: BTreeMap::new(),
                url: None,
                disabled: false,
            },
        },
        McpPresetDto {
            id: "chrome-devtools".to_string(),
            display_name: "Chrome DevTools MCP".to_string(),
            description: "Inspect live DOM, console logs, and network traffic in Chrome."
                .to_string(),
            server: McpServerDto {
                name: "chrome-devtools".to_string(),
                command: Some("npx".to_string()),
                args: vec!["-y".to_string(), "chrome-devtools-mcp@latest".to_string()],
                env: BTreeMap::new(),
                url: None,
                disabled: false,
            },
        },
        McpPresetDto {
            id: "playwright".to_string(),
            display_name: "Playwright Browser MCP".to_string(),
            description: "Automate browser navigation, screenshots, and website inspection."
                .to_string(),
            server: McpServerDto {
                name: "playwright".to_string(),
                command: Some("npx".to_string()),
                args: vec!["-y".to_string(), "@playwright/mcp@latest".to_string()],
                env: BTreeMap::new(),
                url: None,
                disabled: false,
            },
        },
    ]
}

fn mcp_config_path(
    data_dir: &StdPath,
    workspace_path: Option<&StdPath>,
    provider: CliProvider,
) -> PathBuf {
    match (workspace_path, provider) {
        (Some(ws), CliProvider::ClaudeCode) => ws.join(".mcp.json"),
        (Some(ws), CliProvider::Codex) => ws.join(".codex").join("config.toml"),
        (Some(ws), CliProvider::Antigravity) => ws.join(".agents").join("mcp.json"),
        (None, CliProvider::ClaudeCode) => data_dir
            .join("shared")
            .join(provider.as_str())
            .join(".claude.json"),
        (None, CliProvider::Codex) => data_dir
            .join("shared")
            .join(provider.as_str())
            .join("config.toml"),
        (None, CliProvider::Antigravity) => data_dir
            .join("shared")
            .join(provider.as_str())
            .join("mcp.json"),
    }
}

fn json_value_to_mcp_server(name: String, val: &JsonValue) -> McpServerDto {
    let obj = val.as_object();
    let command = obj
        .and_then(|m| m.get("command"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let args = obj
        .and_then(|m| m.get("args"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let env = obj
        .and_then(|m| m.get("env"))
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let url = obj
        .and_then(|m| m.get("url"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let disabled = obj
        .and_then(|m| m.get("disabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    McpServerDto {
        name,
        command,
        args,
        env,
        url,
        disabled,
    }
}

fn mcp_server_to_json_value(srv: &McpServerDto) -> JsonValue {
    let mut obj = JsonMap::new();
    if let Some(cmd) = &srv.command {
        obj.insert("command".to_string(), JsonValue::String(cmd.clone()));
    }
    if !srv.args.is_empty() {
        obj.insert(
            "args".to_string(),
            JsonValue::Array(srv.args.iter().cloned().map(JsonValue::String).collect()),
        );
    }
    if !srv.env.is_empty() {
        let env_map: JsonMap<String, JsonValue> = srv
            .env
            .iter()
            .map(|(k, v)| (k.clone(), JsonValue::String(v.clone())))
            .collect();
        obj.insert("env".to_string(), JsonValue::Object(env_map));
    }
    if let Some(url) = &srv.url {
        obj.insert("url".to_string(), JsonValue::String(url.clone()));
    }
    if srv.disabled {
        obj.insert("disabled".to_string(), JsonValue::Bool(true));
    }
    JsonValue::Object(obj)
}

fn toml_value_to_mcp_server(name: String, val: &toml::Value) -> McpServerDto {
    let tbl = val.as_table();
    let command = tbl
        .and_then(|m| m.get("command"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let args = tbl
        .and_then(|m| m.get("args"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let env = tbl
        .and_then(|m| m.get("env"))
        .and_then(|v| v.as_table())
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let url = tbl
        .and_then(|m| m.get("url"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let enabled = tbl
        .and_then(|m| m.get("enabled"))
        .and_then(|v| v.as_bool());
    let disabled_flag = tbl
        .and_then(|m| m.get("disabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let disabled = disabled_flag || enabled == Some(false);

    McpServerDto {
        name,
        command,
        args,
        env,
        url,
        disabled,
    }
}

fn mcp_server_to_toml_value(srv: &McpServerDto) -> toml::Value {
    let mut tbl = toml::map::Map::new();
    if let Some(cmd) = &srv.command {
        tbl.insert("command".to_string(), toml::Value::String(cmd.clone()));
    }
    if !srv.args.is_empty() {
        tbl.insert(
            "args".to_string(),
            toml::Value::Array(srv.args.iter().cloned().map(toml::Value::String).collect()),
        );
    }
    if !srv.env.is_empty() {
        let mut env_tbl = toml::map::Map::new();
        for (k, v) in &srv.env {
            env_tbl.insert(k.clone(), toml::Value::String(v.clone()));
        }
        tbl.insert("env".to_string(), toml::Value::Table(env_tbl));
    }
    if let Some(url) = &srv.url {
        tbl.insert("url".to_string(), toml::Value::String(url.clone()));
    }
    if srv.disabled {
        tbl.insert("enabled".to_string(), toml::Value::Boolean(false));
    }
    toml::Value::Table(tbl)
}

fn read_mcp_config(
    data_dir: &StdPath,
    workspace_path: Option<&StdPath>,
    provider: CliProvider,
    scope: ConfigScope,
) -> Result<ProviderMcpConfigDto, AppError> {
    let path = mcp_config_path(data_dir, workspace_path, provider);
    let config_path = path.to_string_lossy().into_owned();
    if !path.is_file() {
        return Ok(ProviderMcpConfigDto {
            provider,
            scope,
            config_path,
            servers: Vec::new(),
        });
    }

    let raw = std::fs::read_to_string(&path)
        .map_err(|e| AppError::internal(format!("failed to read mcp config: {e}")))?;
    let mut servers = Vec::new();

    if provider == CliProvider::Codex {
        let doc: toml::Table = if raw.trim().is_empty() {
            toml::Table::new()
        } else {
            toml::from_str(&raw)
                .map_err(|e| AppError::bad_request(format!("invalid codex config.toml: {e}")))?
        };
        if let Some(mcp_tbl) = doc.get("mcp_servers").and_then(|v| v.as_table()) {
            for (name, val) in mcp_tbl {
                servers.push(toml_value_to_mcp_server(name.clone(), val));
            }
        }
    } else {
        let doc: JsonValue = if raw.trim().is_empty() {
            JsonValue::Object(JsonMap::new())
        } else {
            serde_json::from_str(&raw)
                .map_err(|e| AppError::bad_request(format!("invalid mcp json config: {e}")))?
        };
        if let Some(mcp_obj) = doc.get("mcpServers").and_then(|v| v.as_object()) {
            for (name, val) in mcp_obj {
                servers.push(json_value_to_mcp_server(name.clone(), val));
            }
        }
    }

    servers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ProviderMcpConfigDto {
        provider,
        scope,
        config_path,
        servers,
    })
}

fn upsert_mcp_server_in_config(
    data_dir: &StdPath,
    workspace_path: Option<&StdPath>,
    provider: CliProvider,
    scope: ConfigScope,
    srv: McpServerDto,
) -> Result<ProviderMcpConfigDto, AppError> {
    let safe_name = validate_entry_name(&srv.name)?;
    let path = mcp_config_path(data_dir, workspace_path, provider);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::internal(format!("failed to create mcp config dir: {e}")))?;
    }

    if provider == CliProvider::Codex {
        let mut doc: toml::Table = if path.is_file() {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| AppError::internal(format!("failed to read codex config: {e}")))?;
            if raw.trim().is_empty() {
                toml::Table::new()
            } else {
                toml::from_str(&raw).map_err(|e| {
                    AppError::bad_request(format!("invalid codex config.toml: {e}"))
                })?
            }
        } else {
            toml::Table::new()
        };

        let mcp_entry = doc
            .entry("mcp_servers".to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let mcp_tbl = mcp_entry.as_table_mut().ok_or_else(|| {
            AppError::bad_request("existing mcp_servers in config.toml is not a table")
        })?;
        mcp_tbl.insert(safe_name, mcp_server_to_toml_value(&srv));

        let serialized = toml::to_string_pretty(&doc)
            .map_err(|e| AppError::internal(format!("failed to serialize codex toml: {e}")))?;
        std::fs::write(&path, serialized.as_bytes())
            .map_err(|e| AppError::internal(format!("failed to write codex config: {e}")))?;
    } else {
        let mut root: JsonMap<String, JsonValue> = if path.is_file() {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| AppError::internal(format!("failed to read mcp json: {e}")))?;
            if raw.trim().is_empty() {
                JsonMap::new()
            } else {
                serde_json::from_str(&raw)
                    .map_err(|e| AppError::bad_request(format!("invalid mcp json: {e}")))?
            }
        } else {
            JsonMap::new()
        };

        let mcp_entry = root
            .entry("mcpServers".to_string())
            .or_insert_with(|| JsonValue::Object(JsonMap::new()));
        let mcp_obj = mcp_entry
            .as_object_mut()
            .ok_or_else(|| AppError::bad_request("existing mcpServers is not a JSON object"))?;
        mcp_obj.insert(safe_name, mcp_server_to_json_value(&srv));

        let serialized = serde_json::to_string_pretty(&JsonValue::Object(root))
            .map_err(|e| AppError::internal(format!("failed to serialize mcp json: {e}")))?;
        std::fs::write(&path, format!("{serialized}\n").as_bytes())
            .map_err(|e| AppError::internal(format!("failed to write mcp json: {e}")))?;
    }

    read_mcp_config(data_dir, workspace_path, provider, scope)
}

fn delete_mcp_server_in_config(
    data_dir: &StdPath,
    workspace_path: Option<&StdPath>,
    provider: CliProvider,
    name: &str,
) -> Result<(), AppError> {
    let safe_name = validate_entry_name(name)?;
    let path = mcp_config_path(data_dir, workspace_path, provider);
    if !path.is_file() {
        return Err(AppError::not_found("mcp server not found"));
    }

    if provider == CliProvider::Codex {
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| AppError::internal(format!("failed to read codex config: {e}")))?;
        let mut doc: toml::Table = toml::from_str(&raw)
            .map_err(|e| AppError::bad_request(format!("invalid codex config.toml: {e}")))?;
        let removed = doc
            .get_mut("mcp_servers")
            .and_then(|v| v.as_table_mut())
            .and_then(|tbl| tbl.remove(&safe_name));
        if removed.is_none() {
            return Err(AppError::not_found("mcp server not found"));
        }
        let serialized = toml::to_string_pretty(&doc)
            .map_err(|e| AppError::internal(format!("failed to serialize codex toml: {e}")))?;
        std::fs::write(&path, serialized.as_bytes())
            .map_err(|e| AppError::internal(format!("failed to write codex config: {e}")))?;
    } else {
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| AppError::internal(format!("failed to read mcp json: {e}")))?;
        let mut root: JsonMap<String, JsonValue> = serde_json::from_str(&raw)
            .map_err(|e| AppError::bad_request(format!("invalid mcp json: {e}")))?;
        let removed = root
            .get_mut("mcpServers")
            .and_then(|v| v.as_object_mut())
            .and_then(|obj| obj.remove(&safe_name));
        if removed.is_none() {
            return Err(AppError::not_found("mcp server not found"));
        }
        let serialized = serde_json::to_string_pretty(&JsonValue::Object(root))
            .map_err(|e| AppError::internal(format!("failed to serialize mcp json: {e}")))?;
        std::fs::write(&path, format!("{serialized}\n").as_bytes())
            .map_err(|e| AppError::internal(format!("failed to write mcp json: {e}")))?;
    }

    Ok(())
}

fn sync_mcp_server_across_providers(
    data_dir: &StdPath,
    workspace_path: Option<&StdPath>,
    source_provider: CliProvider,
    scope: ConfigScope,
    name: &str,
    target_providers: Vec<CliProvider>,
) -> Result<SyncMcpServerResponse, AppError> {
    let safe_name = validate_entry_name(name)?;
    let source_cfg = read_mcp_config(data_dir, workspace_path, source_provider, scope)?;
    let server = source_cfg
        .servers
        .into_iter()
        .find(|s| s.name == safe_name)
        .ok_or_else(|| AppError::not_found("mcp server not found in source provider"))?;

    let targets: Vec<CliProvider> = if target_providers.is_empty() {
        CliProvider::ALL
            .into_iter()
            .filter(|p| *p != source_provider)
            .collect()
    } else {
        target_providers
    };

    let mut synced = Vec::new();
    for target in targets {
        upsert_mcp_server_in_config(data_dir, workspace_path, target, scope, server.clone())?;
        synced.push(target);
    }

    Ok(SyncMcpServerResponse {
        server,
        synced_providers: synced,
    })
}

async fn list_mcp_presets(_auth: AuthUser) -> Result<Json<Vec<McpPresetDto>>, AppError> {
    Ok(Json(built_in_mcp_presets()))
}

async fn get_global_mcp_config(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(provider): Path<CliProvider>,
) -> Result<Json<ProviderMcpConfigDto>, AppError> {
    Ok(Json(read_mcp_config(
        &state.config.data_dir,
        None,
        provider,
        ConfigScope::Global,
    )?))
}

async fn upsert_global_mcp_server(
    State(state): State<AppState>,
    admin: AdminUser,
    Path((provider, name)): Path<(CliProvider, String)>,
    Json(req): Json<UpsertMcpServerRequest>,
) -> Result<Json<ProviderMcpConfigDto>, AppError> {
    let safe_name = validate_entry_name(&name)?;
    let srv = McpServerDto {
        name: safe_name.clone(),
        command: req.command.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        args: req.args,
        env: req.env,
        url: req.url.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        disabled: req.disabled,
    };
    let cfg = upsert_mcp_server_in_config(
        &state.config.data_dir,
        None,
        provider,
        ConfigScope::Global,
        srv,
    )?;
    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "upsert_global_mcp",
        Some("mcp_server"),
        Some(&safe_name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(Json(cfg))
}

async fn delete_global_mcp_server(
    State(state): State<AppState>,
    admin: AdminUser,
    Path((provider, name)): Path<(CliProvider, String)>,
) -> Result<StatusCode, AppError> {
    delete_mcp_server_in_config(&state.config.data_dir, None, provider, &name)?;
    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "delete_global_mcp",
        Some("mcp_server"),
        Some(&name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn sync_global_mcp_server(
    State(state): State<AppState>,
    admin: AdminUser,
    Path((provider, name)): Path<(CliProvider, String)>,
    Json(req): Json<SyncMcpServerRequest>,
) -> Result<Json<SyncMcpServerResponse>, AppError> {
    let resp = sync_mcp_server_across_providers(
        &state.config.data_dir,
        None,
        provider,
        ConfigScope::Global,
        &name,
        req.target_providers,
    )?;
    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "sync_global_mcp",
        Some("mcp_server"),
        Some(&name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(Json(resp))
}

async fn apply_global_mcp_preset(
    State(state): State<AppState>,
    admin: AdminUser,
    Path((provider, preset_id)): Path<(CliProvider, String)>,
) -> Result<Json<ProviderMcpConfigDto>, AppError> {
    let preset = built_in_mcp_presets()
        .into_iter()
        .find(|p| p.id == preset_id)
        .ok_or_else(|| AppError::not_found("mcp preset not found"))?;
    let cfg = upsert_mcp_server_in_config(
        &state.config.data_dir,
        None,
        provider,
        ConfigScope::Global,
        preset.server,
    )?;
    record_audit_log(
        &state,
        Some(&admin.0.user.id),
        "apply_global_mcp_preset",
        Some("mcp_preset"),
        Some(&preset_id),
        Some(provider.as_str()),
    )
    .await?;
    Ok(Json(cfg))
}

async fn get_project_mcp_config(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider)): Path<(String, CliProvider)>,
) -> Result<Json<ProviderMcpConfigDto>, AppError> {
    Ok(Json(read_mcp_config(
        &state.config.data_dir,
        Some(StdPath::new(&ctx.project.workspace_path)),
        provider,
        ConfigScope::Project,
    )?))
}

async fn upsert_project_mcp_server(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider, name)): Path<(String, CliProvider, String)>,
    Json(req): Json<UpsertMcpServerRequest>,
) -> Result<Json<ProviderMcpConfigDto>, AppError> {
    let safe_name = validate_entry_name(&name)?;
    let srv = McpServerDto {
        name: safe_name.clone(),
        command: req.command.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        args: req.args,
        env: req.env,
        url: req.url.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        disabled: req.disabled,
    };
    let cfg = upsert_mcp_server_in_config(
        &state.config.data_dir,
        Some(StdPath::new(&ctx.project.workspace_path)),
        provider,
        ConfigScope::Project,
        srv,
    )?;
    record_audit_log(
        &state,
        Some(&ctx.user.user.id),
        "upsert_project_mcp",
        Some("mcp_server"),
        Some(&safe_name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(Json(cfg))
}

async fn delete_project_mcp_server(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider, name)): Path<(String, CliProvider, String)>,
) -> Result<StatusCode, AppError> {
    delete_mcp_server_in_config(
        &state.config.data_dir,
        Some(StdPath::new(&ctx.project.workspace_path)),
        provider,
        &name,
    )?;
    record_audit_log(
        &state,
        Some(&ctx.user.user.id),
        "delete_project_mcp",
        Some("mcp_server"),
        Some(&name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn sync_project_mcp_server(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider, name)): Path<(String, CliProvider, String)>,
    Json(req): Json<SyncMcpServerRequest>,
) -> Result<Json<SyncMcpServerResponse>, AppError> {
    let resp = sync_mcp_server_across_providers(
        &state.config.data_dir,
        Some(StdPath::new(&ctx.project.workspace_path)),
        provider,
        ConfigScope::Project,
        &name,
        req.target_providers,
    )?;
    record_audit_log(
        &state,
        Some(&ctx.user.user.id),
        "sync_project_mcp",
        Some("mcp_server"),
        Some(&name),
        Some(provider.as_str()),
    )
    .await?;
    Ok(Json(resp))
}

async fn apply_project_mcp_preset(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, provider, preset_id)): Path<(String, CliProvider, String)>,
) -> Result<Json<ProviderMcpConfigDto>, AppError> {
    let mut preset = built_in_mcp_presets()
        .into_iter()
        .find(|p| p.id == preset_id)
        .ok_or_else(|| AppError::not_found("mcp preset not found"))?;
    let workspace = StdPath::new(&ctx.project.workspace_path);
    if preset_id == GRAPHIFY_PRESET_ID {
        let graph_path = std::path::absolute(workspace.join(GRAPHIFY_GRAPH_PATH))
            .map_err(|e| AppError::internal(format!("resolve Graphify graph path: {e}")))?;
        preset.server.args = vec![graph_path.to_string_lossy().into_owned()];
    }
    let cfg = upsert_mcp_server_in_config(
        &state.config.data_dir,
        Some(workspace),
        provider,
        ConfigScope::Project,
        preset.server,
    )?;
    record_audit_log(
        &state,
        Some(&ctx.user.user.id),
        "apply_project_mcp_preset",
        Some("mcp_preset"),
        Some(&preset_id),
        Some(provider.as_str()),
    )
    .await?;
    Ok(Json(cfg))
}

async fn run_project_graphify_index(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path(_project_id): Path<String>,
) -> Result<Json<GraphifyIndexResponse>, AppError> {
    let row = sqlx::query("SELECT active_provider FROM project_active_cli WHERE project_id = ?")
        .bind(&ctx.project.id)
        .fetch_optional(&state.db)
        .await?;
    let provider = match row {
        Some(r) => {
            let p: String = r.try_get("active_provider")?;
            p.parse().unwrap_or(CliProvider::ClaudeCode)
        }
        None => CliProvider::ClaudeCode,
    };

    let invocation = CliInvocation {
        provider,
        program: "graphify".to_string(),
        args: vec![
            "extract".to_string(),
            ".".to_string(),
            "--code-only".to_string(),
        ],
        working_dir: PathBuf::from(&ctx.project.workspace_path),
        env: BTreeMap::new(),
    };

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let (_approval_tx, approval_rx) = mpsc::unbounded_channel();

    state
        .cli_runner
        .run_turn(invocation, event_tx, approval_rx)
        .await
        .map_err(|e| AppError::internal(format!("graphify indexing failed: {e}")))?;

    let mut output = String::new();
    let mut status = "completed".to_string();
    while let Some(ev) = event_rx.recv().await {
        match ev {
            TurnEvent::TextDelta { text } => output.push_str(&text),
            TurnEvent::TurnComplete { result_text, .. } => {
                if !result_text.is_empty() {
                    if !output.is_empty() && !output.ends_with('\n') {
                        output.push('\n');
                    }
                    output.push_str(&result_text);
                }
            }
            TurnEvent::TurnError { message } => {
                status = "error".to_string();
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str(&message);
            }
            _ => {}
        }
    }

    record_audit_log(
        &state,
        Some(&ctx.user.user.id),
        "run_graphify_index",
        Some("project"),
        Some(&ctx.project.id),
        Some(GRAPHIFY_GRAPH_PATH),
    )
    .await?;

    Ok(Json(GraphifyIndexResponse {
        project_id: ctx.project.id,
        graph_path: GRAPHIFY_GRAPH_PATH.to_string(),
        status,
        output,
    }))
}
