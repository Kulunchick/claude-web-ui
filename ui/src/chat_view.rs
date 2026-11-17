use claude_web_ui_shared::auth::{ProjectDto, Role, UserDto};
use claude_web_ui_shared::chat::{
    ApprovalMode, ChatSessionDto, ChatTurnDto, ChatTurnStatus, PendingPermissionDto, TurnEvent,
};
use leptos::prelude::*;

#[component]
pub fn ChatWorkspaceView(
    project: ProjectDto,
    current_user: UserDto,
    sessions: ReadSignal<Vec<ChatSessionDto>>,
    active_session_id: RwSignal<Option<String>>,
    turns: ReadSignal<Vec<ChatTurnDto>>,
    #[prop(optional)] pending_permission: Option<ReadSignal<Option<PendingPermissionDto>>>,
    #[prop(optional)] on_send_prompt: Option<Callback<String>>,
    #[prop(optional)] on_toggle_approval: Option<Callback<ApprovalMode>>,
    #[prop(optional)] on_resolve_permission: Option<Callback<bool>>,
    #[prop(optional)] on_stop_turn: Option<Callback<()>>,
) -> impl IntoView {
    let is_viewer = current_user.role == Role::Viewer;
    let search_query = RwSignal::new(String::new());
    let prompt_input = RwSignal::new(String::new());
    let show_terminal = RwSignal::new(false);
    let approval_mode = RwSignal::new(ApprovalMode::FullAuto);

    Effect::new(move |_| {
        if let Some(sid) = active_session_id.get()
            && let Some(sess) = sessions.get().into_iter().find(|s| s.id == sid)
        {
            approval_mode.set(sess.approval_mode);
        }
    });

    let filtered_sessions = move || {
        let q = search_query.get().trim().to_ascii_lowercase();
        sessions
            .get()
            .into_iter()
            .filter(|s| {
                q.is_empty()
                    || s.title.to_ascii_lowercase().contains(&q)
                    || s.native_session_id
                        .as_deref()
                        .is_some_and(|n| n.to_ascii_lowercase().contains(&q))
            })
            .collect::<Vec<_>>()
    };

    view! {
        <section class="chat-workspace" data-view="chat-workspace">
            <aside class="chat-sessions-panel panel-card" data-view="session-list">
                <div class="panel-header-row">
                    <h3>"Sessions — " {project.name.clone()}</h3>
                </div>
                <div class="form-group">
                    <input
                        type="search"
                        placeholder="Search sessions..."
                        prop:value=move || search_query.get()
                        on:input=move |ev| search_query.set(event_target_value(&ev))
                    />
                </div>
                <ul class="session-list">
                    <For
                        each=filtered_sessions
                        key=|s| s.id.clone()
                        children=move |sess| {
                            let sid = sess.id.clone();
                            let sid_click = sess.id.clone();
                            view! {
                                <li>
                                    <button
                                        class="session-item"
                                        class:active=move || active_session_id.get().as_deref() == Some(sid.as_str())
                                        on:click=move |_| active_session_id.set(Some(sid_click.clone()))
                                    >
                                        <span class="session-title">{sess.title}</span>
                                        <span class="role-badge">{sess.provider.display_name()}</span>
                                        <span class="session-source">{sess.source}</span>
                                    </button>
                                </li>
                            }
                        }
                    />
                </ul>
            </aside>

            <div class="chat-main-panel panel-card" data-view="chat-stream">
                <div class="panel-header-row">
                    <div>
                        <h3>"CLI Chat Stream"</h3>
                        <p class="project-path">{project.workspace_path}</p>
                    </div>
                    <Show when=move || !is_viewer>
                        <div class="btn-group">
                            <button
                                class="btn-secondary"
                                type="button"
                                on:click=move |_| {
                                    let next = match approval_mode.get() {
                                        ApprovalMode::FullAuto => ApprovalMode::Interactive,
                                        ApprovalMode::Interactive => ApprovalMode::FullAuto,
                                    };
                                    approval_mode.set(next);
                                    if let Some(cb) = on_toggle_approval {
                                        cb.run(next);
                                    }
                                }
                            >
                                {move || match approval_mode.get() {
                                    ApprovalMode::FullAuto => "Mode: Full Auto",
                                    ApprovalMode::Interactive => "Mode: Interactive Approval",
                                }}
                            </button>
                            <button
                                class="btn-secondary"
                                type="button"
                                on:click=move |_| show_terminal.update(|v| *v = !*v)
                            >
                                {move || if show_terminal.get() { "Hide Terminal" } else { "Project Terminal" }}
                            </button>
                            <button
                                class="btn-danger"
                                type="button"
                                on:click=move |_| {
                                    if let Some(cb) = on_stop_turn {
                                        cb.run(());
                                    }
                                }
                            >
                                "Stop"
                            </button>
                        </div>
                    </Show>
                </div>

                <Show when=move || is_viewer>
                    <div class="cli-readonly-pill">"Read-only viewer stream"</div>
                </Show>

                <div class="chat-feed">
                    <For
                        each=move || turns.get()
                        key=|t| t.id.clone()
                        children=move |turn| {
                            let status_label = turn.status.as_str().to_string();
                            let is_running = matches!(
                                turn.status,
                                ChatTurnStatus::Queued | ChatTurnStatus::Running | ChatTurnStatus::WaitingApproval
                            );
                            view! {
                                <article class="turn-card">
                                    <div class="turn-user">
                                        <strong>"Prompt"</strong>
                                        <span class="role-badge">{status_label}</span>
                                        <p>{turn.prompt}</p>
                                    </div>
                                    <div class="turn-events">
                                        {turn.events.into_iter().map(|ev| render_turn_event(ev)).collect_view()}
                                    </div>
                                    <Show when=move || is_running>
                                        <span class="subtitle">"Streaming..."</span>
                                    </Show>
                                </article>
                            }
                        }
                    />
                </div>

                {move || {
                    let perm = pending_permission.and_then(|sig| sig.get());
                    match (perm, is_viewer) {
                        (Some(p), false) => view! {
                            <div class="approval-card" data-view="approval-prompt">
                                <strong>"Permission Request: " {p.tool_name}</strong>
                                <pre>{p.input.to_string()}</pre>
                                <div class="btn-group">
                                    <button
                                        class="btn-primary"
                                        type="button"
                                        on:click=move |_| {
                                            if let Some(cb) = on_resolve_permission {
                                                cb.run(true);
                                            }
                                        }
                                    >
                                        "Allow"
                                    </button>
                                    <button
                                        class="btn-danger"
                                        type="button"
                                        on:click=move |_| {
                                            if let Some(cb) = on_resolve_permission {
                                                cb.run(false);
                                            }
                                        }
                                    >
                                        "Deny"
                                    </button>
                                </div>
                            </div>
                        }.into_any(),
                        _ => ().into_any(),
                    }
                }}

                <Show when=move || !is_viewer>
                    <div class="chat-composer">
                        <textarea
                            rows="3"
                            placeholder="Send prompt to active CLI..."
                            prop:value=move || prompt_input.get()
                            on:input=move |ev| prompt_input.set(event_target_value(&ev))
                        />
                        <button
                            class="btn-primary"
                            type="button"
                            on:click=move |_| {
                                let text = prompt_input.get();
                                if !text.trim().is_empty() {
                                    if let Some(cb) = on_send_prompt {
                                        cb.run(text);
                                    }
                                    prompt_input.set(String::new());
                                }
                            }
                        >
                            "Send"
                        </button>
                    </div>
                </Show>

                <Show when=move || show_terminal.get() && !is_viewer>
                    <div class="xterm-container" data-terminal="project-pty">
                        <p class="subtitle">"Connects via WebSocket to /api/projects/{id}/terminal/ws"</p>
                    </div>
                </Show>
            </div>
        </section>
    }
}

fn render_turn_event(ev: TurnEvent) -> impl IntoView {
    match ev {
        TurnEvent::TextDelta { text } => view! { <div class="event-text">{text}</div> }.into_any(),
        TurnEvent::ThinkingDelta { text } => view! {
            <details class="event-thinking">
                <summary>"Thinking"</summary>
                <pre>{text}</pre>
            </details>
        }
        .into_any(),
        TurnEvent::ToolCallStart { name, input, .. } => view! {
            <div class="event-tool-card">
                <strong>"Tool: " {name}</strong>
                <pre>{input.to_string()}</pre>
            </div>
        }
        .into_any(),
        TurnEvent::ToolCallResult {
            output, is_error, ..
        } => view! {
            <div class=if is_error { "event-tool-result error" } else { "event-tool-result" }>
                <pre>{output}</pre>
            </div>
        }
        .into_any(),
        TurnEvent::SubagentStart { name, .. } => {
            view! { <div class="role-badge">"Subagent started: " {name}</div> }.into_any()
        }
        TurnEvent::SubagentEnd { id } => {
            view! { <div class="role-badge">"Subagent finished: " {id}</div> }.into_any()
        }
        TurnEvent::PermissionRequest { tool_name, .. } => {
            view! { <div class="subtitle">"Requested permission: " {tool_name}</div> }.into_any()
        }
        TurnEvent::TurnComplete { result_text, .. } => {
            view! { <div class="event-complete">{result_text}</div> }.into_any()
        }
        TurnEvent::TurnError { message } => {
            view! { <div class="alert-error">{message}</div> }.into_any()
        }
    }
}
