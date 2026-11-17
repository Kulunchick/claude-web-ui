use crate::auth_view::{
    AdminUsersProjectsView, BootstrapAdminView, InviteRedemptionView, LoginView, ProjectList,
};
use claude_web_ui_shared::auth::{
    ApiErrorResponse, AuthTokensResponse, BootstrapAdminRequest, InviteDto, LoginRequest,
    ProjectDto, RedeemInviteRequest, UserDto,
};
use gloo_net::http::{Request, RequestBuilder};
use leptos::mount::mount_to;
use leptos::prelude::*;
use leptos::web_sys::{CustomEvent, CustomEventInit, HtmlElement};
use std::cell::RefCell;
use wasm_bindgen::prelude::*;

thread_local! {
    // The dashboard owns the host element and replaces it when changing views.
    static MOUNTS: RefCell<Vec<Box<dyn std::any::Any>>> = RefCell::new(Vec::new());
}

#[wasm_bindgen]
pub fn unmount_gateway_view() {
    MOUNTS.with(|mounts| mounts.borrow_mut().clear());
}

#[wasm_bindgen]
pub fn mount_auth(parent: HtmlElement, screen: String, error: Option<String>, invite_code: String) {
    let handle = mount_to(parent, move || {
        view! {
            <main class="auth-shell">
                {match screen.as_str() {
                    "bootstrap" => view! { <BootstrapAdminView initial_error=error /> }.into_any(),
                    "invite" => view! {
                        <InviteRedemptionView
                            initial_error=error
                            prefilled_code=invite_code
                            on_switch_login=Callback::new(|_| switch_auth_screen("login"))
                        />
                    }.into_any(),
                    _ => view! {
                        <LoginView
                            initial_error=error
                            on_switch_invite=Callback::new(|_| switch_auth_screen("invite"))
                        />
                    }.into_any(),
                }}
            </main>
        }
    });
    MOUNTS.with(|mounts| mounts.borrow_mut().push(Box::new(handle)));
}

fn switch_auth_screen(screen: &'static str) {
    emit("cwu:auth-screen", screen);
}

fn emit(name: &str, detail: &str) {
    let init = CustomEventInit::new();
    init.set_detail(&JsValue::from_str(detail));
    if let Some(window) = leptos::web_sys::window()
        && let Ok(event) = CustomEvent::new_with_event_init_dict(name, &init)
    {
        let _ = window.dispatch_event(&event);
    }
}

fn auth_request(screen: &str, fields: &str) -> Result<gloo_net::http::Request, String> {
    fn body<T: serde::de::DeserializeOwned + serde::Serialize>(
        request: RequestBuilder,
        fields: &str,
    ) -> Result<Request, String> {
        let dto: T = serde_json::from_str(fields).map_err(|err| err.to_string())?;
        request.json(&dto).map_err(|err| err.to_string())
    }

    match screen {
        "bootstrap" => body::<BootstrapAdminRequest>(Request::post("/api/auth/bootstrap"), fields),
        "invite" => body::<RedeemInviteRequest>(Request::post("/api/auth/invites/redeem"), fields),
        "login" => body::<LoginRequest>(Request::post("/api/auth/login"), fields),
        _ => Err("Unknown sign-in form.".to_string()),
    }
}

#[wasm_bindgen]
pub async fn submit_auth(screen: String, fields: String) -> Result<String, JsValue> {
    let request = auth_request(&screen, &fields).map_err(JsValue::from)?;
    let response = request
        .send()
        .await
        .map_err(|err| JsValue::from(err.to_string()))?;

    if !response.ok() {
        let message = match response.json::<ApiErrorResponse>().await {
            Ok(error) => error.error,
            Err(_) => format!("HTTP {}", response.status()),
        };
        return Err(JsValue::from(message));
    }

    let auth = response
        .json::<AuthTokensResponse>()
        .await
        .map_err(|err| JsValue::from(err.to_string()))?;
    serde_json::to_string(&auth).map_err(|err| JsValue::from(err.to_string()))
}

#[wasm_bindgen]
pub fn mount_project_list(
    parent: HtmlElement,
    projects: &str,
    selected: Option<String>,
) -> Result<(), JsValue> {
    let projects: Vec<ProjectDto> =
        serde_json::from_str(projects).map_err(|err| JsValue::from(err.to_string()))?;
    let handle = mount_to(parent, move || {
        let (projects, _) = signal(projects);
        let (active_project_id, _) = signal(selected);
        view! {
            <ProjectList
                projects=projects
                active_project_id=active_project_id
                on_select=Callback::new(|id: String| emit("cwu:select-project", &id))
            />
        }
    });
    MOUNTS.with(|mounts| mounts.borrow_mut().push(Box::new(handle)));
    Ok(())
}

#[wasm_bindgen]
pub fn mount_cron(parent: HtmlElement, project_id: String, access_token: String) {
    let handle = mount_to(parent, move || {
        view! {
            <crate::cron_view::CronView
                project_id=project_id
                access_token=access_token
                open_session=Callback::new(|id: String| emit("cwu:open-cron-session", &id))
            />
        }
    });
    MOUNTS.with(|mounts| mounts.borrow_mut().push(Box::new(handle)));
}

#[wasm_bindgen]
pub fn mount_kanban(
    parent: HtmlElement,
    project_id: String,
    access_token: String,
    read_only: bool,
) {
    let handle = mount_to(parent, move || {
        view! {
            <crate::kanban_view::KanbanView
                project_id=project_id
                access_token=access_token
                read_only=read_only
                open_session=Callback::new(|id: String| emit("cwu:open-kanban-session", &id))
            />
        }
    });
    MOUNTS.with(|mounts| mounts.borrow_mut().push(Box::new(handle)));
}

#[wasm_bindgen]
pub fn mount_admin(
    parent: HtmlElement,
    users: &str,
    projects: &str,
    invites: &str,
) -> Result<(), JsValue> {
    fn decode<T: serde::de::DeserializeOwned>(input: &str) -> Result<T, JsValue> {
        serde_json::from_str(input).map_err(|err| JsValue::from(err.to_string()))
    }

    let users: Vec<UserDto> = decode(users)?;
    let projects: Vec<ProjectDto> = decode(projects)?;
    let invites: Vec<InviteDto> = decode(invites)?;
    let handle = mount_to(parent, move || {
        let (users, _) = signal(users);
        let (projects, _) = signal(projects);
        view! { <AdminUsersProjectsView users=users projects=projects invites=invites /> }
    });
    MOUNTS.with(|mounts| mounts.borrow_mut().push(Box::new(handle)));
    Ok(())
}
