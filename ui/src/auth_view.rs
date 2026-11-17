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
    AdminUsersProjects,
}

#[component]
pub fn BootstrapAdminView(
    #[prop(optional)] initial_error: Option<String>,
) -> impl IntoView {
    let (username, set_username) = signal(String::new());
    let (email, set_email) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let error = RwSignal::new(initial_error);

    view! {
        <section class="auth-card" data-view="bootstrap-admin">
            <h2>"Initialize Gateway Admin"</h2>
            <p class="subtitle">"Create the first administrator account to secure this 24/7 gateway."</p>
            <Show when=move || error.get().is_some()>
                <div class="alert-error">{move || error.get().unwrap_or_default()}</div>
            </Show>
            <div class="form-group">
                <label>"Username"</label>
                <input
                    type="text"
                    placeholder="admin"
                    prop:value=move || username.get()
                    on:input=move |ev| set_username.set(event_target_value(&ev))
                />
            </div>
            <div class="form-group">
                <label>"Email"</label>
                <input
                    type="email"
                    placeholder="admin@example.com"
                    prop:value=move || email.get()
                    on:input=move |ev| set_email.set(event_target_value(&ev))
                />
            </div>
            <div class="form-group">
                <label>"Password"</label>
                <input
                    type="password"
                    placeholder="Minimum 8 characters"
                    prop:value=move || password.get()
                    on:input=move |ev| set_password.set(event_target_value(&ev))
                />
            </div>
            <button class="btn-primary" type="submit">"Create Admin Account"</button>
        </section>
    }
}

#[component]
pub fn LoginView(
    #[prop(optional)] on_switch_invite: Option<Callback<()>>,
) -> impl IntoView {
    let (identifier, set_identifier) = signal(String::new());
    let (password, set_password) = signal(String::new());

    view! {
        <section class="auth-card" data-view="login">
            <h2>"Sign In to Claude Web UI"</h2>
            <p class="subtitle">"Authenticate with your username or email and password."</p>
            <div class="form-group">
                <label>"Username or Email"</label>
                <input
                    type="text"
                    placeholder="username or email"
                    prop:value=move || identifier.get()
                    on:input=move |ev| set_identifier.set(event_target_value(&ev))
                />
            </div>
            <div class="form-group">
                <label>"Password"</label>
                <input
                    type="password"
                    placeholder="Password"
                    prop:value=move || password.get()
                    on:input=move |ev| set_password.set(event_target_value(&ev))
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
        </section>
    }
}

#[component]
pub fn InviteRedemptionView(
    #[prop(optional)] prefilled_code: Option<String>,
    #[prop(optional)] on_switch_login: Option<Callback<()>>,
) -> impl IntoView {
    let (code, set_code) = signal(prefilled_code.unwrap_or_default());
    let (username, set_username) = signal(String::new());
    let (email, set_email) = signal(String::new());
    let (password, set_password) = signal(String::new());

    view! {
        <section class="auth-card" data-view="invite-redemption">
            <h2>"Redeem Invite"</h2>
            <p class="subtitle">"Enter your single-use invite code and set your account credentials."</p>
            <div class="form-group">
                <label>"Invite Code"</label>
                <input
                    type="text"
                    placeholder="inv_..."
                    prop:value=move || code.get()
                    on:input=move |ev| set_code.set(event_target_value(&ev))
                />
            </div>
            <div class="form-group">
                <label>"Username"</label>
                <input
                    type="text"
                    prop:value=move || username.get()
                    on:input=move |ev| set_username.set(event_target_value(&ev))
                />
            </div>
            <div class="form-group">
                <label>"Email"</label>
                <input
                    type="email"
                    prop:value=move || email.get()
                    on:input=move |ev| set_email.set(event_target_value(&ev))
                />
            </div>
            <div class="form-group">
                <label>"Password"</label>
                <input
                    type="password"
                    prop:value=move || password.get()
                    on:input=move |ev| set_password.set(event_target_value(&ev))
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
        </section>
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
                <ul class="project-list">
                    <For
                        each=move || projects.get()
                        key=|p| p.id.clone()
                        children=move |project| {
                            let pid = project.id.clone();
                            let pid_select = project.id.clone();
                            view! {
                                <li>
                                    <button
                                        class="project-item"
                                        class:active=move || active_project_id.get().as_deref() == Some(pid.as_str())
                                        on:click=move |_| {
                                            active_project_id.set(Some(pid_select.clone()));
                                            active_tab.set(DashboardTab::ProjectOverview);
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
