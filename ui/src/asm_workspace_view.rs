//! Agents, Skills and MCP manager rendered from dashboard state. The
//! dashboard script owns loading and mutations; this view renders the element
//! ids and data attributes its handlers bind to.

use claude_web_ui_shared::accounts::CliProvider;
use claude_web_ui_shared::agents::{
    provider_model_tiers, ConfigScope, CLAUDE_BUILTIN_TOOLS, PERMISSION_MODES,
};
use leptos::prelude::*;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AsmWorkspaceState {
    pub provider: CliProvider,
    pub scope: ConfigScope,
    pub sub_tab: String,
    pub has_project: bool,
    pub can_edit: bool,
    #[serde(default)]
    pub raw_mode: bool,
    #[serde(default)]
    pub agents: Vec<AsmAgent>,
    #[serde(default)]
    pub selected_agent: Option<AsmAgent>,
    #[serde(default)]
    pub skills: Vec<AsmSkill>,
    #[serde(default)]
    pub selected_skill: Option<AsmSkill>,
    #[serde(default)]
    pub mcp_config: Option<AsmMcpConfig>,
    #[serde(default)]
    pub presets: Vec<AsmPreset>,
    #[serde(default)]
    pub graphify_result: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AsmAgent {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
    pub tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub permission_mode: Option<String>,
    pub system_prompt: String,
    pub raw_markdown: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AsmSkill {
    pub name: String,
    pub description: String,
    pub skill_md: String,
    pub companion_files: Vec<AsmCompanion>,
    pub is_shared: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AsmCompanion {
    pub relative_path: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AsmMcpConfig {
    pub config_path: String,
    pub servers: Vec<AsmMcpServer>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AsmMcpServer {
    pub name: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AsmPreset {
    pub id: String,
    pub display_name: String,
}

fn tab_class(active: bool) -> &'static str {
    if active {
        "btn-secondary active"
    } else {
        "btn-secondary"
    }
}

#[component]
pub fn AgentsSkillsMcpWorkspace(state: AsmWorkspaceState) -> impl IntoView {
    let provider = state.provider;
    let scope = state.scope;
    let sub_tab = state.sub_tab.clone();

    let body = if scope == ConfigScope::Project && !state.has_project {
        view! {
            <div class="panel-card">
                <p class="subtitle">"Select a project from the sidebar or switch to Global Scope."</p>
            </div>
        }
        .into_any()
    } else {
        match sub_tab.as_str() {
            "agents" => agents_body(&state).into_any(),
            "skills" => skills_body(&state).into_any(),
            _ => mcp_body(&state).into_any(),
        }
    };

    view! {
        <section class="panel-card" data-view="agents-skills-mcp">
            <div class="panel-header-row">
                <div>
                    <h2>"Agents, Skills & MCP Manager"</h2>
                    <p class="subtitle">"Native CLI subagents, skills with cross-CLI symlink sharing, and MCP presets."</p>
                </div>
                <div class="tab-bar">
                    <button class=tab_class(scope == ConfigScope::Project) data-asm-scope="project" type="button">"Project Scope"</button>
                    <button class=tab_class(scope == ConfigScope::Global) data-asm-scope="global" type="button">"Global Scope"</button>
                </div>
            </div>
            <div class="tab-bar">
                {CliProvider::ALL.into_iter().map(|p| view! {
                    <button class=tab_class(p == provider) data-asm-prov=p.as_str() type="button">{p.display_name()}</button>
                }).collect_view()}
            </div>
            <div class="tab-bar">
                <button class=tab_class(sub_tab == "agents") data-asm-sub="agents" type="button">"Subagents"</button>
                <button class=tab_class(sub_tab == "skills") data-asm-sub="skills" type="button">"Skills"</button>
                <button class=tab_class(sub_tab == "mcp") data-asm-sub="mcp" type="button">"MCP Servers"</button>
            </div>
        </section>
        {body}
    }
}

fn agents_body(state: &AsmWorkspaceState) -> impl IntoView + use<> {
    let can_edit = state.can_edit;
    let read_only = !can_edit;
    let tiers = provider_model_tiers(state.provider);
    let default_model = tiers[0].to_string();
    let sel = state.selected_agent.clone().unwrap_or_else(|| AsmAgent {
        model: Some(default_model.clone()),
        tools: vec!["Read".into(), "Grep".into()],
        permission_mode: Some("default".into()),
        raw_markdown: format!("---\nname: \ndescription: \nmodel: {default_model}\n---\n"),
        ..AsmAgent::default()
    });
    let sel_model = sel.model.clone().unwrap_or_default();
    let sel_perm = sel.permission_mode.clone().unwrap_or_default();
    let has_rows = !state.agents.is_empty();
    let rows = state
        .agents
        .iter()
        .map(|a| {
            let model = a.model.clone().unwrap_or_else(|| "inherit".into());
            view! {
                <div class="row">
                    <div>
                        <strong>{a.name.clone()}</strong>
                        " "
                        <span class="role-badge">{model}</span>
                        <p class="subtitle">{a.description.clone()}</p>
                    </div>
                    <div class="btn-group">
                        <button class="btn-secondary" data-asm-edit-agent=a.name.clone() type="button">"Edit"</button>
                        {can_edit.then(|| view! {
                            <button class="btn-danger" data-asm-del-agent=a.name.clone() type="button">"Delete"</button>
                        })}
                    </div>
                </div>
            }
        })
        .collect_view();

    let fields = if state.raw_mode {
        view! {
            <div class="form-group">
                <label>"Raw Markdown + YAML Frontmatter"</label>
                <textarea id="asm-agent-raw" name="raw_markdown" rows="12" disabled=read_only
                    prop:value=sel.raw_markdown.clone()></textarea>
            </div>
        }
        .into_any()
    } else {
        let tools = sel.tools.clone();
        view! {
            <div class="form-group">
                <label>"Description"</label>
                <input id="asm-agent-desc" value=sel.description.clone() disabled=read_only />
            </div>
            <div class="form-group">
                <label>"Model Tier"</label>
                <select id="asm-agent-model" disabled=read_only>
                    {tiers.iter().map(|m| view! {
                        <option value=*m selected=sel_model == *m>{*m}</option>
                    }).collect_view()}
                </select>
            </div>
            <div class="form-group">
                <label>"Allowed Tools"</label>
                <div class="checkbox-list">
                    {CLAUDE_BUILTIN_TOOLS.iter().map(|t| view! {
                        <label class="checkbox-item">
                            <input type="checkbox" name="asm_tools" value=*t
                                checked=tools.iter().any(|x| x == t) disabled=read_only />
                            " "
                            {*t}
                        </label>
                    }).collect_view()}
                </div>
            </div>
            <div class="form-group">
                <label>"Disallowed Tools (comma-separated)"</label>
                <input id="asm-agent-disallowed" value=sel.disallowed_tools.join(", ") disabled=read_only />
            </div>
            <div class="form-group">
                <label>"Permission Mode"</label>
                <select id="asm-agent-perm" disabled=read_only>
                    {PERMISSION_MODES.iter().map(|m| view! {
                        <option value=*m selected=sel_perm == *m>{*m}</option>
                    }).collect_view()}
                </select>
            </div>
            <div class="form-group">
                <label>"System Prompt"</label>
                <textarea id="asm-agent-prompt" rows="6" disabled=read_only
                    prop:value=sel.system_prompt.clone()></textarea>
            </div>
        }
        .into_any()
    };

    view! {
        <div class="admin-grid">
            <div class="panel-card">
                <h3>{format!("Configured Subagents ({})", state.provider.display_name())}</h3>
                {rows}
                {(!has_rows).then(|| view! { <p class="subtitle">"No subagents found in this scope."</p> })}
            </div>
            <form class="panel-card" id="asm-agent-form">
                <div class="panel-header-row">
                    <h3>"Subagent Editor"</h3>
                    <button class="btn-secondary" id="asm-toggle-raw-btn" type="button">
                        {if state.raw_mode { "Visual Form" } else { "Raw YAML / Markdown" }}
                    </button>
                </div>
                <div class="form-group">
                    <label>"Agent Name"</label>
                    <input id="asm-agent-name" value=sel.name.clone() required disabled=read_only />
                </div>
                {fields}
                {if can_edit {
                    view! { <button class="btn-primary" type="submit">"Save Subagent"</button> }.into_any()
                } else {
                    view! { <p class="subtitle">"Read-only access in this scope."</p> }.into_any()
                }}
            </form>
        </div>
    }
}

fn skills_body(state: &AsmWorkspaceState) -> impl IntoView + use<> {
    let can_edit = state.can_edit;
    let read_only = !can_edit;
    let sel = state.selected_skill.clone().unwrap_or_else(|| AsmSkill {
        skill_md: "---\nname: \ndescription: \n---\n# Instructions\n".into(),
        ..AsmSkill::default()
    });
    let has_rows = !state.skills.is_empty();
    let rows = state
        .skills
        .iter()
        .map(|s| {
            let shared = if s.is_shared { "true" } else { "false" };
            view! {
                <div class="row">
                    <div>
                        <strong>{s.name.clone()}</strong>
                        {s.is_shared.then(|| view! { " " <span class="role-badge">"Shared"</span> })}
                        <p class="subtitle">{s.description.clone()}</p>
                    </div>
                    <div class="btn-group">
                        <button class="btn-secondary" data-asm-edit-skill=s.name.clone() type="button">"Edit"</button>
                        {can_edit.then(|| view! {
                            <button class="btn-secondary" data-asm-share-skill=s.name.clone() data-shared=shared type="button">
                                {if s.is_shared { "Unshare" } else { "Share" }}
                            </button>
                            <button class="btn-danger" data-asm-del-skill=s.name.clone() type="button">"Delete"</button>
                        })}
                    </div>
                </div>
            }
        })
        .collect_view();
    let has_name = !sel.name.is_empty();
    let has_comps = !sel.companion_files.is_empty();
    let comp_rows = sel
        .companion_files
        .iter()
        .map(|c| {
            view! {
                <div class="row">
                    <code>{c.relative_path.clone()}</code>
                    {(can_edit && has_name).then(|| view! {
                        <button class="btn-danger" data-asm-del-comp=c.relative_path.clone() type="button">"Remove"</button>
                    })}
                </div>
            }
        })
        .collect_view();

    view! {
        <div class="admin-grid">
            <div class="panel-card">
                <h3>{format!("Skills ({})", state.provider.display_name())}</h3>
                {rows}
                {(!has_rows).then(|| view! { <p class="subtitle">"No skills found in this scope."</p> })}
            </div>
            <div class="panel-card">
                <form id="asm-skill-form">
                    <h3>"Skill & Companion Editor"</h3>
                    <div class="form-group">
                        <label>"Skill Name"</label>
                        <input id="asm-skill-name" value=sel.name.clone() required disabled=read_only />
                    </div>
                    <label class="checkbox-item" style="margin-bottom: 10px;">
                        <input type="checkbox" id="asm-skill-share-toggle" checked=sel.is_shared disabled=read_only />
                        " Share Skill across CLIs"
                    </label>
                    <div class="form-group">
                        <label>"SKILL.md"</label>
                        <textarea id="asm-skill-md" rows="8" disabled=read_only prop:value=sel.skill_md.clone()></textarea>
                    </div>
                    {can_edit.then(|| view! { <button class="btn-primary" type="submit">"Save Skill"</button> })}
                </form>
                <div style="margin-top: 16px;">
                    <h4>"Companion Files (scripts/, references/, examples/)"</h4>
                    {comp_rows}
                    {(!has_comps).then(|| view! { <p class="subtitle">"No companion files yet."</p> })}
                    {can_edit.then(|| view! {
                        <form id="asm-comp-form" style="margin-top: 10px;">
                            <div class="form-group">
                                <label>"Relative Path"</label>
                                <input id="asm-comp-path" value="scripts/helper.sh" required />
                            </div>
                            <div class="form-group">
                                <label>"Content"</label>
                                <textarea id="asm-comp-content" rows="4"></textarea>
                            </div>
                            <button class="btn-secondary" type="submit">"Save Companion File"</button>
                        </form>
                    })}
                </div>
            </div>
        </div>
    }
}

fn mcp_body(state: &AsmWorkspaceState) -> impl IntoView + use<> {
    let can_edit = state.can_edit;
    let read_only = !can_edit;
    let cfg = state.mcp_config.clone().unwrap_or_default();
    let has_servers = !cfg.servers.is_empty();
    let rows = cfg
        .servers
        .into_iter()
        .map(|s| {
            let target = s.command.clone().or(s.url.clone()).unwrap_or_default();
            let line = format!("{target} {}", s.args.join(" "));
            view! {
                <div class="row">
                    <div>
                        <strong>{s.name.clone()}</strong>
                        <div class="project-path"><code>{line}</code></div>
                    </div>
                    <div class="btn-group">
                        {can_edit.then(|| view! {
                            <button class="btn-secondary" data-asm-sync-mcp=s.name.clone() type="button">"Sync across CLIs"</button>
                            <button class="btn-danger" data-asm-del-mcp=s.name.clone() type="button">"Delete"</button>
                        })}
                    </div>
                </div>
            }
        })
        .collect_view();
    let presets: Vec<AsmPreset> = if state.presets.is_empty() {
        [
            ("graphify-mcp", "Graphify Knowledge Graph (graphify-mcp)"),
            ("chrome-devtools", "Chrome DevTools MCP"),
            ("playwright", "Playwright Browser MCP"),
        ]
        .into_iter()
        .map(|(id, name)| AsmPreset {
            id: id.into(),
            display_name: name.into(),
        })
        .collect()
    } else {
        state.presets.clone()
    };
    let show_graphify = state.scope == ConfigScope::Project && can_edit;
    let graphify = state.graphify_result.clone().filter(|r| !r.is_empty());

    view! {
        <div class="admin-grid">
            <div class="panel-card">
                <div class="panel-header-row">
                    <h3>"Native MCP Servers"</h3>
                    {show_graphify.then(|| view! {
                        <button class="btn-secondary" id="asm-run-graphify-btn" type="button">"Run Graphify Index"</button>
                    })}
                </div>
                <p class="project-path">{cfg.config_path}</p>
                {rows}
                {(!has_servers).then(|| view! { <p class="subtitle">"No MCP servers configured yet."</p> })}
                <h4 style="margin-top: 14px;">"One-Click Presets"</h4>
                <div class="btn-group">
                    {presets.into_iter().map(|p| view! {
                        <button class="btn-secondary" data-asm-preset=p.id type="button" disabled=read_only>
                            {format!("+ {}", p.display_name)}
                        </button>
                    }).collect_view()}
                </div>
                {graphify.map(|out| view! {
                    <pre class="xterm-fallback-log" style="margin-top: 12px;">{out}</pre>
                })}
            </div>
            <form class="panel-card" id="asm-mcp-form">
                <h3>"Add / Update MCP Server"</h3>
                <div class="form-group">
                    <label>"Server Name"</label>
                    <input id="asm-mcp-name" placeholder="my-mcp" required disabled=read_only />
                </div>
                <div class="form-group">
                    <label>"Command"</label>
                    <input id="asm-mcp-cmd" placeholder="npx" disabled=read_only />
                </div>
                <div class="form-group">
                    <label>"Args (space-separated)"</label>
                    <input id="asm-mcp-args" placeholder="-y @playwright/mcp@latest" disabled=read_only />
                </div>
                {can_edit.then(|| view! { <button class="btn-primary" type="submit">"Save MCP Server"</button> })}
            </form>
        </div>
    }
}
