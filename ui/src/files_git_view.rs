//! Workspace file tree and git diff view (Ticket #4).

use claude_web_ui_shared::auth::Role;
use claude_web_ui_shared::files::{
    DiffLineKind, DiffViewMode, FileContentResponse, FileTreeNode, FileTreeResponse,
    GitDiffResponse, GitStatusResponse,
};
use leptos::prelude::*;

#[component]
pub fn WorkspaceFilesGitView(
    #[prop(optional)] project_id: Option<String>,
    #[prop(default = Role::Viewer)] user_role: Role,
    #[prop(optional)] initial_tree: Option<FileTreeResponse>,
    #[prop(optional)] initial_file: Option<FileContentResponse>,
    #[prop(optional)] initial_git_status: Option<GitStatusResponse>,
    #[prop(optional)] initial_git_diff: Option<GitDiffResponse>,
) -> impl IntoView {
    let respect_gitignore = RwSignal::new(
        initial_tree
            .as_ref()
            .map(|t| t.respect_gitignore)
            .unwrap_or(true),
    );
    let diff_mode = RwSignal::new(
        initial_git_diff
            .as_ref()
            .map(|d| d.mode)
            .unwrap_or(DiffViewMode::Unified),
    );
    let selected_path = RwSignal::new(initial_file.as_ref().map(|f| f.path.clone()));
    let editor_content = RwSignal::new(
        initial_file
            .as_ref()
            .map(|f| f.content.clone())
            .unwrap_or_default(),
    );
    let is_read_only = user_role == Role::Viewer;
    let tree_nodes = initial_tree.map(|t| t.entries).unwrap_or_default();

    view! {
        <section class="files-git-shell" data-view="files-git">
            <div class="files-git-header">
                <div>
                    <h3>"Workspace Explorer & Live Git"</h3>
                    <span class="subtitle">
                        {project_id.unwrap_or_else(|| "No active project".to_string())}
                    </span>
                </div>
                <div class="files-git-controls">
                    <label class="toggle-label">
                        <input
                            type="checkbox"
                            prop:checked=move || respect_gitignore.get()
                            on:change=move |ev| respect_gitignore.set(event_target_checked(&ev))
                        />
                        <span>"Respect .gitignore"</span>
                    </label>
                    <button
                        class="btn-secondary"
                        type="button"
                        on:click=move |_| {
                            let next = match diff_mode.get() {
                                DiffViewMode::Unified => DiffViewMode::Split,
                                DiffViewMode::Split => DiffViewMode::Unified,
                            };
                            diff_mode.set(next);
                        }
                    >
                        {move || match diff_mode.get() {
                            DiffViewMode::Unified => "Diff: Unified",
                            DiffViewMode::Split => "Diff: Split",
                        }}
                    </button>
                </div>
            </div>

            <div class="files-git-grid">
                <aside class="file-tree-pane panel-card">
                    <h4>"Files"</h4>
                    <FileTreeList nodes=tree_nodes selected_path=selected_path />
                </aside>

                <div class="editor-pane panel-card">
                    <div class="editor-toolbar">
                        <strong>
                            {move || selected_path.get().unwrap_or_else(|| "Select a file".to_string())}
                        </strong>
                        <Show when=move || is_read_only>
                            <span class="role-badge">"Read-Only (Viewer)"</span>
                        </Show>
                        <Show when=move || !is_read_only>
                            <button class="btn-primary" type="button">"Save File"</button>
                        </Show>
                    </div>
                    <textarea
                        class="code-editor"
                        readonly=is_read_only
                        prop:value=move || editor_content.get()
                        on:input=move |ev| {
                            if !is_read_only {
                                editor_content.set(event_target_value(&ev));
                            }
                        }
                    />
                </div>

                <aside class="git-pane panel-card">
                    <h4>"Git Status & Diff"</h4>
                    {match initial_git_status {
                        Some(status) => view! {
                            <div class="git-status-summary">
                                <div class="row">
                                    <span>"Branch"</span>
                                    <strong>{status.branch.unwrap_or_else(|| "N/A".to_string())}</strong>
                                </div>
                                <p class="subtitle">
                                    {format!(
                                        "Staged: {} | Unstaged: {} | Untracked: {} | Commits: {}",
                                        status.staged.len(),
                                        status.unstaged.len(),
                                        status.untracked.len(),
                                        status.recent_commits.len()
                                    )}
                                </p>
                            </div>
                        }.into_any(),
                        None => view! { <p class="subtitle">"Git status loaded dynamically."</p> }.into_any(),
                    }}

                    {match initial_git_diff {
                        Some(diff) => view! {
                            <div class="git-diff-preview">
                                {move || match diff_mode.get() {
                                    DiffViewMode::Unified => view! {
                                        <pre class="diff-unified">{diff.unified.clone()}</pre>
                                    }.into_any(),
                                    DiffViewMode::Split => view! {
                                        <div class="diff-split-table">
                                            {diff.files.iter().flat_map(|f| f.hunks.iter()).flat_map(|h| h.split_rows.iter()).map(|row| {
                                                let left_cls = diff_kind_class(row.left_kind);
                                                let right_cls = diff_kind_class(row.right_kind);
                                                view! {
                                                    <div class="diff-split-row">
                                                        <span class=left_cls>{row.left_content.clone().unwrap_or_default()}</span>
                                                        <span class=right_cls>{row.right_content.clone().unwrap_or_default()}</span>
                                                    </div>
                                                }
                                            }).collect_view()}
                                        </div>
                                    }.into_any(),
                                }}
                            </div>
                        }.into_any(),
                        None => view! { <p class="subtitle">"Select a changed file to inspect diff."</p> }.into_any(),
                    }}
                </aside>
            </div>
        </section>
    }
}

#[component]
fn FileTreeList(nodes: Vec<FileTreeNode>, selected_path: RwSignal<Option<String>>) -> AnyView {
    view! {
        <ul class="file-tree-list">
            {nodes
                .into_iter()
                .map(|node| {
                    let path = node.path.clone();
                    let name = node.name.clone();
                    let is_dir = node.is_dir;
                    let children = node.children.clone();
                    let has_children = !children.is_empty();
                    view! {
                        <li class="file-tree-item">
                            <button
                                type="button"
                                class="tree-node-btn"
                                on:click=move |_| {
                                    if !is_dir {
                                        selected_path.set(Some(path.clone()));
                                    }
                                }
                            >
                                {if is_dir { format!("📁 {name}") } else { format!("📄 {name}") }}
                            </button>
                            {has_children.then(|| view! {
                                <FileTreeList nodes=children selected_path=selected_path />
                            })}
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
    .into_any()
}

fn diff_kind_class(kind: DiffLineKind) -> &'static str {
    match kind {
        DiffLineKind::Added => "diff-cell diff-added",
        DiffLineKind::Removed => "diff-cell diff-removed",
        DiffLineKind::Context => "diff-cell diff-context",
        DiffLineKind::Empty => "diff-cell diff-empty",
    }
}
