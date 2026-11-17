use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use claude_web_ui_server::fakes::{FakeCliRunner, FakeMessengerTransport};
use claude_web_ui_server::{fs_link, GatewayConfig, GatewayServer};
use claude_web_ui_shared::auth::{AuthTokensResponse, InviteDto, ProjectDto, Role};
use claude_web_ui_shared::files::{
    DiffLineKind, DiffViewMode, FileContentResponse, FileTreeNode, FileTreeResponse,
    GitCommitsResponse, GitDiffResponse, GitStatusResponse, SaveFileResponse,
};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

struct TestHarness {
    tmp: TempDir,
    server: GatewayServer,
}

impl TestHarness {
    async fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let mut config = GatewayConfig::for_data_dir(tmp.path().join("data"));
        config.jwt_secret = "test-jwt-secret-key-at-least-32-bytes-long".to_string();
        let server = GatewayServer::new(
            config,
            Arc::new(FakeCliRunner::new()),
            Arc::new(FakeMessengerTransport::new()),
        )
        .await
        .expect("gateway server init");
        Self { tmp, server }
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
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                Value::String(String::from_utf8_lossy(&bytes).into_owned())
            })
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

    async fn create_project(&self, admin_token: &str, name: &str, workspace_path: &Path) -> ProjectDto {
        let (status, body) = self
            .request_json(
                Method::POST,
                "/api/projects",
                Some(admin_token),
                Some(json!({
                    "name": name,
                    "workspace_path": workspace_path.to_string_lossy()
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        Self::decode(body).await
    }

    async fn invite_and_redeem(
        &self,
        admin_token: &str,
        role: Role,
        project_ids: Vec<String>,
        username: &str,
    ) -> AuthTokensResponse {
        let (status, body) = self
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
        let invite: InviteDto = Self::decode(body).await;

        let (status, body) = self
            .request_json(
                Method::POST,
                "/api/auth/invites/redeem",
                None,
                Some(json!({
                    "code": invite.code,
                    "username": username,
                    "email": format!("{username}@example.com"),
                    "password": "secure-password-123"
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        Self::decode(body).await
    }

    fn create_workspace(&self, name: &str) -> PathBuf {
        let dir = self.tmp.path().join(name);
        std::fs::create_dir_all(&dir).expect("create workspace dir");
        dir
    }
}

fn collect_paths(nodes: &[FileTreeNode], out: &mut Vec<String>) {
    for n in nodes {
        out.push(n.path.clone());
        collect_paths(&n.children, out);
    }
}

#[tokio::test]
async fn directory_tree_respects_gitignore_toggle_and_blocks_path_traversal_and_symlink_escape() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;
    let ws = h.create_workspace("repo-tree");
    let outside = h.create_workspace("outside-secret");
    std::fs::write(outside.join("secret.txt"), "top-secret").unwrap();

    std::fs::write(ws.join(".gitignore"), "ignored_dir/\n*.log\nsecret.env\n").unwrap();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::create_dir_all(ws.join("ignored_dir")).unwrap();
    std::fs::write(ws.join("ignored_dir/cache.bin"), "ignored").unwrap();
    std::fs::write(ws.join("debug.log"), "log-data").unwrap();
    std::fs::write(ws.join("secret.env"), "TOKEN=123").unwrap();
    // Symlink inside workspace pointing outside workspace root
    fs_link::link_dir(&outside, &ws.join("escape_link")).expect("create escape link");

    let project = h.create_project(&admin.access_token, "TreeProj", &ws).await;
    let dev = h
        .invite_and_redeem(&admin.access_token, Role::Developer, vec![project.id.clone()], "dev1")
        .await;

    // Default respect_gitignore = true hides ignored_dir, debug.log, secret.env, and escape_link
    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/files/tree", project.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let tree: FileTreeResponse = TestHarness::decode(body).await;
    assert!(tree.respect_gitignore);
    let mut paths = Vec::new();
    collect_paths(&tree.entries, &mut paths);
    assert!(paths.contains(&"src".to_string()));
    assert!(paths.contains(&"src/main.rs".to_string()));
    assert!(paths.contains(&".gitignore".to_string()));
    assert!(!paths.iter().any(|p| p.contains("ignored_dir")));
    assert!(!paths.contains(&"debug.log".to_string()));
    assert!(!paths.contains(&"secret.env".to_string()));
    assert!(!paths.iter().any(|p| p.contains("escape_link")));

    // respect_gitignore=false reveals ignored files (still excludes symlink escape)
    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/files/tree?respect_gitignore=false", project.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let tree_all: FileTreeResponse = TestHarness::decode(body).await;
    assert!(!tree_all.respect_gitignore);
    let mut all_paths = Vec::new();
    collect_paths(&tree_all.entries, &mut all_paths);
    assert!(all_paths.contains(&"ignored_dir/cache.bin".to_string()));
    assert!(all_paths.contains(&"debug.log".to_string()));
    assert!(all_paths.contains(&"secret.env".to_string()));
    assert!(!all_paths.iter().any(|p| p.contains("escape_link")));

    // Path traversal with .. must be rejected (400 or 403)
    let (status, _) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/files/tree?path=../outside-secret", project.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::FORBIDDEN,
        "expected 400/403 for .. traversal, got {status}"
    );

    // Direct traversal into symlink escaping workspace must be rejected (400 or 403)
    let (status, _) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/files/tree?path=escape_link", project.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::FORBIDDEN,
        "expected 400/403 for symlink escape, got {status}"
    );
}

#[tokio::test]
async fn file_read_and_write_enforce_rbac_and_path_traversal_protection() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;
    let ws = h.create_workspace("repo-rw");
    let outside = h.create_workspace("outside-rw");
    std::fs::write(outside.join("secret.txt"), "do-not-touch").unwrap();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/lib.rs"), "pub fn hello() -> &'static str { \"hi\" }\n").unwrap();
    fs_link::link_dir(&outside, &ws.join("escape_link")).expect("create escape link");

    let project = h.create_project(&admin.access_token, "RwProj", &ws).await;
    let other_ws = h.create_workspace("other-proj");
    let other_project = h.create_project(&admin.access_token, "OtherProj", &other_ws).await;

    let dev = h
        .invite_and_redeem(&admin.access_token, Role::Developer, vec![project.id.clone()], "dev_rw")
        .await;
    let viewer = h
        .invite_and_redeem(&admin.access_token, Role::Viewer, vec![project.id.clone()], "viewer_rw")
        .await;
    let outsider = h
        .invite_and_redeem(
            &admin.access_token,
            Role::Developer,
            vec![other_project.id.clone()],
            "outsider_rw",
        )
        .await;

    // Viewer can read file
    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/files/content?path=src/lib.rs", project.id),
            Some(&viewer.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let file: FileContentResponse = TestHarness::decode(body).await;
    assert_eq!(file.path, "src/lib.rs");
    assert!(!file.is_binary);
    assert!(file.content.contains("hello()"));

    // Viewer is forbidden from saving via PUT or POST
    for method in [Method::PUT, Method::POST] {
        let (status, _) = h
            .request_json(
                method,
                &format!("/api/projects/{}/files/content", project.id),
                Some(&viewer.access_token),
                Some(json!({
                    "path": "src/lib.rs",
                    "content": "pub fn hacked() {}"
                })),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    // Unassigned user is forbidden on both read and write
    let (status, _) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/files/content?path=src/lib.rs", project.id),
            Some(&outsider.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = h
        .request_json(
            Method::PUT,
            &format!("/api/projects/{}/files/content", project.id),
            Some(&outsider.access_token),
            Some(json!({ "path": "src/lib.rs", "content": "nope" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Developer can save an existing file via PUT and create a new nested file via POST
    let (status, body) = h
        .request_json(
            Method::PUT,
            &format!("/api/projects/{}/files/content", project.id),
            Some(&dev.access_token),
            Some(json!({
                "path": "src/lib.rs",
                "content": "pub fn hello() -> &'static str { \"updated\" }\n"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let saved: SaveFileResponse = TestHarness::decode(body).await;
    assert_eq!(saved.path, "src/lib.rs");
    assert_eq!(
        std::fs::read_to_string(ws.join("src/lib.rs")).unwrap(),
        "pub fn hello() -> &'static str { \"updated\" }\n"
    );

    let (status, body) = h
        .request_json(
            Method::POST,
            &format!("/api/projects/{}/files/content", project.id),
            Some(&dev.access_token),
            Some(json!({
                "path": "nested/new_file.txt",
                "content": "created-by-dev"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let created: SaveFileResponse = TestHarness::decode(body).await;
    assert_eq!(created.path, "nested/new_file.txt");
    assert_eq!(
        std::fs::read_to_string(ws.join("nested/new_file.txt")).unwrap(),
        "created-by-dev"
    );

    // Admin can also read & write
    let (status, _) = h
        .request_json(
            Method::PUT,
            &format!("/api/projects/{}/files/content", project.id),
            Some(&admin.access_token),
            Some(json!({
                "path": "nested/new_file.txt",
                "content": "updated-by-admin"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        std::fs::read_to_string(ws.join("nested/new_file.txt")).unwrap(),
        "updated-by-admin"
    );

    // Path traversal & symlink escapes rejected on both read and write
    for bad_path in ["../outside-rw/secret.txt", "escape_link/secret.txt", "escape_link/new.txt"] {
        let (status, _) = h
            .request_json(
                Method::GET,
                &format!("/api/projects/{}/files/content?path={bad_path}", project.id),
                Some(&dev.access_token),
                None,
            )
            .await;
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::FORBIDDEN,
            "expected 400/403 on read of {bad_path}, got {status}"
        );

        let (status, _) = h
            .request_json(
                Method::PUT,
                &format!("/api/projects/{}/files/content", project.id),
                Some(&dev.access_token),
                Some(json!({ "path": bad_path, "content": "pwned" })),
            )
            .await;
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::FORBIDDEN,
            "expected 400/403 on write of {bad_path}, got {status}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(outside.join("secret.txt")).unwrap(),
        "do-not-touch"
    );
    assert!(!outside.join("new.txt").exists());
}

fn run_git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn init_temp_git_repo(dir: &Path) {
    run_git(dir, &["init", "-b", "main"]);
    run_git(dir, &["config", "user.name", "Test Dev"]);
    run_git(dir, &["config", "user.email", "dev@example.com"]);
    std::fs::write(dir.join("README.md"), "hello\nline2\n").unwrap();
    std::fs::write(dir.join("app.rs"), "fn app() {\n    println!(\"v1\");\n}\n").unwrap();
    run_git(dir, &["add", "README.md", "app.rs"]);
    run_git(dir, &["commit", "-m", "initial commit"]);
}

#[tokio::test]
async fn git_status_reports_branch_staged_unstaged_untracked_and_recent_commits() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;

    // Non-git workspace returns is_git_repo=false cleanly
    let plain_ws = h.create_workspace("non-git");
    let plain_proj = h.create_project(&admin.access_token, "PlainProj", &plain_ws).await;
    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/git/status", plain_proj.id),
            Some(&admin.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let plain_git: GitStatusResponse = TestHarness::decode(body).await;
    assert!(!plain_git.is_git_repo);
    assert!(plain_git.branch.is_none());

    // Real temporary git repo
    let git_ws = h.create_workspace("real-git");
    init_temp_git_repo(&git_ws);
    run_git(&git_ws, &["checkout", "-b", "feature/ui"]);

    // Modify README.md and stage it
    std::fs::write(git_ws.join("README.md"), "hello\nline2-staged\n").unwrap();
    run_git(&git_ws, &["add", "README.md"]);

    // Modify app.rs without staging
    std::fs::write(git_ws.join("app.rs"), "fn app() {\n    println!(\"v2\");\n}\n").unwrap();

    // Add untracked file
    std::fs::write(git_ws.join("notes.txt"), "untracked note\n").unwrap();

    let project = h.create_project(&admin.access_token, "GitProj", &git_ws).await;
    let viewer = h
        .invite_and_redeem(&admin.access_token, Role::Viewer, vec![project.id.clone()], "git_viewer")
        .await;

    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/git/status", project.id),
            Some(&viewer.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let git_status: GitStatusResponse = TestHarness::decode(body).await;
    assert!(git_status.is_git_repo);
    assert_eq!(git_status.branch.as_deref(), Some("feature/ui"));
    assert_eq!(git_status.staged.len(), 1);
    assert_eq!(git_status.staged[0].path, "README.md");
    assert_eq!(git_status.staged[0].status, "M");
    assert_eq!(git_status.unstaged.len(), 1);
    assert_eq!(git_status.unstaged[0].path, "app.rs");
    assert_eq!(git_status.unstaged[0].status, "M");
    assert_eq!(git_status.untracked, vec!["notes.txt".to_string()]);
    assert_eq!(git_status.recent_commits.len(), 1);
    assert_eq!(git_status.recent_commits[0].message, "initial commit");
    assert_eq!(git_status.recent_commits[0].author_name, "Test Dev");

    // Dedicated /git/commits endpoint also works
    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/git/commits?limit=5", project.id),
            Some(&viewer.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let commits_resp: GitCommitsResponse = TestHarness::decode(body).await;
    assert!(commits_resp.is_git_repo);
    assert_eq!(commits_resp.commits.len(), 1);
    assert_eq!(commits_resp.commits[0].message, "initial commit");
}

#[tokio::test]
async fn git_diff_returns_unified_and_split_diff_for_file_and_workspace_and_blocks_traversal() {
    let h = TestHarness::new().await;
    let admin = h.bootstrap_admin().await;
    let git_ws = h.create_workspace("diff-git");
    init_temp_git_repo(&git_ws);

    // Stage README.md change and leave app.rs unstaged
    std::fs::write(git_ws.join("README.md"), "hello\nline2-staged\n").unwrap();
    run_git(&git_ws, &["add", "README.md"]);
    std::fs::write(git_ws.join("app.rs"), "fn app() {\n    println!(\"v2\");\n    println!(\"extra\");\n}\n").unwrap();

    let project = h.create_project(&admin.access_token, "DiffProj", &git_ws).await;
    let dev = h
        .invite_and_redeem(&admin.access_token, Role::Developer, vec![project.id.clone()], "diff_dev")
        .await;

    // Unstaged workspace diff (unified)
    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/git/diff", project.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let ws_diff: GitDiffResponse = TestHarness::decode(body).await;
    assert!(ws_diff.is_git_repo);
    assert!(!ws_diff.staged);
    assert_eq!(ws_diff.mode, DiffViewMode::Unified);
    assert!(ws_diff.unified.contains("-    println!(\"v1\");"));
    assert!(ws_diff.unified.contains("+    println!(\"v2\");"));
    assert_eq!(ws_diff.files.len(), 1);
    assert_eq!(ws_diff.files[0].new_path, "app.rs");

    // Per-file split diff (unstaged app.rs)
    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/git/diff?path=app.rs&mode=split", project.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let split_diff: GitDiffResponse = TestHarness::decode(body).await;
    assert_eq!(split_diff.mode, DiffViewMode::Split);
    assert_eq!(split_diff.path.as_deref(), Some("app.rs"));
    assert_eq!(split_diff.files.len(), 1);
    let hunk = &split_diff.files[0].hunks[0];
    assert!(hunk.split_rows.iter().any(|r| {
        r.left_kind == DiffLineKind::Removed
            && r.left_content.as_deref().is_some_and(|c| c.contains("v1"))
            && r.right_kind == DiffLineKind::Added
            && r.right_content.as_deref().is_some_and(|c| c.contains("v2"))
    }));
    assert!(hunk.split_rows.iter().any(|r| {
        r.left_kind == DiffLineKind::Empty
            && r.right_kind == DiffLineKind::Added
            && r.right_content.as_deref().is_some_and(|c| c.contains("extra"))
    }));

    // Staged diff for README.md
    let (status, body) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/git/diff?path=README.md&staged=true", project.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let staged_diff: GitDiffResponse = TestHarness::decode(body).await;
    assert!(staged_diff.staged);
    assert!(staged_diff.unified.contains("-line2"));
    assert!(staged_diff.unified.contains("+line2-staged"));

    // Path traversal blocked
    let (status, _) = h
        .request_json(
            Method::GET,
            &format!("/api/projects/{}/git/diff?path=../secret.txt", project.id),
            Some(&dev.access_token),
            None,
        )
        .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::FORBIDDEN,
        "expected 400/403 on git diff path traversal, got {status}"
    );
}
