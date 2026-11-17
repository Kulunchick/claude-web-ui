use claude_web_ui_shared::auth::{ProjectDto, Role, UserDto};
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
) -> impl IntoView {
    view! {
        <section class="admin-panel" data-view="admin-users-projects">
            <h2>"Users, Invites & Project Workspaces"</h2>
            <div class="admin-grid">
                <div class="panel-card">
                    <h3>"Registered Projects"</h3>
                    <ul>
                        <For
                            each=move || projects.get()
                            key=|p| p.id.clone()
                            children=move |p| {
                                view! {
                                    <li>
                                        <strong>{p.name}</strong>
                                        " — "
                                        <code>{p.workspace_path}</code>
                                    </li>
                                }
                            }
                        />
                    </ul>
                </div>
                <div class="panel-card">
                    <h3>"Users"</h3>
                    <ul>
                        <For
                            each=move || users.get()
                            key=|u| u.id.clone()
                            children=move |u| {
                                view! {
                                    <li>
                                        <strong>{u.username}</strong>
                                        " (" {u.email} ") — "
                                        <span class="role-badge">{u.role.as_str()}</span>
                                    </li>
                                }
                            }
                        />
                    </ul>
                </div>
            </div>
        </section>
    }
}
