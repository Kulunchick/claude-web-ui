//! Workspace file tree, editor, and git status and diffs.

use claude_web_ui_shared::auth::Role;
use claude_web_ui_shared::files::{
    DiffLineKind, DiffViewMode, FileContentResponse, FileTreeNode, FileTreeResponse,
    GitDiffResponse, GitFileChange, GitStatusResponse,
};
use leptos::prelude::*;
use serde::Deserialize;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Deserialize)]
pub struct WorkspaceBrowserState {
    pub project_name: String,
    pub workspace_path: String,
    pub read_only: bool,
    pub respect_gitignore: bool,
    pub file_tree: Vec<FileTreeNode>,
    pub collapsed_dirs: BTreeMap<String, bool>,
    pub active_file: Option<FileContentResponse>,
    pub editor_text: String,
    pub editor_dirty: bool,
    pub git_status: Option<GitStatusResponse>,
    pub git_diff: Option<GitDiffResponse>,
    pub diff_mode: DiffViewMode,
    pub diff_path: Option<String>,
}

#[component]
pub fn WorkspaceFilesGitView(
    #[prop(optional)] project_id: Option<String>,
    #[prop(default = Role::Viewer)] user_role: Role,
    #[prop(optional)] initial_tree: Option<FileTreeResponse>,
    #[prop(optional)] initial_file: Option<FileContentResponse>,
    #[prop(optional)] initial_git_status: Option<GitStatusResponse>,
    #[prop(optional)] initial_git_diff: Option<GitDiffResponse>,
) -> impl IntoView {
    let state = WorkspaceBrowserState {
        project_name: project_id.unwrap_or_else(|| "No active project".to_string()),
        workspace_path: initial_tree
            .as_ref()
            .map(|tree| tree.root.clone())
            .unwrap_or_default(),
        read_only: user_role == Role::Viewer,
        respect_gitignore: initial_tree
            .as_ref()
            .is_none_or(|tree| tree.respect_gitignore),
        file_tree: initial_tree.map(|tree| tree.entries).unwrap_or_default(),
        collapsed_dirs: BTreeMap::new(),
        editor_text: initial_file
            .as_ref()
            .map(|file| file.content.clone())
            .unwrap_or_default(),
        active_file: initial_file,
        editor_dirty: false,
        git_status: initial_git_status,
        diff_mode: initial_git_diff
            .as_ref()
            .map(|diff| diff.mode)
            .unwrap_or_default(),
        diff_path: initial_git_diff.as_ref().and_then(|diff| diff.path.clone()),
        git_diff: initial_git_diff,
    };
    view! { <WorkspaceFilesGitBrowser state=state /> }
}

#[component]
pub(crate) fn WorkspaceFilesGitBrowser(state: WorkspaceBrowserState) -> impl IntoView {
    let selected = state.active_file.as_ref().map(|file| file.path.clone());
    let file_can_be_saved = state
        .active_file
        .as_ref()
        .is_some_and(|file| !file.is_binary);
    let file_path = selected.clone().unwrap_or_else(|| "Select a file".into());
    let branch = state
        .git_status
        .as_ref()
        .and_then(|status| status.branch.clone());
    let diff_label = match state.diff_mode {
        DiffViewMode::Unified => "Unified",
        DiffViewMode::Split => "Split",
    };

    view! {
        <section data-view="files-git">
            <section class="card workspace-header-card">
                <div>
                    <h2 class="card-title">{state.project_name}</h2>
                    <p class="card-description workspace-path">{state.workspace_path}</p>
                </div>
                <div class="files-git-controls">
                    <label class="toggle-label">
                        <input type="checkbox" class="switch" id="toggle-gitignore" checked=state.respect_gitignore />
                        <span>"Respect .gitignore"</span>
                    </label>
                    <button class="btn-secondary btn-sm" type="button" id="refresh-workspace-btn">"Refresh"</button>
                </div>
            </section>
            <div class="files-git-grid">
                <section class="card file-tree-pane">
                    <div class="pane-head"><h3>"Files"</h3></div>
                    {(!state.read_only).then(|| view!{
                        <form id="new-file-form" class="new-file-form">
                            <input name="path" aria-label="New file path" placeholder="new/file.txt" required />
                            <button class="btn-secondary btn-sm" type="submit" aria-label="Create file">"+"</button>
                        </form>
                    })}
                    <div class="tree-scroll">
                        {if state.file_tree.is_empty() {
                            view! { <p class="subtitle">"Empty workspace"</p> }.into_any()
                        } else {
                            view! {
                                <FileTreeList nodes=state.file_tree selected=selected
                                    collapsed=Arc::new(state.collapsed_dirs) depth=0 />
                            }.into_any()
                        }}
                    </div>
                </section>
                <section class="card editor-pane">
                    <div class="editor-toolbar">
                        <strong class="editor-file-path">{file_path}</strong>
                        <div class="files-git-controls">
                            {if state.read_only {
                                view! { <span class="badge badge-secondary">"Read-Only (Viewer)"</span> }.into_any()
                            } else {
                                view! {
                                    <button class="btn-primary btn-sm" type="button" id="save-file-btn" disabled=!file_can_be_saved>
                                        {if state.editor_dirty { "Save *" } else { "Save" }}
                                    </button>
                                }.into_any()
                            }}
                        </div>
                    </div>
                    {match state.active_file {
                        Some(file) if file.is_binary => view! {
                            <p class="subtitle">{format!("Binary file ({} bytes) cannot be edited in text view.", file.size)}</p>
                        }.into_any(),
                        file => view! {
                            <textarea id="code-editor" class="code-editor" readonly={state.read_only || file.is_none()}
                                placeholder="Select a file from the explorer..." prop:value=state.editor_text />
                        }.into_any(),
                    }}
                </section>
                <section class="card git-pane">
                    <div class="pane-head">
                        <h3>"Git " {branch.map(|branch| view! { <span class="badge badge-outline">{branch}</span> })}</h3>
                        <div class="files-git-controls">
                            <button class="btn-secondary btn-sm" type="button" id="diff-all-btn">"All"</button>
                            <button class="btn-outline btn-sm" type="button" id="toggle-diff-mode-btn" title="Toggle unified / split diff">{diff_label}</button>
                        </div>
                    </div>
                    <GitWorkspaceDetails status=state.git_status diff=state.git_diff
                        diff_mode=state.diff_mode diff_path=state.diff_path />
                </section>
            </div>
        </section>
    }
}

#[component]
fn FileTreeList(
    nodes: Vec<FileTreeNode>,
    #[prop(optional_no_strip)] selected: Option<String>,
    collapsed: Arc<BTreeMap<String, bool>>,
    depth: usize,
) -> AnyView {
    view! {
        <ul class="file-tree-list">
            {nodes.into_iter().map(|node| {
                let padding = format!("padding-left:{}px", 8 + depth * 12);
                if node.is_dir {
                    let is_collapsed = collapsed.get(&node.path).copied().unwrap_or_default();
                    view! {
                        <li>
                            <button class="tree-node-btn list-row" type="button" style=padding
                                aria-expanded=(!is_collapsed).to_string() data-dir-toggle=node.path>
                                {if is_collapsed { "[+] " } else { "[-] " }} {node.name}
                            </button>
                            {(!is_collapsed && !node.children.is_empty()).then(|| view! {
                                <FileTreeList nodes=node.children selected=selected.clone()
                                    collapsed=collapsed.clone() depth={depth + 1} />
                            })}
                        </li>
                    }.into_any()
                } else {
                    let is_selected = selected.as_ref() == Some(&node.path);
                    view! {
                        <li>
                            <button class="tree-node-btn list-row" class:active=is_selected type="button"
                                style=padding data-file-open=node.path>{node.name}</button>
                        </li>
                    }.into_any()
                }
            }).collect_view()}
        </ul>
    }
    .into_any()
}

#[component]
fn GitWorkspaceDetails(
    #[prop(optional_no_strip)] status: Option<GitStatusResponse>,
    #[prop(optional_no_strip)] diff: Option<GitDiffResponse>,
    diff_mode: DiffViewMode,
    #[prop(optional_no_strip)] diff_path: Option<String>,
) -> impl IntoView {
    let (staged, unstaged, untracked, commits) = status
        .map(|status| {
            (
                status.staged,
                status.unstaged,
                status.untracked,
                status.recent_commits,
            )
        })
        .unwrap_or_default();
    let untracked = untracked
        .into_iter()
        .map(|path| GitFileChange {
            path,
            status: "?".into(),
            old_path: None,
        })
        .collect();

    view! {
        <div class="git-changes-section">
            <GitChangeGroup title="Staged" badge="git-badge staged" changes=staged staged=true />
            <GitChangeGroup title="Unstaged" badge="git-badge unstaged" changes=unstaged staged=false />
            <GitChangeGroup title="Untracked" badge="git-badge untracked" changes=untracked staged=false />
        </div>
        <div class="git-diff-section">
            <div class="git-group-label">{format!("Diff ({})", diff_path.as_deref().unwrap_or("Workspace"))}</div>
            <GitDiff diff=diff mode=diff_mode />
        </div>
        <div class="git-commits-section">
            <div class="git-group-label">"Recent Commits"</div>
            {commits.is_empty().then(|| view! { <p class="subtitle">"No commits"</p> })}
            {commits.into_iter().map(|commit| view! {
                <div class="commit-item">
                    <code class="commit-hash">{commit.short_hash}</code>
                    <span class="commit-msg">{commit.message}</span>
                    <span class="commit-meta">{commit.author_name}</span>
                </div>
            }).collect_view()}
        </div>
    }
}

#[component]
fn GitChangeGroup(
    title: &'static str,
    badge: &'static str,
    changes: Vec<GitFileChange>,
    staged: bool,
) -> impl IntoView {
    view! {
        <div class="git-group-label">{format!("{title} ({})", changes.len())}</div>
        {changes.is_empty().then(|| view! { <p class="subtitle">"None"</p> })}
        {changes.into_iter().map(|change| view! {
            <button class="git-change-item list-row" type="button" data-diff-path=change.path.clone() data-diff-staged=staged.to_string()>
                <span class=badge>{change.status}</span><span class="git-change-path">{change.path.clone()}</span>
            </button>
        }).collect_view()}
    }
}

#[component]
fn GitDiff(
    #[prop(optional_no_strip)] diff: Option<GitDiffResponse>,
    mode: DiffViewMode,
) -> impl IntoView {
    let Some(diff) = diff.filter(|diff| diff.is_git_repo) else {
        return view! { <p class="subtitle">"Workspace is not a Git repository."</p> }.into_any();
    };
    if diff.files.is_empty() {
        return view! { <p class="subtitle">"Working tree clean for current selection."</p> }
            .into_any();
    }

    match mode {
        DiffViewMode::Unified => view! {
            <pre class="diff-unified">
                {diff.unified.split('\n').map(|line| {
                    let class = if line.starts_with('+') && !line.starts_with("+++") {
                        "diff-add"
                    } else if line.starts_with('-') && !line.starts_with("---") {
                        "diff-rem"
                    } else if line.starts_with("@@") || line.starts_with("diff --git") {
                        "diff-meta"
                    } else {
                        "diff-ctx"
                    };
                    view! { <div class=class>{line.to_owned()}</div> }
                }).collect_view()}
            </pre>
        }.into_any(),
        DiffViewMode::Split => view! {
            <div class="diff-split-container">
                {diff.files.into_iter().map(|file| view! {
                    <div class="diff-file-block">
                        <div class="diff-file-title">{file.new_path}</div>
                        {file.hunks.into_iter().map(|hunk| view! {
                            <div class="diff-hunk-header">{hunk.header}</div>
                            {hunk.split_rows.into_iter().map(|row| view! {
                                <div class="diff-split-row">
                                    <SplitCell number=row.left_line content=row.left_content kind=row.left_kind />
                                    <SplitCell number=row.right_line content=row.right_content kind=row.right_kind />
                                </div>
                            }).collect_view()}
                        }).collect_view()}
                    </div>
                }).collect_view()}
            </div>
        }.into_any(),
    }
}

#[component]
fn SplitCell(
    #[prop(optional_no_strip)] number: Option<u32>,
    #[prop(optional_no_strip)] content: Option<String>,
    kind: DiffLineKind,
) -> impl IntoView {
    let class = match kind {
        DiffLineKind::Added => "diff-cell diff-added",
        DiffLineKind::Removed => "diff-cell diff-removed",
        DiffLineKind::Context => "diff-cell diff-context",
        DiffLineKind::Empty => "diff-cell diff-empty",
    };
    view! {
        <div class=class>
            <span class="diff-ln">{number.map(|number| number.to_string()).unwrap_or_default()}</span>
            <code>{content.unwrap_or_default()}</code>
        </div>
    }
}
