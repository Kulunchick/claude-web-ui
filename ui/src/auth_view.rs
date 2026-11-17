use claude_web_ui_shared::auth::{InviteDto, ProjectDto, Role, UserDto};
use leptos::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthScreen {
    Bootstrap,
    Login,
    RedeemInvite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardTab {
    ProjectOverview,
    AgentsSkillsMcp,
    AdminUsersProjects,
    AdminAccounts,
}

#[component]
pub fn BootstrapAdminView(
    #[prop(optional_no_strip)] initial_error: Option<String>,
) -> impl IntoView {
    view! {
        <form method="post" class="auth-card" id="bootstrap-form" data-view="bootstrap-admin">
            <h2>"Initialize Gateway Admin"</h2>
            <p class="subtitle">"Create the first administrator account to secure this 24/7 gateway."</p>
            {initial_error.map(|error| view! { <div class="alert-error" role="alert">{error}</div> })}
            <div class="form-group">
                <label for="auth-username">"Username"</label>
                <input
                    id="auth-username"
                    name="username"
                    required
                    autocomplete="username"
                    type="text"
                    placeholder="admin"
                />
            </div>
            <div class="form-group">
                <label for="auth-email">"Email"</label>
                <input
                    id="auth-email"
                    name="email"
                    required
                    autocomplete="email"
                    type="email"
                    placeholder="admin@example.com"
                />
            </div>
            <div class="form-group">
                <label for="auth-password">"Password"</label>
                <input
                    id="auth-password"
                    name="password"
                    required
                    autocomplete="new-password"
                    type="password"
                    placeholder="Minimum 8 characters"
                />
            </div>
            <button class="btn-primary" type="submit">"Create Admin Account"</button>
        </form>
    }
}

#[component]
pub fn LoginView(
    #[prop(optional_no_strip)] initial_error: Option<String>,
    #[prop(optional)] on_switch_invite: Option<Callback<()>>,
) -> impl IntoView {
    view! {
        <form method="post" class="auth-card" id="login-form" data-view="login">
            <h2>"Sign In to Claude Web UI"</h2>
            <p class="subtitle">"Authenticate with your username or email and password."</p>
            {initial_error.map(|error| view! { <div class="alert-error" role="alert">{error}</div> })}
            <div class="form-group">
                <label for="auth-identifier">"Username or Email"</label>
                <input
                    id="auth-identifier"
                    name="identifier"
                    required
                    autocomplete="username"
                    type="text"
                    placeholder="username or email"
                />
            </div>
            <div class="form-group">
                <label for="auth-password">"Password"</label>
                <input
                    id="auth-password"
                    name="password"
                    required
                    autocomplete="current-password"
                    type="password"
                    placeholder="Password"
                />
            </div>
            <button class="btn-primary" type="submit">"Sign In"</button>
            <Show when=move || on_switch_invite.is_some()>
                <button
                    class="btn-link"
                    type="button"
                    on:click=move |_| {
                        if let Some(cb) = on_switch_invite {
                            cb.run(());
                        }
                    }
                >
                    "Have an invite code? Redeem Invite"
                </button>
            </Show>
        </form>
    }
}

#[component]
pub fn InviteRedemptionView(
    #[prop(optional_no_strip)] initial_error: Option<String>,
    #[prop(optional)] prefilled_code: Option<String>,
    #[prop(optional)] on_switch_login: Option<Callback<()>>,
) -> impl IntoView {
    view! {
        <form method="post" class="auth-card" id="invite-form" data-view="invite-redemption">
            <h2>"Redeem Invite"</h2>
            <p class="subtitle">"Enter your single-use invite code and set your account credentials."</p>
            {initial_error.map(|error| view! { <div class="alert-error" role="alert">{error}</div> })}
            <div class="form-group">
                <label for="auth-code">"Invite Code"</label>
                <input
                    id="auth-code"
                    name="code"
                    required
                    type="text"
                    placeholder="inv_..."
                    value=prefilled_code.unwrap_or_default()
                />
            </div>
            <div class="form-group">
                <label for="auth-username">"Username"</label>
                <input
                    id="auth-username"
                    name="username"
                    required
                    autocomplete="username"
                    type="text"
                />
            </div>
            <div class="form-group">
                <label for="auth-email">"Email"</label>
                <input
                    id="auth-email"
                    name="email"
                    required
                    autocomplete="email"
                    type="email"
                />
            </div>
            <div class="form-group">
                <label for="auth-password">"Password"</label>
                <input
                    id="auth-password"
                    name="password"
                    required
                    autocomplete="new-password"
                    type="password"
                />
            </div>
            <button class="btn-primary" type="submit">"Redeem & Join"</button>
            <Show when=move || on_switch_login.is_some()>
                <button
                    class="btn-link"
                    type="button"
                    on:click=move |_| {
                        if let Some(cb) = on_switch_login {
                            cb.run(());
                        }
                    }
                >
                    "Back to Sign In"
                </button>
            </Show>
        </form>
    }
}

#[component]
pub fn ProjectList(
    projects: ReadSignal<Vec<ProjectDto>>,
    active_project_id: ReadSignal<Option<String>>,
    on_select: Callback<String>,
) -> impl IntoView {
    view! {
        <ul class="project-list" data-view="project-switcher">
            <Show when=move || projects.with(Vec::is_empty)>
                <li class="subtitle">"No assigned projects"</li>
            </Show>
            <For
                each=move || projects.get()
                key=|project| project.id.clone()
                children=move |project| {
                    let id = project.id.clone();
                    let select_id = project.id.clone();
                    view! {
                        <li>
                            <button
                                type="button"
                                class="project-item"
                                class:active=move || active_project_id.with(|active| active.as_ref() == Some(&id))
                                aria-pressed=move || active_project_id.with(|active| active.as_ref() == Some(&select_id)).to_string()
                                on:click={
                                    let project_id = project.id;
                                    move |_| on_select.run(project_id.clone())
                                }
                            >
                                <span class="project-name">{project.name}</span>
                                <span class="project-path">{project.workspace_path}</span>
                            </button>
                        </li>
                    }
                }
            />
        </ul>
    }
}

#[component]
pub fn ProjectSwitcherSidebar(
    projects: ReadSignal<Vec<ProjectDto>>,
    active_project_id: RwSignal<Option<String>>,
    current_user: UserDto,
    active_tab: RwSignal<DashboardTab>,
) -> impl IntoView {
    let is_admin = current_user.role == Role::Admin;
    let role_label = current_user.role.as_str().to_string();

    view! {
        <aside class="sidebar" data-view="project-switcher">
            <div class="brand">
                <h1>"Claude Web UI"</h1>
                <span class="role-badge">{role_label}</span>
            </div>
            <div class="sidebar-section">
                <h3>"Projects"</h3>
                <ProjectList
                    projects=projects
                    active_project_id=active_project_id.read_only()
                    on_select=Callback::new(move |id| {
                        active_project_id.set(Some(id));
                        active_tab.set(DashboardTab::ProjectOverview);
                    })
                />
            </div>
            <div class="sidebar-section">
                <h3>"Workspace"</h3>
                <button
                    class="nav-item"
                    class:active=move || active_tab.get() == DashboardTab::AgentsSkillsMcp
                    on:click=move |_| active_tab.set(DashboardTab::AgentsSkillsMcp)
                >
                    "Agents, Skills & MCP"
                </button>
            </div>
            <Show when=move || is_admin>
                <div class="sidebar-section">
                    <button
                        class="nav-item"
                        class:active=move || active_tab.get() == DashboardTab::AdminUsersProjects
                        on:click=move |_| active_tab.set(DashboardTab::AdminUsersProjects)
                    >
                        "Admin: Users & Projects"
                    </button>
                    <button
                        class="nav-item"
                        class:active=move || active_tab.get() == DashboardTab::AdminAccounts
                        on:click=move |_| active_tab.set(DashboardTab::AdminAccounts)
                    >
                        "Admin: CLI Accounts"
                    </button>
                </div>
            </Show>
        </aside>
    }
}

#[component]
pub fn AdminUsersProjectsView(
    users: ReadSignal<Vec<UserDto>>,
    projects: ReadSignal<Vec<ProjectDto>>,
    #[prop(default = Vec::new())] invites: Vec<InviteDto>,
) -> impl IntoView {
    view! {
        <section class="admin-panel" data-view="admin-users-projects">
            <div class="admin-grid">
                <form class="panel-card" id="create-project-form">
                    <h3>"Register Project Workspace"</h3>
                    <div class="form-group">
                        <label for="admin-project-name">"Project Name"</label>
                        <input id="admin-project-name" name="name" required />
                    </div>
                    <div class="form-group">
                        <label for="admin-workspace-path">"Workspace Path"</label>
                        <input id="admin-workspace-path" name="workspace_path" required />
                    </div>
                    <button class="btn-primary" type="submit">"Register Project"</button>
                </form>
                <form class="panel-card" id="create-invite-form">
                    <h3>"Generate Single-Use Invite"</h3>
                    <div class="form-group">
                        <label for="admin-invite-role">"Role"</label>
                        <select id="admin-invite-role" name="role">
                            <option value="developer">"Developer"</option>
                            <option value="viewer">"Viewer"</option>
                            <option value="admin">"Admin"</option>
                        </select>
                    </div>
                    <div class="form-group">
                        <label for="admin-invite-projects">"Allowed Projects"</label>
                        <select id="admin-invite-projects" name="project_ids" multiple>
                            <For
                                each=move || projects.get()
                                key=|project| project.id.clone()
                                children=|project| view!{ <option value=project.id>{project.name}</option> }
                            />
                        </select>
                    </div>
                    <button class="btn-primary" type="submit">"Create Invite"</button>
                </form>
            </div>
            <div class="panel-card">
                <h3>"Users"</h3>
                <Show when=move || users.with(Vec::is_empty)><p class="subtitle">"No users."</p></Show>
                <For
                    each=move || users.get()
                    key=|user| user.id.clone()
                    children=|user| view! {
                        <div class="row">
                            <div>
                                <strong>{user.username}</strong> " (" {user.email} ") "
                                <span class="role-badge">{user.role.as_str()}</span>
                            </div>
                            <button type="button" class="btn-danger" data-revoke-uid=user.id>"Revoke Sessions"</button>
                        </div>
                    }
                />
            </div>
            <div class="panel-card">
                <h3>"Invites"</h3>
                {invites.is_empty().then(|| view! { <p class="subtitle">"No invites yet."</p> })}
                {invites.into_iter().map(|invite| view!{
                    <div class="row">
                        <div><code>{invite.code}</code> " " {invite.role.as_str()}</div>
                        <span>{if invite.used_by.is_some() { "Used" } else { "Available" }}</span>
                    </div>
                }).collect_view()}
            </div>
        </section>
    }
}
