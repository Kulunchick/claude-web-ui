use claude_web_ui_shared::accounts::{CliAccountDto, CliProvider, ProjectCliStateDto};
use claude_web_ui_shared::auth::{ProjectDto, UserDto};
use leptos::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddAccountAuthMode {
    Credentials,
    OAuthTerminal,
}

#[component]
pub fn HeaderCliSwitcher(
    project_cli: ReadSignal<Option<ProjectCliStateDto>>,
    #[prop(optional)] on_switch: Option<Callback<(CliProvider, Option<String>)>>,
) -> impl IntoView {
    let initial = project_cli.get_untracked();
    let selected_provider = RwSignal::new(
        initial
            .as_ref()
            .map(|cli| cli.active_provider)
            .unwrap_or(CliProvider::ClaudeCode),
    );
    let selected_account = RwSignal::new(
        initial
            .and_then(|cli| cli.active_account_id)
            .unwrap_or_default(),
    );

    Effect::new(move |_| {
        if let Some(cli) = project_cli.get() {
            selected_provider.set(cli.active_provider);
            selected_account.set(cli.active_account_id.unwrap_or_default());
        }
    });

    view! {
        <header class="dashboard-header" data-view="header-cli-switcher">
            <div class="cli-switcher">
                {move || match project_cli.get() {
                    None => view! {
                        <span class="subtitle">"Select a project to view active CLI"</span>
                    }.into_any(),
                    Some(cli) if !cli.can_switch => view! {
                        <div class="cli-readonly-pill">
                            <span>"Active CLI: "</span>
                            <strong>{cli.active_provider.display_name()}</strong>
                            <span class="role-badge">"Read-Only"</span>
                        </div>
                    }.into_any(),
                    Some(cli) => {
                        let accounts = cli.available_accounts.clone();
                        view! {
                            <div class="cli-switch-controls">
                                <label for="header-cli-provider">"CLI Provider"</label>
                                <select
                                    id="header-cli-provider"
                                    prop:value=move || selected_provider.get().as_str()
                                    on:change=move |ev| {
                                        if let Ok(p) = event_target_value(&ev).parse::<CliProvider>() {
                                            selected_provider.set(p);
                                            if let Some(cb) = on_switch {
                                                cb.run((p, None));
                                            }
                                        }
                                    }
                                >
                                    <option value="claude_code">"Claude Code"</option>
                                    <option value="codex">"Codex"</option>
                                    <option value="antigravity">"Antigravity"</option>
                                </select>
                                <label for="header-cli-account">"Account"</label>
                                <select
                                    id="header-cli-account"
                                    prop:value=move || selected_account.get()
                                    on:change=move |ev| {
                                        let val = event_target_value(&ev);
                                        selected_account.set(val.clone());
                                        if let Some(cb) = on_switch {
                                            let acc = if val.is_empty() { None } else { Some(val) };
                                            cb.run((selected_provider.get(), acc));
                                        }
                                    }
                                >
                                    <option value="">"— Unassigned —"</option>
                                    <For
                                        each=move || {
                                            let prov = selected_provider.get();
                                            accounts
                                                .iter()
                                                .filter(|a| a.provider == prov)
                                                .cloned()
                                                .collect::<Vec<_>>()
                                        }
                                        key=|a| a.id.clone()
                                        children=move |acc| {
                                            view! {
                                                <option value=acc.id.clone()>{acc.name}</option>
                                            }
                                        }
                                    />
                                </select>
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </header>
    }
}

#[component]
pub fn AccountsManagerView(
    accounts: ReadSignal<Vec<CliAccountDto>>,
    projects: ReadSignal<Vec<ProjectDto>>,
    users: ReadSignal<Vec<UserDto>>,
) -> impl IntoView {
    let show_modal = RwSignal::new(false);
    let modal_provider = RwSignal::new(CliProvider::ClaudeCode);
    let modal_name = RwSignal::new(String::new());
    let auth_mode = RwSignal::new(AddAccountAuthMode::OAuthTerminal);
    let cred_filename = RwSignal::new(".credentials.json".to_string());
    let cred_content = RwSignal::new(String::new());
    let selected_project_id = RwSignal::new(String::new());

    view! {
        <section class="admin-panel" data-view="admin-accounts">
            <div class="panel-header-row">
                <h2>"CLI Accounts & OAuth Profiles"</h2>
                <button
                    class="btn-primary"
                    type="button"
                    on:click=move |_| show_modal.set(true)
                >
                    "+ Add Account"
                </button>
            </div>

            <div class="admin-grid">
                <For
                    each=move || accounts.get()
                    key=|a| a.id.clone()
                    children=move |acc| {
                        let creds = acc.credential_files.join(", ");
                        let status_label = if acc.has_credentials {
                            format!("Credentials: {creds}")
                        } else {
                            "Not authenticated yet".to_string()
                        };
                        view! {
                            <article class="panel-card account-card">
                                <div class="panel-header-row">
                                    <strong>{acc.name}</strong>
                                    <span class="role-badge">{acc.provider.display_name()}</span>
                                </div>
                                <p class="project-path">{acc.profile_dir}</p>
                                <p class="subtitle">{status_label}</p>
                            </article>
                        }
                    }
                />
            </div>

            <div class="panel-card">
                <h3>"Developer Account ACLs"</h3>
                <div class="form-group">
                    <label>"Project"</label>
                    <select
                        prop:value=move || selected_project_id.get()
                        on:change=move |ev| selected_project_id.set(event_target_value(&ev))
                    >
                        <option value="">"Select project..."</option>
                        <For
                            each=move || projects.get()
                            key=|p| p.id.clone()
                            children=move |p| {
                                view! { <option value=p.id.clone()>{p.name}</option> }
                            }
                        />
                    </select>
                </div>
                <ul>
                    <For
                        each=move || users.get()
                        key=|u| u.id.clone()
                        children=move |u| {
                            view! {
                                <li class="row">
                                    <span>{u.username} " (" {u.role.as_str()} ")"</span>
                                </li>
                            }
                        }
                    />
                </ul>
            </div>

            <Show when=move || show_modal.get()>
                <div class="modal-backdrop" data-view="add-account-modal">
                    <div class="modal-card">
                        <div class="panel-header-row">
                            <h3>"Add CLI Account"</h3>
                            <button
                                class="btn-secondary"
                                type="button"
                                on:click=move |_| show_modal.set(false)
                            >
                                "Close"
                            </button>
                        </div>
                        <div class="form-group">
                            <label>"Provider"</label>
                            <select
                                prop:value=move || modal_provider.get().as_str()
                                on:change=move |ev| {
                                    if let Ok(p) = event_target_value(&ev).parse::<CliProvider>() {
                                        modal_provider.set(p);
                                    }
                                }
                            >
                                <option value="claude_code">"Claude Code"</option>
                                <option value="codex">"Codex"</option>
                                <option value="antigravity">"Antigravity"</option>
                            </select>
                        </div>
                        <div class="form-group">
                            <label>"Account Name"</label>
                            <input
                                type="text"
                                placeholder="Work / Personal"
                                prop:value=move || modal_name.get()
                                on:input=move |ev| modal_name.set(event_target_value(&ev))
                            />
                        </div>
                        <div class="tab-bar">
                            <button
                                class="btn-secondary"
                                class:active=move || auth_mode.get() == AddAccountAuthMode::OAuthTerminal
                                type="button"
                                on:click=move |_| auth_mode.set(AddAccountAuthMode::OAuthTerminal)
                            >
                                "OAuth Terminal (xterm.js)"
                            </button>
                            <button
                                class="btn-secondary"
                                class:active=move || auth_mode.get() == AddAccountAuthMode::Credentials
                                type="button"
                                on:click=move |_| auth_mode.set(AddAccountAuthMode::Credentials)
                            >
                                "Upload / Paste Credentials"
                            </button>
                        </div>
                        {move || match auth_mode.get() {
                            AddAccountAuthMode::OAuthTerminal => view! {
                                <div class="xterm-container" data-terminal="oauth-pty">
                                    <p class="subtitle">"Connects via WebSocket to /api/admin/accounts/{id}/login-pty"</p>
                                </div>
                            }.into_any(),
                            AddAccountAuthMode::Credentials => view! {
                                <div class="credentials-form">
                                    <div class="form-group">
                                        <label>"Credential Filename"</label>
                                        <input
                                            type="text"
                                            prop:value=move || cred_filename.get()
                                            on:input=move |ev| cred_filename.set(event_target_value(&ev))
                                        />
                                    </div>
                                    <div class="form-group">
                                        <label>"Credential JSON / Content"</label>
                                        <textarea
                                            rows="6"
                                            prop:value=move || cred_content.get()
                                            on:input=move |ev| cred_content.set(event_target_value(&ev))
                                        />
                                    </div>
                                </div>
                            }.into_any(),
                        }}
                    </div>
                </div>
            </Show>
        </section>
    }
}
