use claude_web_ui_shared::kanban::{KanbanCardDto, KanbanColumn as Column};
use leptos::prelude::*;
use serde_json::{Value, json};

#[derive(Clone)]
enum Action {
    Refresh,
    Create(Value),
    Edit(String, Value),
    Move(String, Column, Option<i64>),
    Decompose(String),
    Diff(String),
}

#[component]
pub fn KanbanView(
    project_id: String,
    #[prop(optional)] access_token: Option<String>,
    #[prop(default = false)] read_only: bool,
    #[prop(default = Vec::new())] initial_cards: Vec<KanbanCardDto>,
    open_session: Callback<String>,
) -> impl IntoView {
    let cards = RwSignal::new(initial_cards);
    let editor = RwSignal::new(None::<Option<String>>);
    let title = RwSignal::new(String::new());
    let description = RwSignal::new(String::new());
    let branch = RwSignal::new(String::new());
    let dedicated = RwSignal::new(false);
    let blockers = RwSignal::new(String::new());
    let dragged = RwSignal::new(None::<String>);
    let error = RwSignal::new(String::new());
    let diff = RwSignal::new(None::<String>);
    let endpoint = StoredValue::new(format!("/api/projects/{project_id}/kanban"));
    let token = StoredValue::new(access_token);
    let submit = Callback::new(move |action: Action| {
        #[cfg(target_arch = "wasm32")]
        if let Some(token) = token.get_value() {
            let endpoint = endpoint.get_value();
            leptos::task::spawn_local(async move {
                error.set(String::new());
                if let Action::Diff(id) = action {
                    match request_diff(&endpoint, &token, &id).await {
                        Ok(result) => diff.set(Some(result)),
                        Err(message) => error.set(message),
                    }
                    return;
                }
                match request(&endpoint, &token, action).await {
                    Ok(result) => {
                        cards.set(result);
                        editor.set(None);
                    }
                    Err(message) => error.set(message),
                }
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (token, endpoint, cards, editor);
            let _ = request_details(action);
        }
    });
    if token.with_value(Option::is_some) {
        Effect::new(move |_| submit.run(Action::Refresh));
    }
    let edit = Callback::new(move |card: Option<KanbanCardDto>| {
        title.set(
            card.as_ref()
                .map(|card| card.title.clone())
                .unwrap_or_default(),
        );
        description.set(
            card.as_ref()
                .map(|card| card.description.clone())
                .unwrap_or_default(),
        );
        dedicated.set(card.as_ref().is_some_and(|card| card.create_git_branch));
        branch.set(
            card.as_ref()
                .and_then(|card| card.git_branch_name.clone())
                .unwrap_or_default(),
        );
        blockers.set(
            card.as_ref()
                .map(|card| card.depends_on.join(", "))
                .unwrap_or_default(),
        );
        editor.set(Some(card.map(|card| card.id)));
    });
    let save = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let depends_on: Vec<_> = blockers
            .get()
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect();
        let branch = branch.get();
        let mut values = json!({
            "title": title.get(),
            "description": description.get(),
            "create_git_branch": dedicated.get(),
            "git_branch_name": (!branch.trim().is_empty()).then_some(branch)
        });
        if let Some(entry) = editor.get() {
            if !entry.as_ref().is_some_and(|id| {
                cards
                    .get()
                    .iter()
                    .any(|card| &card.id == id && card.column == Column::Running)
            }) {
                values["depends_on"] = json!(depends_on);
            }
            submit.run(match entry {
                Some(id) => Action::Edit(id, values),
                None => Action::Create(values),
            });
        }
    };
    view! {
        <section class="panel-card kanban-panel">
            <div class="pane-head">
                <h2>"Kanban"</h2>
                <button type="button" class="btn-secondary" on:click=move |_| submit.run(Action::Refresh)>"Refresh"</button>
                <Show when=move || !read_only>
                    <button type="button" class="btn-primary" on:click=move |_| edit.run(None)>"New card"</button>
                </Show>
            </div>
            <Show when=move || !error.get().is_empty()><p role="alert">{move || error.get()}</p></Show>
            <Show when=move || editor.get().is_some() && !read_only>
                <form class="kanban-form" on:submit=save>
                    <label>"Title"<input required prop:value=move || title.get() on:input=move |ev| title.set(event_target_value(&ev))/></label>
                    <label>"Description"<textarea prop:value=move || description.get() on:input=move |ev| description.set(event_target_value(&ev)) /></label>
                    <label>"Dependency card IDs"<input prop:value=move || blockers.get()
                        on:input=move |ev| blockers.set(event_target_value(&ev))/></label>
                    <label><input type="checkbox" prop:checked=move || dedicated.get()
                        on:change=move |ev| dedicated.set(event_target_checked(&ev))/>"Create dedicated git branch"</label>
                    <label>"Branch name"<input prop:value=move || branch.get() on:input=move |ev| branch.set(event_target_value(&ev))/></label>
                    <div>
                        <button type="submit" class="btn-primary">"Save card"</button>
                        <button type="button" class="btn-secondary" on:click=move |_| editor.set(None)>"Cancel"</button>
                    </div>
                </form>
            </Show>
            <Show when=move || diff.get().is_some()>
                <section class="panel-card">
                    <h3>"Card git diff"</h3>
                    <pre>{move || diff.get().unwrap_or_default()}</pre>
                    <button type="button" class="btn-secondary" on:click=move |_| diff.set(None)>"Close diff"</button>
                </section>
            </Show>
            <div class="kanban-board">
                {Column::ALL.into_iter().map(|column| {
                    view! {
                        <section class="kanban-lane"
                            on:dragover=move |ev| { if dragged.get_untracked().is_some() { ev.prevent_default(); } }
                            on:drop=move |ev| {
                                ev.prevent_default();
                                if let Some(id) = dragged.get_untracked() {
                                    submit.run(Action::Move(id, column, None));
                                    dragged.set(None);
                                }
                            }>
                            <h3>{column_label(column)}</h3>
                            {move || cards.get().into_iter().filter(|card| card.column == column)
                                .enumerate().map(|(index, card)| {
                                    let id = StoredValue::new(card.id.clone());
                                    let details = StoredValue::new(card.clone());
                                    let waiting = card.unresolved_blockers.iter().map(|id| {
                                        cards.with_untracked(|cards| cards.iter().find(|other| &other.id == id)
                                            .map(|other| other.title.clone()).unwrap_or_else(|| id.clone()))
                                    }).collect::<Vec<_>>().join(", ");
                                    view! {
                                        <article class="kanban-card" draggable=(!read_only).to_string()
                                            on:dragstart=move |_| dragged.set(Some(id.get_value()))
                                            on:dragend=move |_| dragged.set(None)
                                            on:drop=move |ev| {
                                                ev.prevent_default();
                                                ev.stop_propagation();
                                                if let Some(source) = dragged.get_untracked().filter(|source| source != &id.get_value()) {
                                                    let source_index = cards.with_untracked(|cards| cards.iter()
                                                        .filter(|card| card.column == column)
                                                        .position(|card| card.id == source));
                                                    let target = index - usize::from(source_index.is_some_and(|position| position < index));
                                                    submit.run(Action::Move(source, column, Some(target as i64)));
                                                }
                                                dragged.set(None);
                                            }>
                                            {if let Some(session) = card.session_id.clone() {
                                                view! { <button type="button" class="kanban-title"
                                                    on:click=move |_| open_session.run(session.clone())>{card.title}</button> }.into_any()
                                            } else { view! { <strong>{card.title}</strong> }.into_any() }}
                                            <p>{card.description}</p>
                                            {(!waiting.is_empty()).then(|| view!{ <p>"Waiting on " {waiting}</p> })}
                                            {card.session_status.map(|status| view!{ <p>"CLI session " {status.as_str().replace('_', " ")}</p> })}
                                            {card.git_branch_name.map(|name| view!{ <p>"Branch " <code>{name}</code></p> })}
                                            {card.blocked_reason.map(|reason| view!{ <p>{reason}</p> })}
                                            {card.result_summary.map(|summary| view!{ <p>{summary}</p> })}
                                            <div class="kanban-actions">
                                                {card.session_id.map(|_| view! {
                                                    <button type="button" class="btn-secondary"
                                                        on:click=move |_| submit.run(Action::Diff(id.get_value()))>"Git diff"</button>
                                                })}
                                                <Show when=move || !read_only>
                                                    <button type="button" class="btn-secondary"
                                                        on:click=move |_| edit.run(Some(details.get_value()))>"Edit"</button>
                                                    <Show when=move || column == Column::Triage>
                                                        <button type="button" class="btn-secondary"
                                                            on:click=move |_| submit.run(Action::Decompose(id.get_value()))>"Decompose with Agent"</button>
                                                    </Show>
                                                    <select aria-label="Move card" on:change=move |ev| {
                                                        let value = event_target_value(&ev);
                                                        if let Some(column) = Column::ALL.into_iter().find(|column| column.as_str() == value) {
                                                            submit.run(Action::Move(id.get_value(), column, None));
                                                        }
                                                    }>
                                                        {Column::ALL.into_iter().map(|destination| view! {
                                                            <option value=destination.as_str() selected={destination == column}>{column_label(destination)}</option>
                                                        }).collect_view()}
                                                    </select>
                                                </Show>
                                            </div>
                                        </article>
                                    }
                                }).collect_view()}
                        </section>
                    }
                }).collect_view()}
            </div>
        </section>
    }
}

fn column_label(column: Column) -> &'static str {
    match column {
        Column::Triage => "Triage",
        Column::Ready => "Ready",
        Column::Running => "Running",
        Column::Blocked => "Blocked",
        Column::Review => "Review",
        Column::Done => "Done",
        Column::Archived => "Archived",
    }
}

fn request_details(action: Action) -> Option<(&'static str, String, Value)> {
    match action {
        Action::Refresh => None,
        Action::Diff(id) => {
            let _ = id;
            None
        }
        Action::Create(values) => Some(("POST", String::new(), values)),
        Action::Edit(id, values) => Some(("PATCH", format!("/{id}"), values)),
        Action::Move(id, column, position) => Some((
            "POST",
            format!("/{id}/move"),
            json!({ "column": column, "position": position }),
        )),
        Action::Decompose(id) => Some(("POST", format!("/{id}/decompose"), json!({}))),
    }
}

#[cfg(target_arch = "wasm32")]
async fn request(
    endpoint: &str,
    token: &str,
    action: Action,
) -> Result<Vec<KanbanCardDto>, String> {
    use gloo_net::http::Request;
    let auth = format!("Bearer {token}");
    let error = |err: gloo_net::Error| err.to_string();
    if let Some((method, suffix, values)) = request_details(action) {
        let url = format!("{endpoint}{suffix}");
        let request = if method == "PATCH" {
            Request::patch(&url)
        } else {
            Request::post(&url)
        };
        let response = request
            .header("Authorization", &auth)
            .json(&values)
            .map_err(error)?
            .send()
            .await
            .map_err(error)?;
        if !response.ok() {
            return Err(response
                .text()
                .await
                .unwrap_or_else(|_| response.status_text()));
        }
    }
    let response = Request::get(endpoint)
        .header("Authorization", &auth)
        .send()
        .await
        .map_err(error)?;
    if !response.ok() {
        return Err(response.status_text());
    }
    response.json().await.map_err(error)
}

#[cfg(target_arch = "wasm32")]
async fn request_diff(endpoint: &str, token: &str, id: &str) -> Result<String, String> {
    let response = gloo_net::http::Request::get(&format!("{endpoint}/{id}/diff"))
        .header("Authorization", &format!("Bearer {token}"))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(response.status_text());
    }
    let data: Value = response.json().await.map_err(|err| err.to_string())?;
    Ok(match data["unified"].as_str() {
        Some("") => "No tracked file changes.",
        Some(diff) => diff,
        None => "No diff recorded.",
    }
    .to_owned())
}
