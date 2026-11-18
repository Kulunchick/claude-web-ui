//! Project chat workspace: session list, Gateway turn stream or native
//! transcript, prompt composer, approvals, and the project terminal host.
//! Rendered from dashboard state; the dashboard script owns the streams,
//! mutations, and the xterm.js terminal attached to `#project-xterm-host`.

use claude_web_ui_shared::accounts::CliProvider;
use leptos::prelude::*;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ChatWorkspaceState {
    pub project_name: String,
    pub workspace_path: String,
    pub is_viewer: bool,
    pub sessions: Vec<ChatSessionItem>,
    pub active_session_id: Option<String>,
    pub session_search: String,
    pub handoff_provider: Option<CliProvider>,
    pub has_active_session: bool,
    pub approval_mode: String,
    pub has_active_turn: bool,
    pub show_terminal: bool,
    pub can_switch_source: bool,
    pub native_source: bool,
    pub native_messages: Vec<NativeMessage>,
    pub turns: Vec<ChatTurn>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatSessionItem {
    pub id: String,
    pub title: String,
    pub provider: CliProvider,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub native_session_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NativeMessage {
    pub role: String,
    pub text: String,
    pub tool_name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ChatTurn {
    pub id: String,
    pub prompt: String,
    pub status: String,
    pub events: Vec<Value>,
    pub error_message: Option<String>,
    pub pending_permission: Option<PendingPermission>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PendingPermission {
    pub request_id: String,
    pub tool_name: String,
    pub input: Value,
}

const EMPTY_FEED: &str = "Select or create a session and send a prompt.";

fn str_field(event: &Value, key: &str) -> String {
    match event.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

fn status_class(status: &str) -> &'static str {
    match status {
        "running" | "in_progress" | "active" => "badge badge-success",
        "failed" | "error" | "cancelled" => "badge badge-destructive",
        _ => "badge badge-secondary",
    }
}

fn turn_event(event: Value) -> AnyView {
    let kind = event.get("type").and_then(Value::as_str).unwrap_or_default();
    match kind {
        "text_delta" => view! { <div class="event-text">{str_field(&event, "text")}</div> }.into_any(),
        "thinking_delta" => view! {
            <details class="event-thinking"><summary>"Thinking"</summary><pre>{str_field(&event, "text")}</pre></details>
        }
        .into_any(),
        "tool_call_start" => {
            let input = event.get("input").cloned().unwrap_or(Value::Null);
            view! {
                <div class="event-tool-card">
                    <strong>{format!("Tool: {}", str_field(&event, "name"))}</strong>
                    <pre>{pretty(&input)}</pre>
                </div>
            }
            .into_any()
        }
        "tool_call_result" => {
            let is_error = event.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            let class = if is_error { "event-tool-result error" } else { "event-tool-result " };
            view! { <div class=class><pre>{str_field(&event, "output")}</pre></div> }.into_any()
        }
        "subagent_start" => view! {
            <div class="badge badge-secondary">{format!("Subagent started: {}", str_field(&event, "name"))}</div>
        }
        .into_any(),
        "subagent_end" => view! {
            <div class="badge badge-secondary">{format!("Subagent finished: {}", str_field(&event, "id"))}</div>
        }
        .into_any(),
        "permission_request" => view! {
            <div class="subtitle event-note">{format!("Requested approval for {}", str_field(&event, "tool_name"))}</div>
        }
        .into_any(),
        "turn_complete" => {
            let text = str_field(&event, "result_text");
            (!text.is_empty())
                .then(|| view! { <div class="event-complete">{text}</div> })
                .into_any()
        }
        "turn_error" => view! { <div class="alert-error">{str_field(&event, "message")}</div> }.into_any(),
        _ => ().into_any(),
    }
}

/// Native CLI transcript records; also mounted on its own when the live
/// transcript socket refreshes the feed.
#[component]
pub fn NativeTranscriptFeed(messages: Vec<NativeMessage>) -> impl IntoView {
    let empty = messages.is_empty();
    view! {
        {messages.into_iter().map(|m| {
            let heading = match &m.tool_name {
                Some(tool) if !tool.is_empty() => format!("{} ({tool})", m.role),
                _ => m.role.clone(),
            };
            view! {
                <article class="turn-card">
                    <strong>{heading}</strong>
                    <pre class="event-text">{m.text}</pre>
                </article>
            }
        }).collect_view()}
        {empty.then(|| view! { <p class="subtitle">{EMPTY_FEED}</p> })}
    }
}

#[component]
pub fn ChatWorkspace(state: ChatWorkspaceState) -> impl IntoView {
    let is_viewer = state.is_viewer;
    let active_id = state.active_session_id.clone();
    let has_sessions = !state.sessions.is_empty();
    let sessions = state
        .sessions
        .into_iter()
        .map(|s| {
            let class = if active_id.as_deref() == Some(s.id.as_str()) {
                "session-item active"
            } else {
                "session-item "
            };
            view! {
                <li>
                    <button class=class data-sid=s.id>
                        <div class="panel-header-row session-item-head">
                            <strong class="truncate">{s.title}</strong>
                            <span class="badge badge-secondary">{s.provider.display_name()}</span>
                        </div>
                        <div class="subtitle session-item-meta">
                            <span>{s.source}</span>
                            {s.native_session_id.map(|native| view! { " \u{2022} " <code>{native}</code> })}
                        </div>
                    </button>
                </li>
            }
        })
        .collect_view();

    let feed = if state.native_source {
        view! { <NativeTranscriptFeed messages=state.native_messages /> }.into_any()
    } else if state.turns.is_empty() {
        view! { <p class="subtitle">{EMPTY_FEED}</p> }.into_any()
    } else {
        state
            .turns
            .into_iter()
            .map(|t| {
                let status_badge = status_class(&t.status);
                let approval = t.pending_permission.filter(|_| !is_viewer).map(|p| {
                    view! {
                        <div class="approval-card" data-view="approval-prompt">
                            <div class="approval-card-head">
                                <span class="badge badge-warning">"Approval"</span>
                                <strong>{format!("Permission Request: {}", p.tool_name)}</strong>
                            </div>
                            <pre>{pretty(&p.input)}</pre>
                            <div class="btn-group">
                                <button class="btn-primary btn-sm" data-allow-turn=t.id.clone() data-req-id=p.request_id.clone() type="button">"Allow"</button>
                                <button class="btn-danger btn-sm" data-deny-turn=t.id.clone() data-req-id=p.request_id type="button">"Deny"</button>
                            </div>
                        </div>
                    }
                });
                view! {
                    <article class="turn-card">
                        <div class="panel-header-row turn-card-head">
                            <strong>"Prompt"</strong>
                            <span class=status_badge>{t.status}</span>
                        </div>
                        <p class="turn-prompt">{t.prompt}</p>
                        <div class="turn-events">{t.events.into_iter().map(turn_event).collect_view()}</div>
                        {t.error_message.map(|e| view! { <div class="alert-error">{e}</div> })}
                        {approval}
                    </article>
                }
            })
            .collect_view()
            .into_any()
    };

    let approval_label = if state.approval_mode == "interactive" {
        "Mode: Interactive Approval"
    } else {
        "Mode: Full Auto"
    };
    let has_active_session = state.has_active_session;
    let approval_pressed = if state.approval_mode == "interactive" { "true" } else { "false" };
    let live = has_active_session && (state.has_active_turn || state.native_source);
    let controls = if is_viewer {
        view! { <span class="badge badge-secondary">"Read-Only Stream"</span> }.into_any()
    } else {
        view! {
            <div class="btn-group chat-controls">
                {state.handoff_provider.map(|p| view! {
                    <button class="btn-secondary btn-sm" id="chat-handoff" data-provider=p.as_str() type="button">
                        {format!("Continue with {}", p.display_name())}
                    </button>
                })}
                {has_active_session.then(|| view! {
                    <button class="btn-outline btn-sm approval-mode-toggle" id="toggle-approval-mode-btn" type="button" aria-pressed=approval_pressed>{approval_label}</button>
                })}
                <button class="btn-secondary btn-sm" id="toggle-project-terminal-btn" type="button">
                    {if state.show_terminal { "Hide Terminal" } else { "Project Terminal" }}
                </button>
                {(has_active_session && state.has_active_turn).then(|| view! {
                    <button class="btn-danger btn-sm" id="stop-active-turn-btn" type="button">"Stop"</button>
                })}
            </div>
        }
        .into_any()
    };

    let terminal = (state.show_terminal && !is_viewer).then(|| {
        view! {
            <div class="chat-terminal">
                <div class="panel-header-row chat-terminal-head">
                    <strong>"Project Terminal (xterm.js)"</strong>
                    <div class="btn-group">
                        <button class="btn-secondary btn-sm" id="term-mode-shell" type="button">"Workspace Shell"</button>
                        <button class="btn-secondary btn-sm" id="term-mode-cli" type="button">"Resume CLI"</button>
                        <button class="btn-ghost btn-sm" id="close-project-term" type="button">"Close"</button>
                    </div>
                </div>
                <div id="project-xterm-host" class="xterm-container">
                    <pre id="project-xterm-fallback" class="xterm-fallback-log">"Connecting to project terminal..."</pre>
                    <form id="project-xterm-fallback-form" class="inline-form" style="display:none;">
                        <input id="project-xterm-fallback-input" placeholder="Type command and press Enter..." />
                        <button class="btn-secondary btn-sm" type="submit">"Send"</button>
                    </form>
                </div>
            </div>
        }
    });

    view! {
        <section class="chat-workspace" data-view="chat-workspace">
            <div class="chat-main" data-view="chat-stream">
                <div class="chat-main-head">
                    <div class="chat-title">
                        <h2 class="m-0 truncate">{state.project_name}</h2>
                        <p class="project-path m-0 truncate">{state.workspace_path}</p>
                    </div>
                    {live.then(|| view! { <span class="badge badge-success" id="chat-live-indicator">"Live"</span> })}
                    {controls}
                </div>

                {state.can_switch_source.then(|| view! {
                    <button class="btn-secondary btn-sm chat-source-toggle" id="chat-feed-source" type="button">
                        {if state.native_source { "Gateway Turns" } else { "Native Transcript" }}
                    </button>
                })}
                <div class="chat-feed">{feed}</div>

                {(!is_viewer).then(|| view! {
                    <form id="chat-prompt-form" class="chat-composer">
                        <textarea id="chat-prompt-input" rows="3" placeholder="Send prompt to active CLI..." required></textarea>
                        <div class="btn-group chat-composer-actions">
                            <button class="btn-primary" type="submit">"Send Prompt"</button>
                        </div>
                    </form>
                })}

                {terminal}
            </div>

            <button class="chat-rail-scrim" id="chat-rail-scrim" type="button" tabindex="-1" aria-label="Close sessions"></button>
            <aside class="chat-rail" id="chat-rail" data-view="session-list">
                <div class="panel-header-row chat-rail-head">
                    <h3>"Sessions"</h3>
                    <div class="btn-group">
                        {(!is_viewer).then(|| view! {
                            <button class="btn-primary btn-sm" id="new-chat-session-btn" type="button">"+ New Chat"</button>
                        })}
                        <button class="btn-ghost btn-sm chat-rail-close" id="chat-rail-close" type="button" aria-label="Close sessions">"Close"</button>
                    </div>
                </div>
                <div class="form-group">
                    <input id="session-search-input" type="search" placeholder="Search sessions..." value=state.session_search />
                </div>
                <ul class="session-list">
                    {sessions}
                    {(!has_sessions).then(|| view! { <li class="subtitle">"No sessions yet."</li> })}
                </ul>
            </aside>
        </section>
    }
}
