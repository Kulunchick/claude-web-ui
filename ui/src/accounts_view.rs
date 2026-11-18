use claude_web_ui_shared::accounts::{CliProvider, ProjectCliStateDto};
use leptos::prelude::*;

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
