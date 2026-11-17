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
    let blockers = RwSignal::new(Vec::<String>::new());
    let new_column = RwSignal::new(Column::Triage);
    let dragged = RwSignal::new(None::<String>);
    let error = RwSignal::new(String::new());
    let diff = RwSignal::new(None::<String>);
    let endpoint = StoredValue::new(format!("/api/projects/{project_id}/kanban"));
    let token = StoredValue::new(access_token);
    #[cfg(target_arch = "wasm32")]
    let request_generation = RwSignal::new(0u64);
    #[cfg(target_arch = "wasm32")]
    let mutation_pending = RwSignal::new(false);
    let submit = Callback::new(move |action: Action| {
        #[cfg(target_arch = "wasm32")]
        if let Some(token) = token.get_value() {
            let endpoint = endpoint.get_value();
            let refreshing = matches!(action, Action::Refresh);
            if mutation_pending.get_untracked() {
                return;
            }
            let mutating = !matches!(action, Action::Refresh | Action::Diff(_));
            if mutating {
                mutation_pending.set(true);
            }
            let generation = request_generation.get_untracked() + 1;
            request_generation.set(generation);
            leptos::task::spawn_local(async move {
                if !refreshing {
                    let _ = error.try_set(String::new());
                }
                if let Action::Diff(id) = action {
                    match request_diff(&endpoint, &token, &id).await {
                        Ok(result) => {
                            let _ = diff.try_set(Some(result));
                        }
                        Err(message) => {
                            let _ = error.try_set(message);
                        }
                    }
                    return;
                }
                let result = request(&endpoint, &token, action).await;
                if mutating {
                    let _ = mutation_pending.try_set(false);
                }
                if request_generation.try_get_untracked() != Some(generation) {
                    return;
                }
                match result {
                    Ok(result) => {
                        let _ = error.try_set(String::new());
                        if cards.try_with_untracked(|current| current != &result) == Some(true) {
                            let _ = cards.try_set(result);
                        }
                        if !refreshing {
                            let _ = editor.try_set(None);
                        }
                    }
                    Err(message) => {
                        let _ = error.try_set(message);
                    }
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
        #[cfg(target_arch = "wasm32")]
        if let Ok(interval) = leptos::prelude::set_interval_with_handle(
            move || {
                if editor.get_untracked().is_none() && dragged.get_untracked().is_none() {
                    submit.run(Action::Refresh);
                }
            },
            std::time::Duration::from_millis(1500),
        ) {
            on_cleanup(move || interval.clear());
        }
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
                .map(|card| card.depends_on.clone())
                .unwrap_or_default(),
        );
        new_column.set(Column::Triage);
        editor.set(Some(card.map(|card| card.id)));
    });
    let save = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        #[cfg(target_arch = "wasm32")]
        let depends_on = selected_dependencies(&event);
        #[cfg(not(target_arch = "wasm32"))]
        let depends_on = blockers.get_untracked();
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
                None => {
                    values["column"] = json!(new_column.get_untracked());
                    Action::Create(values)
                }
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
                    <Show when=move || editor.get() == Some(None)>
                        <label>"Column"
                            <select
                                prop:value=move || new_column.get().as_str()
                                on:change=move |event| {
                                    let value = event_target_value(&event);
                                    if let Some(column) = Column::ALL.into_iter().find(|column| column.as_str() == value) {
                                        new_column.set(column);
                                    }
                                }
                            >
                                {Column::ALL.into_iter().map(|column| view! {
                                    <option value=column.as_str()>{column_label(column)}</option>
                                }).collect_view()}
                            </select>
                        </label>
                    </Show>
                    <label>"Depends on"
                        <select name="depends_on" multiple
                            disabled=move || editor.get().flatten().is_some_and(|id| {
                                cards.with(|cards| cards.iter().any(|card| card.id == id && card.column == Column::Running))
                            })
                        >
                            {move || {
                                let editing = editor.get().flatten();
                                cards.get().into_iter().filter(|card| Some(&card.id) != editing.as_ref()).map(|card| {
                                    let selected = blockers.with(|ids| ids.contains(&card.id));
                                    view! { <option value=card.id prop:selected=selected>{card.title}</option> }
                                }).collect_view()
                            }}
                        </select>
                    </label>
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
                        <section class="kanban-lane" data-kanban-lane=column.as_str()
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

#[cfg(target_arch = "wasm32")]
fn selected_dependencies(event: &leptos::ev::SubmitEvent) -> Vec<String> {
    use leptos::web_sys::{FormData, HtmlFormElement};
    use wasm_bindgen::JsCast;
    event
        .current_target()
        .and_then(|target| target.dyn_into::<HtmlFormElement>().ok())
        .and_then(|form| FormData::new_with_form(&form).ok())
        .map(|form| {
            form.get_all("depends_on")
                .iter()
                .filter_map(|value| value.as_string())
                .collect()
        })
        .unwrap_or_default()
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
