use claude_web_ui_shared::accounts::{CliProvider, ProjectCliStateDto};
use claude_web_ui_shared::chat::ChatSessionDto;
use claude_web_ui_shared::cron::{CronDestination, CronJobConfig, CronJobsResponse, CronRunDto};
use leptos::prelude::*;

#[derive(Clone)]
enum Action {
    Refresh,
    Save(Option<String>, CronJobConfig),
    Toggle(String, bool),
    Run(String),
    Delete(String),
}

#[derive(Clone)]
struct CronData {
    schedules: CronJobsResponse,
    runs: Vec<CronRunDto>,
    cli: Option<ProjectCliStateDto>,
    sessions: Vec<ChatSessionDto>,
}

#[component]
pub fn CronView(
    project_id: String,
    access_token: String,
    open_session: Callback<String>,
) -> impl IntoView {
    let data = RwSignal::new(None::<CronData>);
    let editor = RwSignal::new(None::<(Option<String>, CronJobConfig)>);
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let endpoint = StoredValue::new(format!("/api/projects/{project_id}"));
    let token = StoredValue::new(access_token);
    let submit = Callback::new(move |action: Action| {
        #[cfg(target_arch = "wasm32")]
        if !busy.get_untracked() {
            let endpoint = endpoint.get_value();
            let token = token.get_value();
            busy.set(true);
            error.set(String::new());
            leptos::task::spawn_local(async move {
                let close_editor = matches!(action, Action::Save(..));
                match request(&endpoint, &token, action).await {
                    Ok(result) => {
                        let _ = data.try_set(Some(result));
                        if close_editor {
                            let _ = editor.try_set(None);
                        }
                    }
                    Err(message) => {
                        let _ = error.try_set(message);
                    }
                }
                let _ = busy.try_set(false);
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (endpoint, token, &action);
            match action {
                Action::Save(id, config) => {
                    let _ = (id, config);
                }
                Action::Toggle(id, enabled) => {
                    let _ = (id, enabled);
                }
                Action::Run(id) | Action::Delete(id) => {
                    let _ = id;
                }
                Action::Refresh => {}
            }
        }
    });
    Effect::new(move |_| submit.run(Action::Refresh));
    let writable = move || data.with(|value| value.as_ref().is_some_and(|e| e.schedules.writable));
    let new_schedule = move |_| {
        editor.set(Some((
            None,
            CronJobConfig {
                expression: String::new(),
                prompt: String::new(),
                provider: None,
                account_id: None,
                pinned_session_id: None,
                notification: None,
                enabled: true,
            },
        )))
    };

    view! {
        <section class="panel-card cron-panel">
            <div class="pane-head">
                <h2>"Cron schedules"</h2>
                <button type="button" class="btn-secondary" disabled=move || busy.get()
                    on:click=move |_| submit.run(Action::Refresh)>"Refresh"</button>
                <Show when=writable>
                    <button type="button" class="btn-primary" disabled=move || busy.get()
                        on:click=new_schedule>"New schedule"</button>
                </Show>
            </div>
            <Show when=move || busy.get() && data.get().is_none()>
                <p role="status">"Loading schedules…"</p>
            </Show>
            <Show when=move || !error.get().is_empty()>
                <p role="alert">{move || error.get()}</p>
            </Show>
            <Show when=move || writable() && editor.with(Option::is_some)>
                <CronEditor editor=editor data=data busy=busy submit=submit />
            </Show>
            {move || data.get().map(|data| {
                if data.schedules.jobs.is_empty() {
                    return view! { <p>"No schedules yet."</p> }.into_any();
                }
                let can_write = data.schedules.writable;
                data.schedules.jobs.into_iter().map(|job| {
                    let edit_job = job.clone();
                    let toggle_job = job.clone();
                    let run_id = job.id.clone();
                    let delete_id = job.id.clone();
                    let confirm_delete = RwSignal::new(false);
                    let toggle_label = if job.config.enabled { "Pause" } else { "Resume" };
                    view! {
                        <article class="kanban-card">
                            <h3>{job.config.prompt}</h3>
                            <p><code>{job.config.expression}</code>" UTC · "
                                {job.next_run_at.map(|at| format!("Next run {at}")).unwrap_or_else(|| "Paused".into())}
                            </p>
                            {can_write.then(|| view! { <div>
                                <button type="button" class="btn-secondary" disabled=move || busy.get()
                                    on:click=move |_| editor.set(Some((edit_job.id.clone().into(), edit_job.config.clone())))>"Edit"</button>
                                <button type="button" class="btn-secondary" disabled=move || busy.get()
                                    on:click=move |_| {
                                        submit.run(Action::Toggle(toggle_job.id.clone(), !toggle_job.config.enabled));
                                    }>{toggle_label}</button>
                                <button type="button" class="btn-secondary" disabled=move || busy.get()
                                    on:click=move |_| submit.run(Action::Run(run_id.clone()))>"Run now"</button>
                                <button type="button" class="btn-secondary" disabled=move || busy.get()
                                    on:click=move |_| {
                                        if confirm_delete.get() { submit.run(Action::Delete(delete_id.clone())); }
                                        else { confirm_delete.set(true); }
                                    }>{move || if confirm_delete.get() { "Confirm delete" } else { "Delete" }}</button>
                                <Show when=move || confirm_delete.get()>
                                    <button type="button" class="btn-secondary"
                                        on:click=move |_| confirm_delete.set(false)>"Cancel delete"</button>
                                </Show>
                            </div> })}
                        </article>
                    }
                }).collect_view().into_any()
            })}
        </section>
        <section class="panel-card cron-panel">
            <h2>"Run history"</h2>
            {move || data.get().map(|data| {
                if data.runs.is_empty() {
                    return view! { <p>"No runs yet."</p> }.into_any();
                }
                data.runs.into_iter().map(|run| {
                    let label = run.prompt;
                    view! {
                        <article class="kanban-card">
                            <h3>{label}</h3>
                            <p>{format!("{} · {}", run.status.as_str().replace('_', " "),
                                match run.trigger {
                                    claude_web_ui_shared::cron::CronTrigger::Manual => "Manual",
                                    claude_web_ui_shared::cron::CronTrigger::Scheduled => "Scheduled",
                                })}</p>
                            <p>{format!("Queued {}", run.queued_at)}</p>
                            {run.started_at.map(|at| view! { <p>{format!("Started {at}")}</p> })}
                            {run.completed_at.map(|at| view! { <p>{format!("Completed {at}")}</p> })}
                            {run.summary.map(|value| view! { <p class="cron-run-summary">{value}</p> })}
                            {run.error.map(|value| view! { <p>{value}</p> })}
                            {run.notification_error.map(|value| view! { <p>{format!("Notification failed: {value}")}</p> })}
                            {run.session_id.map(|id| view! {
                                <button type="button" class="btn-secondary"
                                    on:click=move |_| open_session.run(id.clone())>"Open session"</button>
                            })}
                        </article>
                    }
                }).collect_view().into_any()
            })}
        </section>
    }
}

fn option_value(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

#[component]
fn CronEditor(
    editor: RwSignal<Option<(Option<String>, CronJobConfig)>>,
    data: RwSignal<Option<CronData>>,
    busy: RwSignal<bool>,
    submit: Callback<Action>,
) -> impl IntoView {
    let config = move || editor.get().map(|(_, config)| config);
    let change = move |edit: &dyn Fn(&mut CronJobConfig)| {
        editor.update(|value| {
            if let Some((_, config)) = value {
                edit(config);
            }
        })
    };
    let destinations = move || {
        data.with(|data| {
            data.as_ref()
                .map(|data| data.schedules.destinations.clone())
                .unwrap_or_default()
        })
    };
    let destination_key = |destination: &CronDestination| {
        format!(
            "{}/{}/{}",
            destination.platform.as_str(),
            destination.chat_id,
            destination.thread_id.as_deref().unwrap_or_default()
        )
    };

    view! {
        <form class="kanban-form" on:submit=move |event: leptos::ev::SubmitEvent| {
            event.prevent_default();
            if let Some((id, config)) = editor.get_untracked() { submit.run(Action::Save(id, config)); }
        }>
            <label>"Cron expression (UTC)"
                <input required placeholder="0 9 * * MON" prop:value=move || config().map(|c| c.expression).unwrap_or_default()
                    on:input=move |ev| change(&|c| c.expression = event_target_value(&ev)) />
            </label>
            <label>"Prompt"
                <textarea required rows=4 prop:value=move || config().map(|c| c.prompt).unwrap_or_default()
                    on:input=move |ev| change(&|c| c.prompt = event_target_value(&ev)) />
            </label>
            <label>"CLI provider"
                <select prop:value=move || config().and_then(|c| c.provider).map(|p| p.as_str()).unwrap_or_default()
                    on:change=move |ev| change(&|c| {
                        c.provider = event_target_value(&ev).parse().ok();
                        c.account_id = None;
                    })>
                    <option value="">"Project or pinned session default"</option>
                    {CliProvider::ALL.into_iter().map(|provider| view! {
                        <option value=provider.as_str()>{provider.display_name()}</option>
                    }).collect_view()}
                </select>
            </label>
            <label>"Session"
                <select prop:value=move || config().and_then(|c| c.pinned_session_id).unwrap_or_default()
                    on:change=move |ev| change(&|c| {
                        c.pinned_session_id = option_value(event_target_value(&ev));
                        c.account_id = None;
                    })>
                    <option value="">"Start a fresh session each run"</option>
                    {move || data.get().map(|data| data.sessions.into_iter()
                        .filter(|s| config().and_then(|c| c.provider).is_none_or(|p| p == s.provider))
                        .map(|session| view! {
                            <option value=session.id>{format!("Resume {} · {}", session.title, session.provider.display_name())}</option>
                        }).collect_view())}
                </select>
            </label>
            <label>"CLI account"
                <select prop:value=move || config().and_then(|c| c.account_id).unwrap_or_default()
                    on:change=move |ev| change(&|c| c.account_id = option_value(event_target_value(&ev)))>
                    <option value="">"Project account for this provider"</option>
                    {move || {
                        let current = config()?;
                        let data = data.get()?;
                        let cli = data.cli?;
                        let provider = current.provider.or_else(|| data.sessions.iter()
                            .find(|s| Some(&s.id) == current.pinned_session_id.as_ref()).map(|s| s.provider))
                            .unwrap_or(cli.active_provider);
                        Some(cli.available_accounts.into_iter().filter(|account| account.provider == provider)
                            .map(|account| view! { <option value=account.id>{account.name}</option> }).collect_view())
                    }}
                </select>
            </label>
            <label>"Completion report"
                <select prop:value=move || config().and_then(|c| c.notification)
                    .as_ref().map(destination_key).unwrap_or_default()
                    on:change=move |ev| change(&|c| c.notification = destinations().into_iter()
                        .find(|destination| destination_key(destination) == event_target_value(&ev)))>
                    <option value="">"No messenger report"</option>
                    {move || destinations().into_iter().map(|dest| {
                        let details = dest.thread_id.as_ref().map(|thread| format!(" · Thread {thread}")).unwrap_or_default();
                        view! { <option value=destination_key(&dest)>
                            {format!("{} · {}{details}", dest.platform.as_str(), dest.chat_id)}
                        </option> }
                    }).collect_view()}
                </select>
            </label>
            <label>
                <input type="checkbox" prop:checked=move || config().is_some_and(|c| c.enabled)
                    on:change=move |ev| change(&|c| c.enabled = event_target_checked(&ev)) />
                "Enabled"
            </label>
            <div>
                <button type="submit" class="btn-primary" disabled=move || busy.get()>"Save schedule"</button>
                <button type="button" class="btn-secondary" on:click=move |_| editor.set(None)>"Cancel"</button>
            </div>
        </form>
    }
}

#[cfg(target_arch = "wasm32")]
async fn request(project: &str, token: &str, action: Action) -> Result<CronData, String> {
    use gloo_net::http::{Request, RequestBuilder, Response};

    async fn decode(response: Response) -> Result<Response, String> {
        if response.ok() {
            return Ok(response);
        }
        Err(response
            .json::<claude_web_ui_shared::auth::ApiErrorResponse>()
            .await
            .map(|error| error.error)
            .unwrap_or_else(|_| format!("HTTP {}", response.status())))
    }

    async fn get<T: serde::de::DeserializeOwned>(
        request: RequestBuilder,
        auth: &str,
    ) -> Result<T, String> {
        let response = request
            .header("Authorization", auth)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        decode(response)
            .await?
            .json()
            .await
            .map_err(|error| error.to_string())
    }

    let cron = format!("{project}/cron");
    let auth = format!("Bearer {token}");
    let mutation = match action {
        Action::Refresh => None,
        Action::Save(None, config) => Some(
            Request::post(&cron)
                .header("Authorization", &auth)
                .json(&config),
        ),
        Action::Save(Some(id), config) => Some(
            Request::put(&format!("{cron}/{id}"))
                .header("Authorization", &auth)
                .json(&config),
        ),
        Action::Toggle(id, enabled) => Some(
            Request::patch(&format!("{cron}/{id}"))
                .header("Authorization", &auth)
                .json(&claude_web_ui_shared::cron::SetCronEnabledRequest { enabled }),
        ),
        Action::Run(id) => Some(
            Request::post(&format!("{cron}/{id}/run"))
                .header("Authorization", &auth)
                .build(),
        ),
        Action::Delete(id) => Some(
            Request::delete(&format!("{cron}/{id}"))
                .header("Authorization", &auth)
                .build(),
        ),
    };
    if let Some(request) = mutation {
        decode(
            request
                .map_err(|error| error.to_string())?
                .send()
                .await
                .map_err(|error| error.to_string())?,
        )
        .await?;
    }
    let schedules: CronJobsResponse = get(Request::get(&cron), &auth).await?;
    let runs = get(Request::get(&format!("{cron}/runs")), &auth).await?;
    let (cli, sessions) = if schedules.writable {
        (
            Some(get(Request::get(&format!("{project}/cli")), &auth).await?),
            get(Request::get(&format!("{project}/sessions")), &auth).await?,
        )
    } else {
        (None, Vec::new())
    };
    Ok(CronData {
        schedules,
        runs,
        cli,
        sessions,
    })
}
