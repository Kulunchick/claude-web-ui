use claude_web_ui_shared::accounts::CliProvider;
use claude_web_ui_shared::agents::{
    provider_model_tiers, ConfigScope, McpPresetDto, ProviderMcpConfigDto, SkillDto, SubagentDto,
    CLAUDE_BUILTIN_TOOLS, PERMISSION_MODES,
};
use claude_web_ui_shared::auth::Role;
use leptos::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerSubTab {
    Subagents,
    Skills,
    McpServers,
}

#[component]
pub fn AgentsSkillsMcpView(
    current_role: Role,
    active_project_id: ReadSignal<Option<String>>,
    #[prop(default = Vec::new())] initial_subagents: Vec<SubagentDto>,
    #[prop(default = Vec::new())] initial_skills: Vec<SkillDto>,
    #[prop(optional_no_strip)] initial_mcp_config: Option<ProviderMcpConfigDto>,
    #[prop(default = Vec::new())] initial_presets: Vec<McpPresetDto>,
) -> impl IntoView {
    let selected_provider = RwSignal::new(CliProvider::ClaudeCode);
    let selected_scope = RwSignal::new(ConfigScope::Project);
    let sub_tab = RwSignal::new(ManagerSubTab::Subagents);

    let (subagents, _) = signal(initial_subagents);
    let (skills, _) = signal(initial_skills);
    let (mcp_config, _) = signal(initial_mcp_config);
    let (presets, _) = signal(initial_presets);

    // Visual Subagents form + Raw Markdown/YAML toggle
    let raw_mode = RwSignal::new(false);
    let agent_name = RwSignal::new(String::new());
    let agent_desc = RwSignal::new(String::new());
    let agent_model = RwSignal::new("sonnet".to_string());
    let agent_tools = RwSignal::new(vec!["Read".to_string(), "Grep".to_string()]);
    let agent_disallowed = RwSignal::new(String::new());
    let agent_perm = RwSignal::new("default".to_string());
    let agent_prompt = RwSignal::new(String::new());
    let agent_raw_md = RwSignal::new(String::new());

    // Skills form
    let skill_name = RwSignal::new(String::new());
    let skill_md = RwSignal::new(String::new());
    let skill_shared = RwSignal::new(false);
    let companion_path = RwSignal::new("scripts/helper.sh".to_string());
    let companion_content = RwSignal::new(String::new());

    // MCP form
    let mcp_name = RwSignal::new(String::new());
    let mcp_cmd = RwSignal::new(String::new());
    let mcp_args = RwSignal::new(String::new());

    let can_mutate = move || match selected_scope.get() {
        ConfigScope::Global => current_role == Role::Admin,
        ConfigScope::Project => current_role != Role::Viewer && active_project_id.get().is_some(),
    };

    view! {
        <section class="admin-panel" data-view="agents-skills-mcp">
            <div class="panel-header-row">
                <div>
                    <h2>"Agents, Skills & MCP Manager"</h2>
                    <p class="subtitle">"Configure native CLI subagents, skills, cross-CLI symlinks, and MCP presets."</p>
                </div>
                <div class="tab-bar">
                    <button
                        class="btn-secondary"
                        class:active=move || selected_scope.get() == ConfigScope::Project
                        type="button"
                        on:click=move |_| selected_scope.set(ConfigScope::Project)
                    >
                        "Project Scope"
                    </button>
                    <button
                        class="btn-secondary"
                        class:active=move || selected_scope.get() == ConfigScope::Global
                        type="button"
                        on:click=move |_| selected_scope.set(ConfigScope::Global)
                    >
                        "Global Scope"
                    </button>
                </div>
            </div>

            <div class="tab-bar" data-tabs="cli-provider">
                <button
                    class="btn-secondary"
                    class:active=move || selected_provider.get() == CliProvider::ClaudeCode
                    type="button"
                    on:click=move |_| {
                        selected_provider.set(CliProvider::ClaudeCode);
                        agent_model.set("sonnet".to_string());
                    }
                >
                    "Claude Code"
                </button>
                <button
                    class="btn-secondary"
                    class:active=move || selected_provider.get() == CliProvider::Codex
                    type="button"
                    on:click=move |_| {
                        selected_provider.set(CliProvider::Codex);
                        agent_model.set("gpt-5-codex".to_string());
                    }
                >
                    "Codex"
                </button>
                <button
                    class="btn-secondary"
                    class:active=move || selected_provider.get() == CliProvider::Antigravity
                    type="button"
                    on:click=move |_| {
                        selected_provider.set(CliProvider::Antigravity);
                        agent_model.set("pro".to_string());
                    }
                >
                    "Antigravity"
                </button>
            </div>

            <div class="tab-bar" data-tabs="manager-section">
                <button
                    class="btn-secondary"
                    class:active=move || sub_tab.get() == ManagerSubTab::Subagents
                    type="button"
                    on:click=move |_| sub_tab.set(ManagerSubTab::Subagents)
                >
                    "Subagents"
                </button>
                <button
                    class="btn-secondary"
                    class:active=move || sub_tab.get() == ManagerSubTab::Skills
                    type="button"
                    on:click=move |_| sub_tab.set(ManagerSubTab::Skills)
                >
                    "Skills"
                </button>
                <button
                    class="btn-secondary"
                    class:active=move || sub_tab.get() == ManagerSubTab::McpServers
                    type="button"
                    on:click=move |_| sub_tab.set(ManagerSubTab::McpServers)
                >
                    "MCP Servers"
                </button>
            </div>

            {move || match sub_tab.get() {
                ManagerSubTab::Subagents => view! {
                    <div class="admin-grid">
                        <div class="panel-card">
                            <h3>"Configured Subagents"</h3>
                            <For
                                each=move || subagents.get()
                                key=|a| a.name.clone()
                                children=move |agent| {
                                    view! {
                                        <div class="row">
                                            <div>
                                                <strong>{agent.name}</strong>
                                                " — "
                                                <span class="role-badge">{agent.model.unwrap_or_else(|| "inherit".to_string())}</span>
                                                <p class="subtitle">{agent.description}</p>
                                            </div>
                                        </div>
                                    }
                                }
                            />
                        </div>
                        <div class="panel-card">
                            <div class="panel-header-row">
                                <h3>"Subagent Editor"</h3>
                                <button
                                    class="btn-secondary"
                                    type="button"
                                    on:click=move |_| raw_mode.update(|v| *v = !*v)
                                >
                                    {move || if raw_mode.get() { "Visual Form" } else { "Raw YAML / Markdown" }}
                                </button>
                            </div>
                            <div class="form-group">
                                <label>"Agent Name"</label>
                                <input
                                    type="text"
                                    placeholder="architect"
                                    prop:value=move || agent_name.get()
                                    on:input=move |ev| agent_name.set(event_target_value(&ev))
                                />
                            </div>
                            <Show
                                when=move || !raw_mode.get()
                                fallback=move || view! {
                                    <div class="form-group">
                                        <label>"Raw Markdown + YAML Frontmatter"</label>
                                        <textarea
                                            rows="12"
                                            prop:value=move || agent_raw_md.get()
                                            on:input=move |ev| agent_raw_md.set(event_target_value(&ev))
                                        />
                                    </div>
                                }
                            >
                                <div class="form-group">
                                    <label>"Description"</label>
                                    <input
                                        type="text"
                                        prop:value=move || agent_desc.get()
                                        on:input=move |ev| agent_desc.set(event_target_value(&ev))
                                    />
                                </div>
                                <div class="form-group">
                                    <label>"Model Tier"</label>
                                    <select
                                        prop:value=move || agent_model.get()
                                        on:change=move |ev| agent_model.set(event_target_value(&ev))
                                    >
                                        <For
                                            each=move || provider_model_tiers(selected_provider.get()).iter().copied()
                                            key=|m| *m
                                            children=move |tier| view! { <option value=tier>{tier}</option> }
                                        />
                                    </select>
                                </div>
                                <div class="form-group">
                                    <label>"Allowed Tools"</label>
                                    <div class="checkbox-list">
                                        <For
                                            each=move || CLAUDE_BUILTIN_TOOLS.iter().copied()
                                            key=|t| *t
                                            children=move |tool| {
                                                let tool_name = tool.to_string();
                                                let tool_chk = tool_name.clone();
                                                let tool_toggle = tool_name.clone();
                                                view! {
                                                    <label class="checkbox-item">
                                                        <input
                                                            type="checkbox"
                                                            prop:checked=move || agent_tools.get().contains(&tool_chk)
                                                            on:change=move |_| {
                                                                agent_tools.update(|list| {
                                                                    if let Some(pos) = list.iter().position(|x| x == &tool_toggle) {
                                                                        list.remove(pos);
                                                                    } else {
                                                                        list.push(tool_toggle.clone());
                                                                    }
                                                                });
                                                            }
                                                        />
                                                        {tool}
                                                    </label>
                                                }
                                            }
                                        />
                                    </div>
                                </div>
                                <div class="form-group">
                                    <label>"Disallowed Tools (comma-separated)"</label>
                                    <input
                                        type="text"
                                        placeholder="Write, Edit"
                                        prop:value=move || agent_disallowed.get()
                                        on:input=move |ev| agent_disallowed.set(event_target_value(&ev))
                                    />
                                </div>
                                <div class="form-group">
                                    <label>"Permission Mode"</label>
                                    <select
                                        prop:value=move || agent_perm.get()
                                        on:change=move |ev| agent_perm.set(event_target_value(&ev))
                                    >
                                        <For
                                            each=move || PERMISSION_MODES.iter().copied()
                                            key=|m| *m
                                            children=move |mode| view! { <option value=mode>{mode}</option> }
                                        />
                                    </select>
                                </div>
                                <div class="form-group">
                                    <label>"System Prompt"</label>
                                    <textarea
                                        rows="6"
                                        prop:value=move || agent_prompt.get()
                                        on:input=move |ev| agent_prompt.set(event_target_value(&ev))
                                    />
                                </div>
                            </Show>
                            <button class="btn-primary" type="button" disabled=move || !can_mutate()>
                                "Save Subagent"
                            </button>
                        </div>
                    </div>
                }.into_any(),
                ManagerSubTab::Skills => view! {
                    <div class="admin-grid">
                        <div class="panel-card">
                            <h3>"Skills"</h3>
                            <For
                                each=move || skills.get()
                                key=|s| s.name.clone()
                                children=move |skill| {
                                    view! {
                                        <div class="row">
                                            <div>
                                                <strong>{skill.name}</strong>
                                                {if skill.is_shared {
                                                    Some(view! { <span class="role-badge">"Shared"</span> })
                                                } else {
                                                    None
                                                }}
                                                <p class="subtitle">{skill.description}</p>
                                            </div>
                                        </div>
                                    }
                                }
                            />
                        </div>
                        <div class="panel-card">
                            <h3>"Skill & Companion Editor"</h3>
                            <div class="form-group">
                                <label>"Skill Name"</label>
                                <input
                                    type="text"
                                    prop:value=move || skill_name.get()
                                    on:input=move |ev| skill_name.set(event_target_value(&ev))
                                />
                            </div>
                            <label class="checkbox-item">
                                <input
                                    type="checkbox"
                                    prop:checked=move || skill_shared.get()
                                    on:change=move |ev| skill_shared.set(event_target_checked(&ev))
                                />
                                "Share Skill across CLIs"
                            </label>
                            <div class="form-group">
                                <label>"SKILL.md"</label>
                                <textarea
                                    rows="8"
                                    prop:value=move || skill_md.get()
                                    on:input=move |ev| skill_md.set(event_target_value(&ev))
                                />
                            </div>
                            <div class="form-group">
                                <label>"Companion File Path (scripts/, references/, examples/)"</label>
                                <input
                                    type="text"
                                    prop:value=move || companion_path.get()
                                    on:input=move |ev| companion_path.set(event_target_value(&ev))
                                />
                            </div>
                            <div class="form-group">
                                <label>"Companion File Content"</label>
                                <textarea
                                    rows="4"
                                    prop:value=move || companion_content.get()
                                    on:input=move |ev| companion_content.set(event_target_value(&ev))
                                />
                            </div>
                            <button class="btn-primary" type="button" disabled=move || !can_mutate()>
                                "Save Skill"
                            </button>
                        </div>
                    </div>
                }.into_any(),
                ManagerSubTab::McpServers => view! {
                    <div class="admin-grid">
                        <div class="panel-card">
                            <div class="panel-header-row">
                                <h3>"Native MCP Servers"</h3>
                                <button class="btn-secondary" type="button" disabled=move || !can_mutate()>
                                    "Run Graphify Index"
                                </button>
                            </div>
                            <p class="project-path">
                                {move || mcp_config.get().map(|c| c.config_path).unwrap_or_default()}
                            </p>
                            {move || mcp_config.get().map(|cfg| {
                                view! {
                                    <div>
                                        <For
                                            each=move || cfg.servers.clone()
                                            key=|s| s.name.clone()
                                            children=move |srv| {
                                                view! {
                                                    <div class="row">
                                                        <div>
                                                            <strong>{srv.name}</strong>
                                                            " — "
                                                            <code>{srv.command.unwrap_or_default()}</code>
                                                        </div>
                                                    </div>
                                                }
                                            }
                                        />
                                    </div>
                                }
                            })}
                            <h4>"One-Click Presets"</h4>
                            <div class="btn-group">
                                <For
                                    each=move || presets.get()
                                    key=|p| p.id.clone()
                                    children=move |preset| {
                                        view! {
                                            <button class="btn-secondary" type="button">
                                                {preset.display_name}
                                            </button>
                                        }
                                    }
                                />
                            </div>
                        </div>
                        <div class="panel-card">
                            <h3>"Add / Edit MCP Server"</h3>
                            <div class="form-group">
                                <label>"Server Name"</label>
                                <input
                                    type="text"
                                    prop:value=move || mcp_name.get()
                                    on:input=move |ev| mcp_name.set(event_target_value(&ev))
                                />
                            </div>
                            <div class="form-group">
                                <label>"Command"</label>
                                <input
                                    type="text"
                                    prop:value=move || mcp_cmd.get()
                                    on:input=move |ev| mcp_cmd.set(event_target_value(&ev))
                                />
                            </div>
                            <div class="form-group">
                                <label>"Args (space-separated)"</label>
                                <input
                                    type="text"
                                    prop:value=move || mcp_args.get()
                                    on:input=move |ev| mcp_args.set(event_target_value(&ev))
                                />
                            </div>
                            <button class="btn-primary" type="button" disabled=move || !can_mutate()>
                                "Save MCP Server"
                            </button>
                        </div>
                    </div>
                }.into_any(),
            }}
        </section>
    }
}
