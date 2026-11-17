pub mod accounts_view;
pub mod agents_skills_mcp_view;
pub mod auth_view;
pub mod chat_view;
pub mod cron_view;
pub mod files_git_view;
pub mod kanban_view;

use accounts_view::{AccountsManagerView, HeaderCliSwitcher};
use auth_view::{
    AdminUsersProjectsView, AuthScreen, BootstrapAdminView, DashboardTab, InviteRedemptionView,
    LoginView, ProjectSwitcherSidebar,
};
use chat_view::ChatWorkspaceView;
use claude_web_ui_shared::accounts::{CliAccountDto, ProjectCliStateDto};
use claude_web_ui_shared::auth::{ProjectDto, UserDto};
use claude_web_ui_shared::chat::{ChatSessionDto, ChatTurnDto};
use leptos::prelude::*;

#[component]
pub fn GatewayApp(
    #[prop(default = true)] initialized: bool,
    #[prop(optional)] user: Option<UserDto>,
    #[prop(default = Vec::new())] initial_projects: Vec<ProjectDto>,
    #[prop(default = Vec::new())] initial_users: Vec<UserDto>,
    #[prop(default = Vec::new())] initial_accounts: Vec<CliAccountDto>,
    #[prop(optional)] initial_project_cli: Option<ProjectCliStateDto>,
    #[prop(default = Vec::new())] initial_sessions: Vec<ChatSessionDto>,
    #[prop(default = Vec::new())] initial_turns: Vec<ChatTurnDto>,
) -> impl IntoView {
    let initial_screen = if initialized {
        AuthScreen::Login
    } else {
        AuthScreen::Bootstrap
    };
    let screen = RwSignal::new(initial_screen);
    let (projects, _) = signal(initial_projects);
    let (users, _) = signal(initial_users);
    let (accounts, _) = signal(initial_accounts);
    let (project_cli, _) = signal(initial_project_cli);
    let (sessions, _) = signal(initial_sessions);
    let (turns, _) = signal(initial_turns);
    let active_project_id = RwSignal::new(None::<String>);
    let active_session_id = RwSignal::new(None::<String>);
    let active_tab = RwSignal::new(DashboardTab::ProjectOverview);

    match user {
        None => view! {
            <main class="auth-shell">
                {move || match screen.get() {
                    AuthScreen::Bootstrap => view! { <BootstrapAdminView /> }.into_any(),
                    AuthScreen::Login => view! {
                        <LoginView on_switch_invite=Callback::new(move |_| screen.set(AuthScreen::RedeemInvite)) />
                    }.into_any(),
                    AuthScreen::RedeemInvite => view! {
                        <InviteRedemptionView on_switch_login=Callback::new(move |_| screen.set(AuthScreen::Login)) />
                    }.into_any(),
                }}
            </main>
        }
        .into_any(),
        Some(current_user) => {
            let chat_user = current_user.clone();
            view! {
                <div class="dashboard-shell">
                    <ProjectSwitcherSidebar
                        projects=projects
                        active_project_id=active_project_id
                        current_user=current_user
                        active_tab=active_tab
                    />
                    <main class="dashboard-main">
                        <HeaderCliSwitcher project_cli=project_cli />
                        {move || match active_tab.get() {
                            DashboardTab::ProjectOverview => {
                                let selected = active_project_id
                                    .get()
                                    .and_then(|pid| projects.get().into_iter().find(|p| p.id == pid));
                                match selected {
                                    Some(proj) => view! {
                                        <ChatWorkspaceView
                                            project=proj
                                            current_user=chat_user.clone()
                                            sessions=sessions
                                            active_session_id=active_session_id
                                            turns=turns
                                        />
                                    }.into_any(),
                                    None => view! {
                                        <section class="panel-card">
                                            <h2>"Project Workspace"</h2>
                                            <p>"Select a project from the sidebar."</p>
                                        </section>
                                    }.into_any(),
                                }
                            }
                            DashboardTab::AdminUsersProjects => view! {
                                <AdminUsersProjectsView users=users projects=projects />
                            }.into_any(),
                            DashboardTab::AdminAccounts => view! {
                                <AccountsManagerView accounts=accounts projects=projects users=users />
                            }.into_any(),
                        }}
                    </main>
                </div>
            }
            .into_any()
        }
    }
}
