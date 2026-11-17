use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use claude_web_ui_server::fakes::{FakeCliRunner, FakeMessengerTransport};
use claude_web_ui_server::{GatewayConfig, GatewayServer};
use claude_web_ui_server::traits::TurnEvent;
use claude_web_ui_shared::accounts::CliProvider;
use claude_web_ui_shared::agents::{
    ConfigScope, GraphifyIndexResponse, McpPresetDto, ProviderMcpConfigDto, SkillDto, SubagentDto,
    SyncMcpServerResponse,
};
use claude_web_ui_shared::auth::{AuthTokensResponse, InviteDto, ProjectDto};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

struct TestHarness {
    _tmp: TempDir,
    data_dir: PathBuf,
    cli_runner: Arc<FakeCliRunner>,
    server: GatewayServer,
}

impl TestHarness {
    async fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let data_dir = tmp.path().join("data");
        let mut config = GatewayConfig::for_data_dir(&data_dir);
        config.jwt_secret = "test-jwt-secret-key-at-least-32-bytes-long".to_string();
        let cli_runner = Arc::new(FakeCliRunner::new());
        let server = GatewayServer::new(
            config,
            cli_runner.clone(),
            Arc::new(FakeMessengerTransport::new()),
        )
        .await
        .expect("gateway server init");
        Self {
            _tmp: tmp,
            data_dir,
            cli_runner,
            server,
        }
    }

    async fn request_json(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(tok) = token {
            builder = builder.header("authorization", format!("Bearer {tok}"));
        }
        let req_body = match body {
            Some(val) => {
                builder = builder.header("content-type", "application/json");
                Body::from(val.to_string())
            }
            None => Body::empty(),
        };
        let req = builder.body(req_body).expect("request");
        let resp = self
            .server
            .router()
            .oneshot(req)
            .await
            .expect("router response");
        let status = resp.status();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        let val = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        (status, val)
    }

    async fn decode<T: DeserializeOwned>(val: Value) -> T {
        serde_json::from_value(val).expect("decode json response")
    }

    async fn bootstrap_admin(&self) -> AuthTokensResponse {
        let (status, body) = self
            .request_json(
                Method::POST,
                "/api/auth/bootstrap",
                None,
                Some(json!({
                    "username": "admin",
                    "email": "admin@example.com",
                    "password": "correct-horse-battery"
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        Self::decode(body).await
    }

    async fn create_project(&self, admin_token: &str, name: &str) -> ProjectDto {
        let ws = self
            ._tmp
            .path()
            .join(format!("ws-{}", name.to_lowercase().replace(' ', "-")));
        std::fs::create_dir_all(&ws).unwrap();
        let (status, body) = self
            .request_json(
                Method::POST,
                "/api/projects",
                Some(admin_token),
                Some(json!({
                    "name": name,
                    "workspace_path": ws.to_string_lossy()
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        Self::decode(body).await
    }

    async fn invite_and_redeem(
        &self,
        admin_token: &str,
        role: &str,
        username: &str,
        project_ids: Vec<String>,
    ) -> AuthTokensResponse {
        let (status, inv_body) = self
            .request_json(
                Method::POST,
                "/api/admin/invites",
                Some(admin_token),
                Some(json!({
                    "role": role,
                    "project_ids": project_ids
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        let invite: InviteDto = Self::decode(inv_body).await;

        let (status, auth_body) = self
            .request_json(
                Method::POST,
                "/api/auth/invites/redeem",
                None,
                Some(json!({
                    "code": invite.code,
                    "username": username,
                    "email": format!("{username}@example.com"),
                    "password": "password-123"
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        Self::decode(auth_body).await
    }
}

#[tokio::test]
async fn subagents_crud_roundtrip_yaml_frontmatter_and_rbac_across_providers() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;
    let alpha = h.create_project(&admin.access_token, "Alpha").await;
    let beta = h.create_project(&admin.access_token, "Beta").await;
    let dev = h
        .invite_and_redeem(&admin.access_token, "developer", "dev1", vec![alpha.id.clone()])
        .await;
    let viewer = h
        .invite_and_redeem(&admin.access_token, "viewer", "viewer1", vec![alpha.id.clone()])
        .await;

    // 1. Developer & Viewer are forbidden from mutating Global agents
    for tok in [&dev.access_token, &viewer.access_token] {
        let (status, _) = h
            .request_json(
                Method::POST,
                "/api/global/agents/claude_code",
                Some(tok),
                Some(json!({
                    "name": "architect",
                    "description": "Blocked",
                    "model": "opus",
                    "system_prompt": "Nope"
                })),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    // 2. Admin creates a Global Claude Code subagent using the structured visual payload
    let (status, body) = h
        .request_json(
            Method::POST,
            "/api/global/agents/claude_code",
            Some(&admin.access_token),
            Some(json!({
                "name": "architect",
                "description": "High-level architecture reviewer",
                "model": "opus",
                "tools": ["Read", "Grep", "Glob"],
                "disallowed_tools": ["Write", "Edit"],
                "permission_mode": "plan",
                "system_prompt": "Analyze architecture carefully before proposing changes."
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let global_claude: SubagentDto = TestHarness::decode(body).await;
    assert_eq!(global_claude.name, "architect");
    assert_eq!(global_claude.provider, CliProvider::ClaudeCode);
    assert_eq!(global_claude.scope, ConfigScope::Global);
    assert_eq!(global_claude.model.as_deref(), Some("opus"));
    assert_eq!(global_claude.tools, vec!["Read", "Grep", "Glob"]);
    assert_eq!(global_claude.disallowed_tools, vec!["Write", "Edit"]);
    assert_eq!(global_claude.permission_mode.as_deref(), Some("plan"));

    let expected_global_path = h
        .data_dir
        .join("shared")
        .join("claude_code")
        .join("agents")
        .join("architect.md");
    assert!(expected_global_path.is_file());
    let disk_md = std::fs::read_to_string(&expected_global_path).unwrap();
    assert!(disk_md.starts_with("---\n"));
    assert!(disk_md.contains("disallowedTools:"));
    assert!(disk_md.contains("permissionMode: plan"));
    assert!(disk_md.contains("Analyze architecture carefully before proposing changes."));

    // Developer & Viewer can read Global agents
    for tok in [&dev.access_token, &viewer.access_token] {
        let (status, body) = h
            .request_json(Method::GET, "/api/global/agents/claude_code", Some(tok), None)
            .await;
        assert_eq!(status, StatusCode::OK);
        let list: Vec<SubagentDto> = TestHarness::decode(body).await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "architect");
    }

    // 3. Developer creates Project-scoped subagents on assigned project Alpha across Claude, Codex, Antigravity
    let (status, body) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/agents/claude_code", alpha.id),
            Some(&dev.access_token),
            Some(json!({
                "name": "coder",
                "description": "Fast feature implementer",
                "model": "sonnet",
                "tools": ["Read", "Edit", "Write", "Bash"],
                "disallowed_tools": [],
                "permission_mode": "acceptEdits",
                "system_prompt": "Implement features with clean tests."
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let proj_claude: SubagentDto = TestHarness::decode(body).await;
    assert_eq!(proj_claude.scope, ConfigScope::Project);

    let alpha_ws = PathBuf::from(&alpha.workspace_path);
    assert!(alpha_ws.join(".claude").join("agents").join("coder.md").is_file());

    // Codex project agent
    let (status, _) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/agents/codex", alpha.id),
            Some(&dev.access_token),
            Some(json!({
                "name": "reviewer",
                "description": "PR reviewer",
                "model": "gpt-5-codex",
                "tools": ["Read", "Grep"],
                "system_prompt": "Review diffs."
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(alpha_ws.join(".codex").join("agents").join("reviewer.md").is_file());

    // Antigravity project agent with `flash_lite` model tier via raw Markdown/YAML round-trip
    let raw_agy = "---\nname: collector\ndescription: Fast log and metric collector\nmodel: flash_lite\ntools: Read, Bash\ndisallowedTools:\n  - Write\npermissionMode: dontAsk\n---\nCollect logs quickly and summarize anomalies.\n";
    let (status, body) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/agents/antigravity", alpha.id),
            Some(&dev.access_token),
            Some(json!({
                "name": "collector",
                "raw_markdown": raw_agy
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let agy_agent: SubagentDto = TestHarness::decode(body).await;
    assert_eq!(agy_agent.provider, CliProvider::Antigravity);
    assert_eq!(agy_agent.model.as_deref(), Some("flash_lite"));
    assert_eq!(agy_agent.tools, vec!["Read", "Bash"]);
    assert_eq!(agy_agent.disallowed_tools, vec!["Write"]);
    assert_eq!(agy_agent.permission_mode.as_deref(), Some("dontAsk"));
    assert_eq!(
        agy_agent.system_prompt.trim(),
        "Collect logs quickly and summarize anomalies."
    );
    assert!(alpha_ws.join(".agents").join("agents").join("collector.md").is_file());

    // Update via PUT (raw_markdown -> visual fields round-trip)
    let updated_raw = "---\nname: collector\ndescription: Updated collector\nmodel: pro\ntools:\n  - Read\n  - Grep\npermissionMode: default\n---\nUpdated prompt body.\n";
    let (status, body) = h
        .request_json(
            Method::PUT,
            &format!("/api/projects/{}/agents/antigravity/collector", alpha.id),
            Some(&dev.access_token),
            Some(json!({
                "name": "collector",
                "raw_markdown": updated_raw
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let updated: SubagentDto = TestHarness::decode(body).await;
    assert_eq!(updated.model.as_deref(), Some("pro"));
    assert_eq!(updated.tools, vec!["Read", "Grep"]);
    assert_eq!(updated.system_prompt.trim(), "Updated prompt body.");

    // 4. Path traversal / unsafe names rejected with 400
    let (status, _) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/agents/claude_code", alpha.id),
            Some(&dev.access_token),
            Some(json!({
                "name": "../escape",
                "description": "bad",
                "system_prompt": "bad"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 5. Viewer can GET project agents, but gets 403 on POST/PUT/DELETE
    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/agents/antigravity/collector", alpha.id),
            Some(&viewer.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let fetched: SubagentDto = TestHarness::decode(body).await;
    assert_eq!(fetched.model.as_deref(), Some("pro"));

    let (status, _) = h
        .request_json(
            Method::PUT,
            &format!("/api/projects/{}/agents/antigravity/collector", alpha.id),
            Some(&viewer.access_token),
            Some(json!({ "name": "collector", "system_prompt": "blocked" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = h
        .request_json(
            Method::DELETE,
            &format!("/api/projects/{}/agents/antigravity/collector", alpha.id),
            Some(&viewer.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Developer cannot access or mutate unassigned project Beta
    let (status, _) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/agents/claude_code", beta.id),
            Some(&dev.access_token),
            Some(json!({ "name": "coder", "system_prompt": "blocked" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Developer deletes project agent, Admin deletes global agent
    let (status, _) = h
        .request_json(
            Method::DELETE,
            &format!("/api/projects/{}/agents/antigravity/collector", alpha.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!alpha_ws.join(".agents").join("agents").join("collector.md").exists());

    let (status, _) = h
        .request_json(
            Method::DELETE,
            "/api/global/agents/claude_code/architect",
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!expected_global_path.exists());
}

#[tokio::test]
async fn skills_crud_companion_files_and_cross_cli_share_toggle_with_rbac() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;
    let alpha = h.create_project(&admin.access_token, "Alpha").await;
    let dev = h
        .invite_and_redeem(&admin.access_token, "developer", "dev1", vec![alpha.id.clone()])
        .await;
    let viewer = h
        .invite_and_redeem(&admin.access_token, "viewer", "viewer1", vec![alpha.id.clone()])
        .await;

    let skill_md = "---\nname: tdd-flow\ndescription: Red-Green-Refactor vertical TDD loop\n---\n# TDD Flow\nWrite a failing test first.\n";

    // Developer cannot mutate Global skills
    let (status, _) = h
        .request_json(
            Method::POST,
            "/api/global/skills/claude_code",
            Some(&dev.access_token),
            Some(json!({ "name": "tdd-flow", "skill_md": skill_md })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Developer creates a project skill with companion files in scripts/, references/, examples/
    let (status, body) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/skills/claude_code", alpha.id),
            Some(&dev.access_token),
            Some(json!({
                "name": "tdd-flow",
                "skill_md": skill_md,
                "companion_files": [
                    { "relative_path": "scripts/check.sh", "content": "#!/bin/sh\ncargo test\n" },
                    { "relative_path": "references/rules.md", "content": "Always assert at seams.\n" },
                    { "relative_path": "examples/sample.txt", "content": "red -> green\n" }
                ]
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let created: SkillDto = TestHarness::decode(body).await;
    assert_eq!(created.name, "tdd-flow");
    assert_eq!(created.provider, CliProvider::ClaudeCode);
    assert_eq!(created.scope, ConfigScope::Project);
    assert_eq!(created.description, "Red-Green-Refactor vertical TDD loop");
    assert!(!created.is_shared);
    assert_eq!(created.companion_files.len(), 3);

    let alpha_ws = PathBuf::from(&alpha.workspace_path);
    let claude_skill_dir = alpha_ws.join(".claude").join("skills").join("tdd-flow");
    assert!(claude_skill_dir.join("SKILL.md").is_file());
    assert!(claude_skill_dir.join("scripts").join("check.sh").is_file());
    assert!(claude_skill_dir.join("references").join("rules.md").is_file());
    assert!(claude_skill_dir.join("examples").join("sample.txt").is_file());

    // Path traversal or non-companion dir rejected on companion file write
    for bad_rel in ["../escape.txt", "/etc/passwd", "other/file.txt", "scripts/../../bad"] {
        let (status, _) = h
            .request_json(
                Method::PUT,
                &format!(
                    "/api/projects/{}/skills/claude_code/tdd-flow/files",
                    alpha.id
                ),
                Some(&dev.access_token),
                Some(json!({ "relative_path": bad_rel, "content": "nope" })),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "expected 400 for {bad_rel}");
    }

    // Update companion file and delete one companion file
    let (status, body) = h
        .request_json(
            Method::PUT,
            &format!(
                "/api/projects/{}/skills/claude_code/tdd-flow/files",
                alpha.id
            ),
            Some(&dev.access_token),
            Some(json!({
                "relative_path": "scripts/check.sh",
                "content": "#!/bin/sh\ncargo test --workspace\n"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let updated: SkillDto = TestHarness::decode(body).await;
    assert!(updated
        .companion_files
        .iter()
        .any(|f| f.relative_path == "scripts/check.sh" && f.content.contains("--workspace")));

    let (status, body) = h
        .request_json(
            Method::DELETE,
            &format!(
                "/api/projects/{}/skills/claude_code/tdd-flow/files?path=examples/sample.txt",
                alpha.id
            ),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let after_del: SkillDto = TestHarness::decode(body).await;
    assert_eq!(after_del.companion_files.len(), 2);
    assert!(!claude_skill_dir.join("examples").join("sample.txt").exists());

    // Viewer cannot toggle share or edit skills
    let (status, _) = h
        .request_json(
            Method::PUT,
            &format!(
                "/api/projects/{}/skills/claude_code/tdd-flow/share",
                alpha.id
            ),
            Some(&viewer.access_token),
            Some(json!({ "shared": true })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Toggle "Share Skill across CLIs" ON
    let (status, body) = h
        .request_json(
            Method::PUT,
            &format!(
                "/api/projects/{}/skills/claude_code/tdd-flow/share",
                alpha.id
            ),
            Some(&dev.access_token),
            Some(json!({ "shared": true })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let shared_dto: SkillDto = TestHarness::decode(body).await;
    assert!(shared_dto.is_shared);
    assert_eq!(shared_dto.shared_providers.len(), 3);

    let codex_skill_dir = alpha_ws.join(".codex").join("skills").join("tdd-flow");
    let agy_skill_dir = alpha_ws.join(".agents").join("skills").join("tdd-flow");
    assert!(codex_skill_dir.join("SKILL.md").is_file());
    assert!(agy_skill_dir.join("SKILL.md").is_file());
    assert!(codex_skill_dir.join("scripts").join("check.sh").is_file());

    // Reading from Codex sees the shared skill
    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/skills/codex/tdd-flow", alpha.id),
            Some(&viewer.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let codex_view: SkillDto = TestHarness::decode(body).await;
    assert!(codex_view.is_shared);

    // Toggle "Share Skill across CLIs" OFF -> removes cross-CLI symlinks/junctions without deleting Claude's files
    let (status, body) = h
        .request_json(
            Method::PUT,
            &format!(
                "/api/projects/{}/skills/claude_code/tdd-flow/share",
                alpha.id
            ),
            Some(&dev.access_token),
            Some(json!({ "shared": false })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let unshared: SkillDto = TestHarness::decode(body).await;
    assert!(!unshared.is_shared);
    assert!(!codex_skill_dir.exists());
    assert!(!agy_skill_dir.exists());
    assert!(claude_skill_dir.join("SKILL.md").is_file());
    assert!(claude_skill_dir.join("scripts").join("check.sh").is_file());

    // Global skill create + cross-CLI share by Admin
    let (status, _) = h
        .request_json(
            Method::POST,
            "/api/global/skills/codex",
            Some(&admin.access_token),
            Some(json!({
                "name": "global-audit",
                "skill_md": "---\nname: global-audit\ndescription: Global security audit\n---\nAudit everything.\n"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, body) = h
        .request_json(
            Method::PUT,
            "/api/global/skills/codex/global-audit/share",
            Some(&admin.access_token),
            Some(json!({ "shared": true })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let global_shared: SkillDto = TestHarness::decode(body).await;
    assert!(global_shared.is_shared);
    assert!(h
        .data_dir
        .join("shared")
        .join("claude_code")
        .join("skills")
        .join("global-audit")
        .join("SKILL.md")
        .is_file());
    assert!(h
        .data_dir
        .join("shared")
        .join("antigravity")
        .join("skills")
        .join("global-audit")
        .join("SKILL.md")
        .is_file());

    // Deleting canonical skill unlinks shared symlinks and removes canonical directory
    let (status, _) = h
        .request_json(
            Method::DELETE,
            "/api/global/skills/codex/global-audit",
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!h
        .data_dir
        .join("shared")
        .join("codex")
        .join("skills")
        .join("global-audit")
        .exists());
    assert!(!h
        .data_dir
        .join("shared")
        .join("claude_code")
        .join("skills")
        .join("global-audit")
        .exists());
}

#[tokio::test]
async fn mcp_manager_native_formats_presets_cross_cli_sync_graphify_index_and_rbac() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;
    let alpha = h.create_project(&admin.access_token, "Alpha").await;
    let dev = h
        .invite_and_redeem(&admin.access_token, "developer", "dev1", vec![alpha.id.clone()])
        .await;
    let viewer = h
        .invite_and_redeem(&admin.access_token, "viewer", "viewer1", vec![alpha.id.clone()])
        .await;

    // 1. Presets catalog
    let (status, body) = h
        .request_json(Method::GET, "/api/mcp/presets", Some(&viewer.access_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let presets: Vec<McpPresetDto> = TestHarness::decode(body).await;
    assert!(presets.iter().any(|p| p.id == "graphify-mcp"));
    assert!(presets.iter().any(|p| p.id == "chrome-devtools"));
    assert!(presets.iter().any(|p| p.id == "playwright"));

    // Seed existing config files to verify unrelated keys are preserved
    let alpha_ws = PathBuf::from(&alpha.workspace_path);
    std::fs::write(alpha_ws.join(".mcp.json"), r#"{"customKey": true}"#).unwrap();
    std::fs::create_dir_all(alpha_ws.join(".codex")).unwrap();
    std::fs::write(
        alpha_ws.join(".codex").join("config.toml"),
        "model = \"gpt-5-codex\"\n",
    )
    .unwrap();

    // 2. Developer applies graphify-mcp preset to Claude Code in Project scope
    let (status, body) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/mcp/claude_code/presets/graphify-mcp", alpha.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let claude_cfg: ProviderMcpConfigDto = TestHarness::decode(body).await;
    assert_eq!(claude_cfg.provider, CliProvider::ClaudeCode);
    assert_eq!(claude_cfg.scope, ConfigScope::Project);
    assert_eq!(claude_cfg.servers.len(), 1);
    assert_eq!(claude_cfg.servers[0].name, "graphify");
    assert_eq!(
        claude_cfg.servers[0].command.as_deref(),
        Some("graphify-mcp")
    );
    assert_eq!(
        claude_cfg.servers[0].args,
        vec!["--graph", "graphify-out/graph.json"]
    );

    let raw_mcp_json: Value =
        serde_json::from_str(&std::fs::read_to_string(alpha_ws.join(".mcp.json")).unwrap()).unwrap();
    assert_eq!(raw_mcp_json["customKey"], json!(true));
    assert_eq!(
        raw_mcp_json["mcpServers"]["graphify"]["command"],
        json!("graphify-mcp")
    );

    // 3. Developer applies chrome-devtools preset to Codex (TOML)
    let (status, body) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/mcp/codex/presets/chrome-devtools", alpha.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let codex_cfg: ProviderMcpConfigDto = TestHarness::decode(body).await;
    assert_eq!(codex_cfg.servers.len(), 1);
    assert_eq!(codex_cfg.servers[0].name, "chrome-devtools");

    let raw_codex_toml =
        std::fs::read_to_string(alpha_ws.join(".codex").join("config.toml")).unwrap();
    assert!(raw_codex_toml.contains("model = \"gpt-5-codex\""));
    assert!(raw_codex_toml.contains("mcp_servers"));
    assert!(raw_codex_toml.contains("chrome-devtools"));

    // 4. Upsert custom/playwright server on Antigravity (.agents/mcp.json)
    let (status, body) = h
        .request_json(
            Method::PUT,
            &format!("/api/projects/{}/mcp/antigravity/custom-web", alpha.id),
            Some(&dev.access_token),
            Some(json!({
                "name": "custom-web",
                "command": "npx",
                "args": ["-y", "@playwright/mcp@latest"],
                "env": { "HEADLESS": "1" },
                "disabled": false
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let agy_cfg: ProviderMcpConfigDto = TestHarness::decode(body).await;
    assert_eq!(agy_cfg.servers.len(), 1);
    assert!(alpha_ws.join(".agents").join("mcp.json").is_file());

    // 5. Cross-CLI sync `graphify` from Claude Code to Codex and Antigravity
    let (status, body) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/mcp/claude_code/graphify/sync", alpha.id),
            Some(&dev.access_token),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let sync_resp: SyncMcpServerResponse = TestHarness::decode(body).await;
    assert_eq!(sync_resp.server.name, "graphify");
    assert!(sync_resp.synced_providers.contains(&CliProvider::Codex));
    assert!(sync_resp
        .synced_providers
        .contains(&CliProvider::Antigravity));

    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/mcp/codex", alpha.id),
            Some(&viewer.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let codex_after_sync: ProviderMcpConfigDto = TestHarness::decode(body).await;
    assert!(codex_after_sync.servers.iter().any(|s| s.name == "graphify"));
    assert!(codex_after_sync
        .servers
        .iter()
        .any(|s| s.name == "chrome-devtools"));

    // Delete chrome-devtools from Codex while preserving graphify & model key
    let (status, _) = h
        .request_json(
            Method::DELETE,
            &format!("/api/projects/{}/mcp/codex/chrome-devtools", alpha.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let raw_codex_after_del =
        std::fs::read_to_string(alpha_ws.join(".codex").join("config.toml")).unwrap();
    assert!(raw_codex_after_del.contains("model = \"gpt-5-codex\""));
    assert!(!raw_codex_after_del.contains("chrome-devtools"));
    assert!(raw_codex_after_del.contains("graphify"));

    // 6. Global MCP RBAC & .claude.json persistence
    let (status, _) = h
        .request_json(
            Method::POST,
            "/api/global/mcp/claude_code/presets/playwright",
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = h
        .request_json(
            Method::POST,
            "/api/global/mcp/claude_code/presets/playwright",
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(h
        .data_dir
        .join("shared")
        .join("claude_code")
        .join(".claude.json")
        .is_file());

    // 7. Viewer is blocked from project MCP mutations and graphify indexing
    let (status, _) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/mcp/graphify/index", alpha.id),
            Some(&viewer.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 8. Developer runs graphify indexing via CliRunner (FakeCliRunner)
    h.cli_runner.push_turn(vec![
        TurnEvent::TextDelta {
            text: "Scanning workspace...\n".to_string(),
        },
        TurnEvent::TurnComplete {
            session_id: None,
            result_text: "Indexed 42 files into graphify-out/graph.json".to_string(),
        },
    ]);

    let (status, body) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/mcp/graphify/index", alpha.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let idx_resp: GraphifyIndexResponse = TestHarness::decode(body).await;
    assert_eq!(idx_resp.project_id, alpha.id);
    assert_eq!(idx_resp.graph_path, "graphify-out/graph.json");
    assert_eq!(idx_resp.status, "completed");
    assert!(idx_resp
        .output
        .contains("Indexed 42 files into graphify-out/graph.json"));

    let invocations = h.cli_runner.recorded_invocations();
    assert_eq!(invocations.len(), 1);
    assert_eq!(invocations[0].program, "graphify");
    assert_eq!(
        invocations[0].working_dir,
        PathBuf::from(&alpha.workspace_path)
    );
    assert!(invocations[0]
        .args
        .iter()
        .any(|a| a.contains("graphify-out/graph.json")));
}

#[tokio::test]
async fn embedded_ui_serves_agents_skills_mcp_manager_and_presets() {
    let h = TestHarness::new().await;

    let html_resp = h
        .server
        .router()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(html_resp.status(), StatusCode::OK);
    let html_bytes = html_resp.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8_lossy(&html_bytes);
    assert!(html.contains("Claude Web UI"));

    let js_resp = h
        .server
        .router()
        .oneshot(Request::builder().uri("/app.js").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(js_resp.status(), StatusCode::OK);
    let js_bytes = js_resp.into_body().collect().await.unwrap().to_bytes();
    let js = String::from_utf8_lossy(&js_bytes);
    assert!(js.contains("Agents, Skills &amp; MCP") || js.contains("Agents, Skills & MCP"));
    assert!(js.contains("Share Skill across CLIs"));
    assert!(js.contains("graphify-mcp"));
    assert!(js.contains("chrome-devtools"));
    assert!(js.contains("Run Graphify Index"));
    assert!(js.contains("/mcp/graphify/index"));
    assert!(js.contains("raw_markdown"));
}
