use crate::gateway;
use claude_web_ui_desktop::{GatewayProfile, GatewayProfiles, Notice, TrayState};
use claude_web_ui_shared::auth::{AuthTokensResponse, LoginRequest};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Mutex, time::Duration};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_notification::NotificationExt;

const SERVICE: &str = "com.claudewebui.desktop";
const DASHBOARD: &str = "dashboard";

#[derive(Clone, Deserialize, Serialize)]
pub struct Credentials {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

impl Credentials {
    fn entry(profile: &GatewayProfile) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, &profile.id).map_err(|error| error.to_string())
    }

    fn load(profile: &GatewayProfile) -> Result<Option<Self>, String> {
        match Self::entry(profile)?.get_password() {
            Ok(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|error| error.to_string()),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn save(&self, profile: &GatewayProfile) -> Result<(), String> {
        let data = serde_json::to_string(self).map_err(|error| error.to_string())?;
        Self::entry(profile)?
            .set_password(&data)
            .map_err(|error| error.to_string())
    }
}

#[derive(Default)]
struct DesktopSession {
    generation: u64,
    profiles: GatewayProfiles,
    credentials: Option<Credentials>,
    tray: TrayState,
    can_stop: bool,
    monitor: Option<tauri::async_runtime::JoinHandle<()>>,
}

struct Desktop {
    path: PathBuf,
    session: Mutex<DesktopSession>,
    client: reqwest::Client,
    status: MenuItem<tauri::Wry>,
    stop: MenuItem<tauri::Wry>,
}

fn require_settings(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "profiles" {
        Ok(())
    } else {
        Err("Open Gateway Profiles to change desktop settings.".into())
    }
}

#[tauri::command]
fn profiles(window: WebviewWindow, desktop: State<'_, Desktop>) -> Result<GatewayProfiles, String> {
    require_settings(&window)?;
    Ok(desktop.session.lock().unwrap().profiles.clone())
}

#[tauri::command]
async fn add_profile(
    window: WebviewWindow,
    app: AppHandle,
    name: String,
    origin: String,
) -> Result<(), String> {
    require_settings(&window)?;
    activate(&app, |profiles| {
        profiles
            .add(&name, &origin)
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
}

#[tauri::command]
async fn select_profile(window: WebviewWindow, app: AppHandle, id: String) -> Result<(), String> {
    require_settings(&window)?;
    activate(&app, |profiles| {
        profiles.select(&id).map_err(|error| error.to_string())
    })
}

#[tauri::command]
async fn sign_in(
    window: WebviewWindow,
    app: AppHandle,
    id: String,
    identifier: String,
    password: String,
) -> Result<(), String> {
    require_settings(&window)?;
    let desktop = app.state::<Desktop>();
    let profile = desktop
        .session
        .lock()
        .unwrap()
        .profiles
        .profiles
        .iter()
        .find(|profile| profile.id == id)
        .cloned()
        .ok_or("The Gateway profile does not exist.")?;
    let response = desktop
        .client
        .post(profile.endpoint(&["api", "auth", "login"]))
        .json(&LoginRequest {
            identifier,
            password,
        })
        .send()
        .await
        .map_err(|_| "Could not reach the Gateway.")?;
    if !response.status().is_success() {
        return Err(format!(
            "Gateway sign-in failed (HTTP {}).",
            response.status().as_u16()
        ));
    }
    let tokens = response
        .json::<AuthTokensResponse>()
        .await
        .map_err(|_| "The Gateway returned an invalid sign-in response.")?;
    activate(&app, |profiles| {
        profiles.select(&id).map_err(|error| error.to_string())?;
        Credentials {
            access_token: tokens.access_token,
            refresh_token: Some(tokens.refresh_token),
        }
        .save(&profile)
    })
}

#[tauri::command]
async fn save_credentials(
    window: WebviewWindow,
    app: AppHandle,
    id: String,
    access_token: String,
    refresh_token: String,
) -> Result<(), String> {
    require_settings(&window)?;
    let access_token = access_token.trim();
    let refresh_token = refresh_token.trim();
    if access_token.split('.').count() != 3 {
        return Err("Enter a Gateway access JWT.".into());
    }
    let credentials = Credentials {
        access_token: access_token.into(),
        refresh_token: (!refresh_token.is_empty()).then(|| refresh_token.to_owned()),
    };
    activate(&app, |profiles| {
        profiles.select(&id).map_err(|error| error.to_string())?;
        credentials.save(profiles.active().unwrap())
    })
}

fn activate(
    app: &AppHandle,
    update: impl FnOnce(&mut GatewayProfiles) -> Result<(), String>,
) -> Result<(), String> {
    let desktop = app.state::<Desktop>();
    let mut session = desktop.session.lock().unwrap();
    let mut profiles = session.profiles.clone();
    update(&mut profiles)?;
    profiles
        .save(&desktop.path)
        .map_err(|error| error.to_string())?;
    session.profiles = profiles;
    let result = start_monitor(app, &mut session);
    drop(session);
    if let Some(window) = app.get_webview_window(DASHBOARD) {
        let _ = window.destroy();
    }
    result
}

fn start_monitor(app: &AppHandle, session: &mut DesktopSession) -> Result<(), String> {
    if let Some(monitor) = session.monitor.take() {
        monitor.abort();
    }
    session.generation += 1;
    session.tray = TrayState::default();
    session.can_stop = false;
    session.credentials = None;
    sync_tray(app, session);
    let Some(profile) = session.profiles.active().cloned() else {
        return Ok(());
    };
    let credentials = Credentials::load(&profile)?;
    session.credentials = credentials.clone();
    let app = app.clone();
    let generation = session.generation;
    let client = app.state::<Desktop>().client.clone();
    session.monitor = Some(tauri::async_runtime::spawn(gateway::monitor(
        app,
        generation,
        client,
        profile,
        credentials,
    )));
    Ok(())
}

fn sync_tray(app: &AppHandle, session: &DesktopSession) {
    let desktop = app.state::<Desktop>();
    let _ = desktop.status.set_text(session.tray.label());
    let _ = desktop
        .stop
        .set_enabled(session.can_stop && session.tray.stop_target().is_some());
    if let Some(tray) = app.tray_by_id("gateway") {
        let _ = tray.set_tooltip(Some(session.tray.label()));
    }
}

pub fn update_tray(
    app: &AppHandle,
    generation: u64,
    update: impl FnOnce(&mut TrayState, &mut bool) -> Option<Notice>,
) {
    let desktop = app.state::<Desktop>();
    let mut session = desktop.session.lock().unwrap();
    if session.generation != generation {
        return;
    }
    let DesktopSession { tray, can_stop, .. } = &mut *session;
    let notice = update(tray, can_stop);
    sync_tray(app, &session);
    if let Some(notice) = notice {
        let (title, body) = match notice {
            Notice::Approval { tool } => (
                "Approval requested",
                format!("{tool} is waiting for Allow or Deny."),
            ),
            Notice::TurnCompleted => (
                "CLI turn complete",
                "Open the Gateway dashboard to see the result.".into(),
            ),
            Notice::CardReview { title } => ("Kanban card ready for review", title),
            Notice::CardBlocked { title } => ("Kanban card blocked", title),
        };
        let _ = app.notification().builder().title(title).body(body).show();
    }
}

pub fn replace_credentials(
    app: &AppHandle,
    generation: u64,
    profile: &GatewayProfile,
    credentials: Credentials,
) -> Result<(), String> {
    let desktop = app.state::<Desktop>();
    let mut session = desktop.session.lock().unwrap();
    if session.generation != generation {
        return Err("Gateway profile changed.".into());
    }
    session.credentials = Some(credentials.clone());
    let result = credentials.save(profile);
    if let Some(window) = app.get_webview_window(DASHBOARD) {
        let origin = serde_json::to_string(&profile.origin.origin().ascii_serialization()).unwrap();
        let token = serde_json::to_string(&credentials.access_token).unwrap();
        let _ = window.eval(format!("if (location.origin === {origin}) {{ window.__CWU_DESKTOP_ACCESS_TOKEN = {token}; window.dispatchEvent(new Event('cwu:desktop-token')); }}"));
    }
    result
}

#[tauri::command]
async fn open_dashboard(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    require_settings(&window)?;
    show_dashboard(&app)
}

fn show_dashboard(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(DASHBOARD) {
        return focus(&window);
    }
    let desktop = app.state::<Desktop>();
    let (profile, token) = {
        let session = desktop.session.lock().unwrap();
        let Some(profile) = session.profiles.active().cloned() else {
            return show_profiles(app);
        };
        (
            profile,
            session
                .credentials
                .as_ref()
                .map(|creds| creds.access_token.clone()),
        )
    };
    let allowed_origin = profile.origin.origin();
    let origin = serde_json::to_string(&allowed_origin.ascii_serialization()).unwrap();
    let script = format!(
        "if (window.top === window && location.origin === {origin}) {{ window.__CWU_DESKTOP_ACCESS_TOKEN = {}; }}",
        serde_json::to_string(&token).unwrap()
    );
    WebviewWindowBuilder::new(app, DASHBOARD, WebviewUrl::External(profile.origin))
        .title(format!("Claude Web UI - {}", profile.name))
        .inner_size(1200.0, 800.0)
        .incognito(true)
        .on_navigation(move |url| url.origin() == allowed_origin)
        .initialization_script(script)
        .build()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn focus(window: &WebviewWindow) -> Result<(), String> {
    window
        .show()
        .and_then(|_| window.unminimize())
        .and_then(|_| window.set_focus())
        .map_err(|error| error.to_string())
}

fn show_profiles(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("profiles")
        .ok_or("Gateway Profiles window unavailable")?;
    focus(&window)
}

async fn stop_active(app: AppHandle) {
    let desktop = app.state::<Desktop>();
    let request = {
        let session = desktop.session.lock().unwrap();
        session
            .profiles
            .active()
            .zip(session.credentials.as_ref())
            .zip(session.tray.stop_target())
            .filter(|_| session.can_stop)
            .map(|((profile, creds), target)| {
                desktop
                    .client
                    .post(profile.endpoint(&[
                        "api",
                        "projects",
                        &target.project_id,
                        "turns",
                        &target.turn_id,
                        "cancel",
                    ]))
                    .bearer_auth(&creds.access_token)
            })
    };
    if let Some(request) = request
        && !request
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
    {
        let _ = app
            .notification()
            .builder()
            .title("Could not stop the run")
            .body("Check the active Gateway session.")
            .show();
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            profiles,
            add_profile,
            select_profile,
            save_credentials,
            sign_in,
            open_dashboard
        ])
        .setup(|app| {
            let path = app.path().app_config_dir()?.join("profiles.json");
            let loaded = GatewayProfiles::load(&path);
            let status = MenuItem::with_id(app, "status", "Disconnected", false, None::<&str>)?;
            let open = MenuItem::with_id(app, "open", "Open Dashboard", true, None::<&str>)?;
            let stop = MenuItem::with_id(app, "stop", "Stop Active Run", false, None::<&str>)?;
            let settings =
                MenuItem::with_id(app, "settings", "Gateway Profiles", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&status, &open, &stop, &settings, &quit])?;
            TrayIconBuilder::with_id("gateway")
                .icon(app.default_window_icon().expect("desktop icon").clone())
                .tooltip("Disconnected")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = show_dashboard(&app);
                        });
                    }
                    "settings" => {
                        let _ = show_profiles(app);
                    }
                    "stop" => {
                        tauri::async_runtime::spawn(stop_active(app.clone()));
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()?;
            let profiles = loaded.as_ref().cloned().unwrap_or_default();
            app.manage(Desktop {
                path,
                session: Mutex::new(DesktopSession {
                    profiles,
                    ..Default::default()
                }),
                client,
                status,
                stop,
            });
            let desktop = app.state::<Desktop>();
            let mut session = desktop.session.lock().unwrap();
            let startup_error = loaded
                .err()
                .map(|error| error.to_string())
                .or_else(|| start_monitor(app.handle(), &mut session).err());
            if let Some(error) = startup_error {
                let _ = app
                    .notification()
                    .builder()
                    .title("Gateway profiles unavailable")
                    .body(error)
                    .show();
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "profiles"
                && let tauri::WindowEvent::CloseRequested { api, .. } = event
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("run Claude Web UI desktop");
}
