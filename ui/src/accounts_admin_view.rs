//! CLI account administration, developer account ACLs, and the account
//! authentication modal, rendered from dashboard state. The dashboard script
//! owns loading, mutations, and the OAuth terminal attached to
//! `#oauth-xterm-host`.

use claude_web_ui_shared::accounts::CliProvider;
use leptos::prelude::*;
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AccountsAdminState {
    pub accounts: Vec<AdminAccount>,
    pub projects: Vec<AclProject>,
    pub acl_project_id: Option<String>,
    pub acl_members: Vec<AclMember>,
    pub acl_list: Vec<AclEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdminAccount {
    pub id: String,
    pub name: String,
    pub provider: CliProvider,
    #[serde(default)]
    pub profile_dir: String,
    #[serde(default)]
    pub credential_files: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AclProject {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AclMember {
    pub user_id: String,
    pub username: String,
    pub role: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AclEntry {
    pub user_id: String,
    pub restricted: bool,
    pub allowed_accounts: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccountModalState {
    pub step: String,
    #[serde(default)]
    pub tab: String,
    #[serde(default)]
    pub account: Option<AdminAccount>,
}

#[component]
pub fn AccountsAdminWorkspace(state: AccountsAdminState) -> impl IntoView {
    let accounts = state.accounts.clone();
    let has_accounts = !accounts.is_empty();
    let cards = accounts
        .iter()
        .cloned()
        .map(|acc| {
            let creds = if acc.credential_files.is_empty() {
                "Not authenticated yet".to_string()
            } else {
                format!("Credentials: {}", acc.credential_files.join(", "))
            };
            view! {
                <article class="panel-card account-card">
                    <div class="panel-header-row">
                        <strong>{acc.name.clone()}</strong>
                        <span class="role-badge">{acc.provider.display_name()}</span>
                    </div>
                    <p class="project-path">{acc.profile_dir.clone()}</p>
                    <p class="subtitle">{creds}</p>
                    <div class="btn-group">
                        <button class="btn-secondary" data-auth-acc=acc.id.clone()>"OAuth / Credentials"</button>
                        <button class="btn-secondary" data-rename-acc=acc.id.clone()>"Rename"</button>
                        <button class="btn-danger" data-del-acc=acc.id.clone()>"Delete"</button>
                    </div>
                </article>
            }
        })
        .collect_view();

    let has_projects = !state.projects.is_empty();
    let selected_project = state.acl_project_id.clone();
    let project_options = state
        .projects
        .iter()
        .cloned()
        .map(|p| {
            let selected = selected_project.as_deref() == Some(p.id.as_str());
            view! { <option value=p.id selected=selected>{p.name}</option> }
        })
        .collect_view();

    let developers: Vec<AclMember> = state
        .acl_members
        .iter()
        .filter(|m| m.role == "developer")
        .cloned()
        .collect();
    let has_developers = !developers.is_empty();
    let acl_rows = developers
        .into_iter()
        .map(|m| {
            let acl = state
                .acl_list
                .iter()
                .find(|a| a.user_id == m.user_id)
                .cloned()
                .unwrap_or_default();
            let checkboxes = accounts
                .iter()
                .cloned()
                .map(|acc| {
                    let checked = acl.restricted && acl.allowed_accounts.contains(&acc.id);
                    let label = format!(" {} ({})", acc.name, acc.provider.display_name());
                    view! {
                        <label class="checkbox-item">
                            <input type="checkbox" data-acl-acc=m.user_id.clone() value=acc.id checked=checked />
                            {label}
                        </label>
                    }
                })
                .collect_view();
            view! {
                <div class="panel-card">
                    <div class="panel-header-row">
                        <strong>{m.username.clone()}</strong>
                        <label class="checkbox-item">
                            <input type="checkbox" data-acl-unrestricted=m.user_id.clone() checked=!acl.restricted />
                            " Unrestricted (All Accounts)"
                        </label>
                    </div>
                    <div class="checkbox-list">
                        {checkboxes}
                        {(!has_accounts).then(|| view! { <span class="subtitle">"No CLI accounts created yet."</span> })}
                    </div>
                    <button class="btn-secondary" data-save-acl=m.user_id.clone()>"Save ACL"</button>
                </div>
            }
        })
        .collect_view();

    view! {
        <section class="panel-card" data-view="cli-accounts">
            <div class="panel-header-row">
                <div>
                    <h2>"CLI Accounts & OAuth Profiles"</h2>
                    <p class="subtitle">"Isolated credentials per account with shared provider sessions & skills."</p>
                </div>
                <button class="btn-primary" id="open-add-account-btn" type="button">"+ Add Account"</button>
            </div>
            <div class="admin-grid">
                {cards}
                {(!has_accounts).then(|| view! { <p class="subtitle">"No CLI accounts yet."</p> })}
            </div>
        </section>
        <section class="panel-card">
            <h3>"Developer Account ACLs"</h3>
            <div class="form-group">
                <label>"Project"</label>
                <select id="acl-project-select">
                    {project_options}
                    {(!has_projects).then(|| view! { <option value="">"No projects"</option> })}
                </select>
            </div>
            {acl_rows}
            {(!has_developers).then(|| view! { <p class="subtitle">"No Developer members assigned to this project."</p> })}
        </section>
    }
}

#[component]
pub fn AccountAuthModal(modal: AccountModalState) -> impl IntoView {
    if modal.step == "create" {
        return view! {
            <div class="modal-backdrop">
                <div class="modal-card">
                    <div class="panel-header-row">
                        <h3>"Add Account"</h3>
                        <button class="btn-secondary" id="close-account-modal" type="button">"Close"</button>
                    </div>
                    <form id="create-account-form">
                        <div class="form-group">
                            <label>"CLI Provider"</label>
                            <select name="provider">
                                {CliProvider::ALL.into_iter().map(|p| view! {
                                    <option value=p.as_str()>{p.display_name()}</option>
                                }).collect_view()}
                            </select>
                        </div>
                        <div class="form-group">
                            <label>"Account Name"</label>
                            <input name="name" placeholder="e.g. Work, Personal" required />
                        </div>
                        <button class="btn-primary" type="submit">"Create Profile & Authenticate"</button>
                    </form>
                </div>
            </div>
        }
        .into_any();
    }

    let Some(acc) = modal.account else {
        return ().into_any();
    };
    let provider = acc.provider.display_name();
    let credentials_tab = modal.tab == "credentials";
    let tab_class = |active: bool| {
        if active {
            "btn-secondary active"
        } else {
            "btn-secondary"
        }
    };

    let body = if credentials_tab {
        let has_files = !acc.credential_files.is_empty();
        let files = acc
            .credential_files
            .iter()
            .cloned()
            .map(|f| {
                view! {
                    <div class="row">
                        <code>{f.clone()}</code>
                        <button class="btn-danger" data-del-cred=f type="button">"Delete"</button>
                    </div>
                }
            })
            .collect_view();
        view! {
            <form id="upload-cred-form">
                <div class="form-group">
                    <label>"Choose Credential File (.credentials.json, .claude.json, auth.json)"</label>
                    <input type="file" id="cred-file-picker" />
                </div>
                <div class="form-group">
                    <label>"Target Filename"</label>
                    <input id="cred-filename" name="filename" value=".credentials.json" required />
                </div>
                <div class="form-group">
                    <label>"Paste Credential Content"</label>
                    <textarea id="cred-content" name="content" rows="6" required></textarea>
                </div>
                <button class="btn-primary" type="submit">"Save Credential File"</button>
            </form>
            <div style="margin-top: 14px;">
                <h4>"Stored Credential Files"</h4>
                {files}
                {(!has_files).then(|| view! { <p class="subtitle">"No credential files uploaded yet."</p> })}
            </div>
        }
        .into_any()
    } else {
        view! {
            <div class="btn-group" style="margin-bottom: 10px;">
                <button class="btn-secondary" id="launch-oauth-pty-btn" type="button">"Launch OAuth Terminal"</button>
            </div>
            <div id="oauth-xterm-host" class="xterm-container">
                <pre id="oauth-xterm-fallback" class="xterm-fallback-log">
                    {format!("Click \"Launch OAuth Terminal\" to start {provider} login...")}
                </pre>
                <form id="oauth-fallback-input-form" style="display:none; gap:8px;">
                    <input id="oauth-fallback-input" placeholder="Paste OAuth code and press Enter..." />
                    <button class="btn-secondary" type="submit">"Send"</button>
                </form>
            </div>
        }
        .into_any()
    };

    view! {
        <div class="modal-backdrop">
            <div class="modal-card">
                <div class="panel-header-row">
                    <div>
                        <h3>{format!("Authenticate: {}", acc.name)}</h3>
                        <span class="role-badge">{provider}</span>
                    </div>
                    <button class="btn-secondary" id="close-account-modal" type="button">"Close"</button>
                </div>
                <div class="tab-bar">
                    <button class=tab_class(!credentials_tab) id="modal-tab-pty" type="button">"OAuth Terminal (xterm.js)"</button>
                    <button class=tab_class(credentials_tab) id="modal-tab-creds" type="button">"Upload / Paste Credentials"</button>
                </div>
                {body}
            </div>
        </div>
    }
    .into_any()
}
