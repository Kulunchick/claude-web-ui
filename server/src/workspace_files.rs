//! Workspace file explorer, editor, and git panel (Ticket #4).

use crate::auth::{project_rbac_middleware, AppError, ProjectContext};
use crate::AppState;
use axum::extract::Query;
use axum::middleware;
use axum::routing::get;
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use claude_web_ui_shared::auth::Role;
use claude_web_ui_shared::files::{
    DiffLineKind, DiffViewMode, FileContentResponse, FileTreeNode, FileTreeResponse, GitCommitDto,
    GitCommitsResponse, GitDiffFile, GitDiffHunk, GitDiffLine, GitDiffResponse, GitFileChange,
    GitSplitDiffRow, GitStatusResponse, SaveFileRequest, SaveFileResponse,
};
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};
use tokio::process::Command;

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/api/projects/{project_id}/files/tree", get(get_file_tree))
        .route(
            "/api/projects/{project_id}/files/content",
            get(get_file_content)
                .put(save_file_content)
                .post(save_file_content),
        )
        .route("/api/projects/{project_id}/git/status", get(get_git_status))
        .route(
            "/api/projects/{project_id}/git/commits",
            get(get_git_commits),
        )
        .route("/api/projects/{project_id}/git/diff", get(get_git_diff))
        .layer(middleware::from_fn_with_state(
            state,
            project_rbac_middleware,
        ))
}

#[derive(Debug, Deserialize)]
pub struct FileTreeQuery {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default = "default_true")]
    pub respect_gitignore: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct FileContentQuery {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct GitCommitsQuery {
    #[serde(default = "default_commit_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize)]
pub struct GitDiffQuery {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub staged: bool,
    #[serde(default)]
    pub mode: DiffViewMode,
}

fn default_commit_limit() -> usize {
    20
}

async fn get_file_tree(
    Extension(ctx): Extension<ProjectContext>,
    Query(query): Query<FileTreeQuery>,
) -> Result<Json<FileTreeResponse>, AppError> {
    let workspace_root = PathBuf::from(&ctx.project.workspace_path);
    let rel = query.path.unwrap_or_default();
    let respect_gitignore = query.respect_gitignore;

    let response = tokio::task::spawn_blocking(move || {
        build_file_tree(&workspace_root, &rel, respect_gitignore)
    })
    .await
    .map_err(|e| AppError::internal(format!("file tree task failed: {e}")))??;

    Ok(Json(response))
}

async fn get_file_content(
    Extension(ctx): Extension<ProjectContext>,
    Query(query): Query<FileContentQuery>,
) -> Result<Json<FileContentResponse>, AppError> {
    let workspace_root = PathBuf::from(&ctx.project.workspace_path);
    let rel = query.path;

    let response = tokio::task::spawn_blocking(move || read_workspace_file(&workspace_root, &rel))
        .await
        .map_err(|e| AppError::internal(format!("file read task failed: {e}")))??;

    Ok(Json(response))
}

async fn save_file_content(
    Extension(ctx): Extension<ProjectContext>,
    Json(req): Json<SaveFileRequest>,
) -> Result<Json<SaveFileResponse>, AppError> {
    if ctx.effective_role == Role::Viewer {
        return Err(AppError::forbidden("viewer role is read-only"));
    }
    let workspace_root = PathBuf::from(&ctx.project.workspace_path);
    let response = tokio::task::spawn_blocking(move || {
        write_workspace_file(&workspace_root, &req.path, &req.content)
    })
    .await
    .map_err(|e| AppError::internal(format!("file write task failed: {e}")))??;

    Ok(Json(response))
}

async fn get_git_status(
    Extension(ctx): Extension<ProjectContext>,
    Query(query): Query<GitCommitsQuery>,
) -> Result<Json<GitStatusResponse>, AppError> {
    let workspace_root = canonicalize_root(Path::new(&ctx.project.workspace_path))?;
    if !check_is_git_repo(&workspace_root).await? {
        return Ok(Json(GitStatusResponse {
            is_git_repo: false,
            branch: None,
            staged: Vec::new(),
            unstaged: Vec::new(),
            untracked: Vec::new(),
            recent_commits: Vec::new(),
        }));
    }

    let status_out = run_git_cmd(&workspace_root, &["status", "--porcelain=v1", "-b", "-uall", "-z"])
        .await?;
    let (branch, staged, unstaged, untracked) = parse_git_status_z(&status_out);
    let limit = query.limit.clamp(1, 100);
    let recent_commits = fetch_git_commits(&workspace_root, limit).await?;

    Ok(Json(GitStatusResponse {
        is_git_repo: true,
        branch,
        staged,
        unstaged,
        untracked,
        recent_commits,
    }))
}

async fn get_git_commits(
    Extension(ctx): Extension<ProjectContext>,
    Query(query): Query<GitCommitsQuery>,
) -> Result<Json<GitCommitsResponse>, AppError> {
    let workspace_root = canonicalize_root(Path::new(&ctx.project.workspace_path))?;
    if !check_is_git_repo(&workspace_root).await? {
        return Ok(Json(GitCommitsResponse {
            is_git_repo: false,
            commits: Vec::new(),
        }));
    }

    let limit = query.limit.clamp(1, 100);
    let commits = fetch_git_commits(&workspace_root, limit).await?;
    Ok(Json(GitCommitsResponse {
        is_git_repo: true,
        commits,
    }))
}

async fn get_git_diff(
    Extension(ctx): Extension<ProjectContext>,
    Query(query): Query<GitDiffQuery>,
) -> Result<Json<GitDiffResponse>, AppError> {
    let workspace_root = canonicalize_root(Path::new(&ctx.project.workspace_path))?;

    let validated_path = if let Some(raw_path) = query.path.as_deref()
        && !raw_path.trim().is_empty()
    {
        let segments = validate_relative_segments(raw_path, false)?;
        check_existing_prefixes(&workspace_root, &segments, false)?;
        Some(segments.join("/"))
    } else {
        None
    };

    if !check_is_git_repo(&workspace_root).await? {
        return Ok(Json(GitDiffResponse {
            is_git_repo: false,
            path: validated_path,
            staged: query.staged,
            mode: query.mode,
            unified: String::new(),
            files: Vec::new(),
        }));
    }

    let mut args = vec!["diff", "--no-color", "--unified=3"];
    if query.staged {
        args.push("--cached");
    }
    if let Some(ref rel) = validated_path {
        args.push("--");
        args.push(rel.as_str());
    }

    let diff_bytes = run_git_cmd(&workspace_root, &args).await?;
    let mut unified = String::from_utf8_lossy(&diff_bytes).into_owned();

    // If diff is empty for a specific unstaged path, check if it's an untracked file
    if unified.trim().is_empty()
        && !query.staged
        && let Some(ref rel) = validated_path
        && let Ok(untracked_out) = run_git_cmd(
            &workspace_root,
            &["ls-files", "--others", "--exclude-standard", "--", rel.as_str()],
        )
        .await
        && !String::from_utf8_lossy(&untracked_out).trim().is_empty()
    {
        let file_path = workspace_root.join(rel);
        if let Ok(text) = std::fs::read_to_string(&file_path) {
            unified = build_untracked_unified_diff(rel, &text);
        }
    }

    let files = parse_unified_diff(&unified);

    Ok(Json(GitDiffResponse {
        is_git_repo: true,
        path: validated_path,
        staged: query.staged,
        mode: query.mode,
        unified,
        files,
    }))
}

fn build_untracked_unified_diff(rel_path: &str, text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let count = lines.len();
    let mut out = format!(
        "diff --git a/{rel_path} b/{rel_path}\nnew file mode 100644\n--- /dev/null\n+++ b/{rel_path}\n@@ -0,0 +1,{count} @@\n"
    );
    for line in lines {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn parse_unified_diff(unified: &str) -> Vec<GitDiffFile> {
    let mut files: Vec<GitDiffFile> = Vec::new();
    let mut cur_file: Option<GitDiffFile> = None;
    let mut cur_hunk: Option<GitDiffHunk> = None;
    let mut old_ln: u32 = 0;
    let mut new_ln: u32 = 0;

    let flush_hunk = |file: &mut Option<GitDiffFile>, hunk: &mut Option<GitDiffHunk>| {
        if let (Some(f), Some(mut h)) = (file.as_mut(), hunk.take()) {
            h.split_rows = build_split_rows(&h.lines);
            f.hunks.push(h);
        }
    };
    let flush_file = |files: &mut Vec<GitDiffFile>,
                      file: &mut Option<GitDiffFile>,
                      hunk: &mut Option<GitDiffHunk>| {
        flush_hunk(file, hunk);
        if let Some(f) = file.take() {
            files.push(f);
        }
    };

    for raw_line in unified.lines() {
        let line = raw_line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("diff --git ") {
            flush_file(&mut files, &mut cur_file, &mut cur_hunk);
            let (old_p, new_p) = parse_diff_git_paths(rest);
            cur_file = Some(GitDiffFile {
                old_path: old_p,
                new_path: new_p,
                hunks: Vec::new(),
            });
            continue;
        }

        if cur_file.is_none() {
            continue;
        }

        if let Some(p) = line.strip_prefix("--- ") {
            if cur_hunk.is_none()
                && let Some(f) = cur_file.as_mut()
                && p != "/dev/null"
            {
                f.old_path = p.strip_prefix("a/").unwrap_or(p).to_string();
            }
            continue;
        }
        if let Some(p) = line.strip_prefix("+++ ") {
            if cur_hunk.is_none()
                && let Some(f) = cur_file.as_mut()
                && p != "/dev/null"
            {
                f.new_path = p.strip_prefix("b/").unwrap_or(p).to_string();
            }
            continue;
        }

        if line.starts_with("@@ ") {
            flush_hunk(&mut cur_file, &mut cur_hunk);
            if let Some((os, oc, ns, nc)) = parse_hunk_header(line) {
                old_ln = os;
                new_ln = ns;
                cur_hunk = Some(GitDiffHunk {
                    header: line.to_string(),
                    old_start: os,
                    old_count: oc,
                    new_start: ns,
                    new_count: nc,
                    lines: Vec::new(),
                    split_rows: Vec::new(),
                });
            }
            continue;
        }

        if let Some(h) = cur_hunk.as_mut() {
            if let Some(content) = line.strip_prefix('+') {
                h.lines.push(GitDiffLine {
                    kind: DiffLineKind::Added,
                    old_line: None,
                    new_line: Some(new_ln),
                    content: content.to_string(),
                });
                new_ln += 1;
            } else if let Some(content) = line.strip_prefix('-') {
                h.lines.push(GitDiffLine {
                    kind: DiffLineKind::Removed,
                    old_line: Some(old_ln),
                    new_line: None,
                    content: content.to_string(),
                });
                old_ln += 1;
            } else if let Some(content) = line.strip_prefix(' ') {
                h.lines.push(GitDiffLine {
                    kind: DiffLineKind::Context,
                    old_line: Some(old_ln),
                    new_line: Some(new_ln),
                    content: content.to_string(),
                });
                old_ln += 1;
                new_ln += 1;
            }
        }
    }

    flush_file(&mut files, &mut cur_file, &mut cur_hunk);
    files
}

fn parse_diff_git_paths(rest: &str) -> (String, String) {
    if let Some((a, b)) = rest.split_once(" b/") {
        let old_p = a.strip_prefix("a/").unwrap_or(a).to_string();
        return (old_p, b.to_string());
    }
    (rest.to_string(), rest.to_string())
}

fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32)> {
    let inner = line.strip_prefix("@@ ")?;
    let (range_part, _) = inner.split_once(" @@")?;
    let mut parts = range_part.split_whitespace();
    let old_part = parts.next()?.strip_prefix('-')?;
    let new_part = parts.next()?.strip_prefix('+')?;
    let (os, oc) = parse_range(old_part)?;
    let (ns, nc) = parse_range(new_part)?;
    Some((os, oc, ns, nc))
}

fn parse_range(s: &str) -> Option<(u32, u32)> {
    if let Some((start, count)) = s.split_once(',') {
        Some((start.parse().ok()?, count.parse().ok()?))
    } else {
        Some((s.parse().ok()?, 1))
    }
}

fn build_split_rows(lines: &[GitDiffLine]) -> Vec<GitSplitDiffRow> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        match lines[i].kind {
            DiffLineKind::Context => {
                rows.push(GitSplitDiffRow {
                    left_line: lines[i].old_line,
                    left_content: Some(lines[i].content.clone()),
                    left_kind: DiffLineKind::Context,
                    right_line: lines[i].new_line,
                    right_content: Some(lines[i].content.clone()),
                    right_kind: DiffLineKind::Context,
                });
                i += 1;
            }
            DiffLineKind::Removed | DiffLineKind::Added => {
                let mut removed = Vec::new();
                let mut added = Vec::new();
                while i < lines.len() && lines[i].kind == DiffLineKind::Removed {
                    removed.push(&lines[i]);
                    i += 1;
                }
                while i < lines.len() && lines[i].kind == DiffLineKind::Added {
                    added.push(&lines[i]);
                    i += 1;
                }
                let max_len = removed.len().max(added.len());
                for idx in 0..max_len {
                    let left = removed.get(idx);
                    let right = added.get(idx);
                    rows.push(GitSplitDiffRow {
                        left_line: left.and_then(|l| l.old_line),
                        left_content: left.map(|l| l.content.clone()),
                        left_kind: if left.is_some() {
                            DiffLineKind::Removed
                        } else {
                            DiffLineKind::Empty
                        },
                        right_line: right.and_then(|r| r.new_line),
                        right_content: right.map(|r| r.content.clone()),
                        right_kind: if right.is_some() {
                            DiffLineKind::Added
                        } else {
                            DiffLineKind::Empty
                        },
                    });
                }
            }
            DiffLineKind::Empty => {
                i += 1;
            }
        }
    }
    rows
}

async fn check_is_git_repo(workspace_root: &Path) -> Result<bool, AppError> {
    let output = Command::new("git")
        .current_dir(workspace_root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .await
        .map_err(|e| AppError::internal(format!("failed to execute git: {e}")))?;
    if !output.status.success() {
        return Ok(false);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim() == "true")
}

async fn run_git_cmd(workspace_root: &Path, args: &[&str]) -> Result<Vec<u8>, AppError> {
    let output = Command::new("git")
        .current_dir(workspace_root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .output()
        .await
        .map_err(|e| AppError::internal(format!("failed to execute git: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(AppError::internal(format!("git command failed: {stderr}")));
    }
    Ok(output.stdout)
}

async fn fetch_git_commits(
    workspace_root: &Path,
    limit: usize,
) -> Result<Vec<GitCommitDto>, AppError> {
    let limit_arg = format!("-n{limit}");
    let output = Command::new("git")
        .current_dir(workspace_root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "log",
            &limit_arg,
            "--format=%H%x1f%h%x1f%an%x1f%ae%x1f%aI%x1f%s%x1e",
        ])
        .output()
        .await
        .map_err(|e| AppError::internal(format!("failed to execute git log: {e}")))?;

    if !output.status.success() {
        // Unborn branch (no commits yet) returns non-zero
        return Ok(Vec::new());
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let mut commits = Vec::new();
    for rec in raw.split('\x1e') {
        let trimmed = rec.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split('\x1f').collect();
        if parts.len() >= 6 {
            commits.push(GitCommitDto {
                hash: parts[0].to_string(),
                short_hash: parts[1].to_string(),
                author_name: parts[2].to_string(),
                author_email: parts[3].to_string(),
                committed_at: parts[4].to_string(),
                message: parts[5].to_string(),
            });
        }
    }
    Ok(commits)
}

fn parse_branch_header(header: &str) -> Option<String> {
    let rest = header.strip_prefix("## ")?;
    if let Some(branch) = rest.strip_prefix("No commits yet on ") {
        return Some(branch.trim().to_string());
    }
    if let Some(branch) = rest.strip_prefix("Initial commit on ") {
        return Some(branch.trim().to_string());
    }
    if rest.starts_with("HEAD (no branch)") {
        return Some("HEAD (detached)".to_string());
    }
    let before_remote = rest.split("...").next().unwrap_or(rest);
    let branch = before_remote.split(" [").next().unwrap_or(before_remote).trim();
    if branch.is_empty() {
        None
    } else {
        Some(branch.to_string())
    }
}

fn parse_git_status_z(
    bytes: &[u8],
) -> (
    Option<String>,
    Vec<GitFileChange>,
    Vec<GitFileChange>,
    Vec<String>,
) {
    let mut tokens = bytes
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned());

    let mut branch = None;
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();

    while let Some(entry) = tokens.next() {
        if entry.starts_with("## ") {
            branch = parse_branch_header(&entry);
            continue;
        }
        if entry.len() < 3 {
            continue;
        }
        let mut chars = entry.chars();
        let x = chars.next().unwrap_or(' ');
        let y = chars.next().unwrap_or(' ');
        let _space = chars.next();
        let path = entry[3..].to_string();

        if x == '?' && y == '?' {
            untracked.push(path);
            continue;
        }

        let old_path = if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            tokens.next()
        } else {
            None
        };

        if x != ' ' && x != '?' {
            staged.push(GitFileChange {
                path: path.clone(),
                status: x.to_string(),
                old_path: old_path.clone(),
            });
        }
        if y != ' ' && y != '?' {
            unstaged.push(GitFileChange {
                path,
                status: y.to_string(),
                old_path,
            });
        }
    }

    (branch, staged, unstaged, untracked)
}

fn canonicalize_root(workspace_root: &Path) -> Result<PathBuf, AppError> {
    if !workspace_root.exists() || !workspace_root.is_dir() {
        return Err(AppError::bad_request(
            "project workspace directory does not exist",
        ));
    }
    std::fs::canonicalize(workspace_root)
        .map_err(|e| AppError::internal(format!("failed to canonicalize workspace root: {e}")))
}

fn validate_relative_segments(raw_path: &str, allow_empty: bool) -> Result<Vec<String>, AppError> {
    let trimmed = raw_path.trim().replace('\\', "/");
    if trimmed.contains('\0') {
        return Err(AppError::bad_request("invalid null byte in path"));
    }
    if trimmed.is_empty() || trimmed == "." {
        if allow_empty {
            return Ok(Vec::new());
        }
        return Err(AppError::bad_request("path is required"));
    }
    if trimmed.starts_with('/') {
        return Err(AppError::bad_request("absolute paths are not allowed"));
    }

    let mut segments = Vec::new();
    for comp in Path::new(&trimmed).components() {
        match comp {
            Component::Normal(os) => {
                let s = os.to_string_lossy();
                if s.is_empty() {
                    continue;
                }
                segments.push(s.into_owned());
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::bad_request(
                    "path traversal outside workspace is not allowed",
                ));
            }
        }
    }

    if segments.is_empty() && !allow_empty {
        return Err(AppError::bad_request("path is required"));
    }
    Ok(segments)
}

fn check_existing_prefixes(
    canonical_root: &Path,
    segments: &[String],
    for_write: bool,
) -> Result<PathBuf, AppError> {
    let mut cur = canonical_root.to_path_buf();
    for (idx, seg) in segments.iter().enumerate() {
        cur.push(seg);
        let is_last = idx + 1 == segments.len();
        if let Ok(meta) = std::fs::symlink_metadata(&cur) {
            if meta.file_type().is_symlink() {
                let resolved = std::fs::canonicalize(&cur)
                    .map_err(|_| AppError::forbidden("symlink escapes workspace root"))?;
                if !resolved.starts_with(canonical_root) {
                    return Err(AppError::forbidden("symlink escapes workspace root"));
                }
            } else if let Ok(resolved) = std::fs::canonicalize(&cur)
                && !resolved.starts_with(canonical_root)
            {
                return Err(AppError::forbidden("path escapes workspace root"));
            }

            if for_write {
                if !is_last {
                    let resolved = std::fs::canonicalize(&cur).map_err(|_| {
                        AppError::forbidden("path cannot be resolved inside workspace")
                    })?;
                    if !resolved.is_dir() {
                        return Err(AppError::bad_request("parent path is not a directory"));
                    }
                } else if cur.is_dir() {
                    return Err(AppError::bad_request("cannot overwrite a directory"));
                }
            }
        }
    }
    Ok(cur)
}

fn resolve_existing_in_workspace(
    workspace_root: &Path,
    raw_rel: &str,
    allow_empty: bool,
) -> Result<(PathBuf, PathBuf, String), AppError> {
    let canonical_root = canonicalize_root(workspace_root)?;
    let segments = validate_relative_segments(raw_rel, allow_empty)?;
    let normalized_rel = segments.join("/");
    let candidate = check_existing_prefixes(&canonical_root, &segments, false)?;

    if !candidate.exists() {
        return Err(AppError::not_found("path not found"));
    }

    let canonical_target = std::fs::canonicalize(&candidate)
        .map_err(|_| AppError::forbidden("path cannot be resolved inside workspace"))?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(AppError::forbidden("path escapes workspace root"));
    }

    Ok((canonical_root, canonical_target, normalized_rel))
}

fn format_modified_at(meta: &std::fs::Metadata) -> Option<String> {
    meta.modified()
        .ok()
        .map(|t| DateTime::<Utc>::from(t).to_rfc3339())
}

fn read_workspace_file(workspace_root: &Path, raw_rel: &str) -> Result<FileContentResponse, AppError> {
    let (_canonical_root, target_file, normalized_rel) =
        resolve_existing_in_workspace(workspace_root, raw_rel, false)?;

    let meta = std::fs::metadata(&target_file)
        .map_err(|e| AppError::internal(format!("failed to stat file: {e}")))?;
    if meta.is_dir() {
        return Err(AppError::bad_request("cannot read directory as file"));
    }

    let bytes = std::fs::read(&target_file)
        .map_err(|e| AppError::internal(format!("failed to read file: {e}")))?;
    let size = bytes.len() as u64;
    let (content, is_binary) = match String::from_utf8(bytes) {
        Ok(text) => (text, false),
        Err(_) => (String::new(), true),
    };

    Ok(FileContentResponse {
        path: normalized_rel,
        content,
        size,
        is_binary,
        modified_at: format_modified_at(&meta),
    })
}

fn write_workspace_file(
    workspace_root: &Path,
    raw_rel: &str,
    content: &str,
) -> Result<SaveFileResponse, AppError> {
    let canonical_root = canonicalize_root(workspace_root)?;
    let segments = validate_relative_segments(raw_rel, false)?;
    let normalized_rel = segments.join("/");
    let cur = check_existing_prefixes(&canonical_root, &segments, true)?;

    if let Some(parent) = cur.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::internal(format!("failed to create parent dirs: {e}")))?;
        let canonical_parent = std::fs::canonicalize(parent)
            .map_err(|e| AppError::internal(format!("failed to canonicalize parent: {e}")))?;
        if !canonical_parent.starts_with(&canonical_root) {
            return Err(AppError::forbidden("path escapes workspace root"));
        }
    }

    std::fs::write(&cur, content.as_bytes())
        .map_err(|e| AppError::internal(format!("failed to write file: {e}")))?;
    let canonical_written = std::fs::canonicalize(&cur)
        .map_err(|e| AppError::internal(format!("failed to stat written file: {e}")))?;
    if !canonical_written.starts_with(&canonical_root) {
        return Err(AppError::forbidden("path escapes workspace root"));
    }

    let meta = std::fs::metadata(&canonical_written)
        .map_err(|e| AppError::internal(format!("failed to stat written file: {e}")))?;

    Ok(SaveFileResponse {
        path: normalized_rel,
        size: meta.len(),
        modified_at: format_modified_at(&meta),
    })
}

fn build_file_tree(
    workspace_root: &Path,
    raw_rel: &str,
    respect_gitignore: bool,
) -> Result<FileTreeResponse, AppError> {
    let (canonical_root, target_dir, rel_root) =
        resolve_existing_in_workspace(workspace_root, raw_rel, true)?;

    if !target_dir.is_dir() {
        return Err(AppError::bad_request("target path is not a directory"));
    }

    // Load ancestor .gitignore rules if browsing a subdirectory
    let mut rules = Vec::new();
    if respect_gitignore {
        load_gitignore_for_dir(&canonical_root, "", &mut rules);
        if !rel_root.is_empty() {
            let mut acc = PathBuf::from(&canonical_root);
            let mut rel_acc = String::new();
            for seg in rel_root.split('/') {
                acc.push(seg);
                if !rel_acc.is_empty() {
                    rel_acc.push('/');
                }
                rel_acc.push_str(seg);
                load_gitignore_for_dir(&acc, &rel_acc, &mut rules);
            }
        }
    }

    let entries = read_dir_recursive(
        &canonical_root,
        &target_dir,
        &rel_root,
        respect_gitignore,
        &rules,
    )?;

    Ok(FileTreeResponse {
        root: rel_root,
        respect_gitignore,
        entries,
    })
}

fn read_dir_recursive(
    canonical_root: &Path,
    dir: &Path,
    rel_dir: &str,
    respect_gitignore: bool,
    inherited_rules: &[GitignoreRule],
) -> Result<Vec<FileTreeNode>, AppError> {
    let mut rules = inherited_rules.to_vec();
    if respect_gitignore && !rel_dir.is_empty() {
        // Root .gitignore already loaded before recursion; load nested .gitignore here
        load_gitignore_for_dir(dir, rel_dir, &mut rules);
    }

    let read_dir = std::fs::read_dir(dir)
        .map_err(|e| AppError::internal(format!("failed to read directory: {e}")))?;

    let mut nodes = Vec::new();
    for entry_res in read_dir {
        let entry = match entry_res {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }

        let entry_path = entry.path();
        let Ok(canonical_entry) = std::fs::canonicalize(&entry_path) else {
            continue;
        };
        if !canonical_entry.starts_with(canonical_root) {
            // Skip any symlink or junction escaping workspace root
            continue;
        }

        let Ok(meta) = std::fs::metadata(&canonical_entry) else {
            continue;
        };
        let is_dir = meta.is_dir();
        let rel_path = if rel_dir.is_empty() {
            name.clone()
        } else {
            format!("{rel_dir}/{name}")
        };

        if respect_gitignore && is_ignored(&rules, &rel_path, is_dir) {
            continue;
        }

        let children = if is_dir {
            read_dir_recursive(
                canonical_root,
                &canonical_entry,
                &rel_path,
                respect_gitignore,
                &rules,
            )?
        } else {
            Vec::new()
        };

        nodes.push(FileTreeNode {
            name,
            path: rel_path,
            is_dir,
            size: if is_dir { 0 } else { meta.len() },
            children,
        });
    }

    nodes.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });

    Ok(nodes)
}

#[derive(Debug, Clone)]
struct GitignoreRule {
    base_rel: String,
    pattern: String,
    negated: bool,
    dir_only: bool,
    anchored: bool,
}

fn load_gitignore_for_dir(dir: &Path, rel_dir: &str, rules: &mut Vec<GitignoreRule>) {
    let gitignore_path = dir.join(".gitignore");
    let Ok(contents) = std::fs::read_to_string(gitignore_path) else {
        return;
    };
    for raw_line in contents.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let mut pat = line.to_string();
        let negated = if let Some(rest) = pat.strip_prefix('!') {
            pat = rest.to_string();
            true
        } else {
            false
        };
        if pat.starts_with("\\#") || pat.starts_with("\\!") {
            pat = pat[1..].to_string();
        }
        let dir_only = pat.ends_with('/');
        if dir_only {
            pat = pat.trim_end_matches('/').to_string();
        }
        let anchored = if let Some(rest) = pat.strip_prefix('/') {
            pat = rest.to_string();
            true
        } else {
            pat.contains('/')
        };
        if pat.is_empty() {
            continue;
        }
        rules.push(GitignoreRule {
            base_rel: rel_dir.to_string(),
            pattern: pat,
            negated,
            dir_only,
            anchored,
        });
    }
}

fn is_ignored(rules: &[GitignoreRule], rel_path: &str, is_dir: bool) -> bool {
    let mut ignored = false;
    for rule in rules {
        let sub_path = if rule.base_rel.is_empty() {
            rel_path
        } else if let Some(rest) = rel_path.strip_prefix(&format!("{}/", rule.base_rel)) {
            rest
        } else {
            continue;
        };

        if rule.dir_only && !is_dir {
            continue;
        }

        let matched = if rule.anchored {
            glob_match(&rule.pattern, sub_path)
        } else {
            let basename = sub_path.rsplit('/').next().unwrap_or(sub_path);
            glob_match(&rule.pattern, basename) || glob_match(&rule.pattern, sub_path)
        };

        if matched {
            ignored = !rule.negated;
        }
    }
    ignored
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let pat_parts: Vec<&str> = pattern.split('/').collect();
    let txt_parts: Vec<&str> = text.split('/').collect();
    match_segments(&pat_parts, &txt_parts)
}

fn match_segments(pat: &[&str], txt: &[&str]) -> bool {
    if pat.is_empty() {
        return txt.is_empty();
    }
    if pat[0] == "**" {
        for i in 0..=txt.len() {
            if match_segments(&pat[1..], &txt[i..]) {
                return true;
            }
        }
        return false;
    }
    if txt.is_empty() {
        return false;
    }
    segment_glob_match(pat[0], txt[0]) && match_segments(&pat[1..], &txt[1..])
}

fn segment_glob_match(pat: &str, txt: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = txt.chars().collect();
    let mut dp = vec![vec![false; t.len() + 1]; p.len() + 1];
    dp[0][0] = true;
    for i in 1..=p.len() {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=p.len() {
        for j in 1..=t.len() {
            if p[i - 1] == '*' {
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
            } else if p[i - 1] == '?' || p[i - 1] == t[j - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            }
        }
    }
    dp[p.len()][t.len()]
}
