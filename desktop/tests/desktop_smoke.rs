use claude_web_ui_desktop::{GatewayProfiles, Notice, TrayState};

#[test]
fn switching_profiles_persists_the_selected_gateway_and_its_secure_endpoints() {
    let dir = std::env::temp_dir().join(format!("desktop-profiles-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("profiles.json");
    let mut profiles = GatewayProfiles::default();
    let home = profiles
        .add("Home", "https://gateway.example:8443/")
        .unwrap();
    let work = profiles.add("Work", "wss://work.example").unwrap();
    profiles.select(&home).unwrap();
    profiles.save(&path).unwrap();

    let mut restored = GatewayProfiles::load(&path).unwrap();
    assert_eq!(
        restored.active().unwrap().origin.as_str(),
        "https://gateway.example:8443/"
    );
    restored.select(&work).unwrap();
    assert_eq!(
        restored.active().unwrap().project_stream("abc").as_str(),
        "wss://work.example/api/projects/abc/chat/ws"
    );
    assert!(
        restored
            .add("Invalid", "https://user:pass@gateway.example")
            .is_err()
    );
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn a_gateway_websocket_drives_the_tray_and_emits_one_notification_per_approval() {
    let mut tray = TrayState::default();
    tray.connect("project");
    assert_eq!(tray.label(), "Connected · Idle");
    let request = r#"{"seq":1,"project_id":"project","session_id":"session","turn_id":"turn","status":"waiting_approval","event":{"type":"permission_request","request_id":"r","tool_name":"Read","input":{}},"timestamp":"2026-01-01T00:00:00Z"}"#;
    assert_eq!(
        tray.on_frame("project", request),
        Some(Notice::Approval {
            tool: "Read".into()
        })
    );
    assert_eq!(tray.on_frame("project", request), None);
    assert_eq!(tray.label(), "Connected · Waiting for approval");
    assert_eq!(tray.stop_target().unwrap().session_id, "session");
    let cancelled_queued = r#"{"seq":2,"project_id":"project","session_id":"session","turn_id":"queued","status":"cancelled","event":{"type":"turn_error","message":"Cancelled"},"timestamp":"2026-01-01T00:00:01Z"}"#;
    assert_eq!(tray.on_frame("project", cancelled_queued), None);
    assert_eq!(tray.stop_target().unwrap().turn_id, "turn");
    let complete = r#"{"seq":3,"project_id":"project","session_id":"session","turn_id":"turn","status":"completed","event":{"type":"turn_complete","session_id":null,"result_text":"done"},"timestamp":"2026-01-01T00:00:01Z"}"#;
    assert_eq!(
        tray.on_frame("project", complete),
        Some(Notice::TurnCompleted)
    );
    assert_eq!(tray.label(), "Connected · Idle");
    assert!(tray.stop_target().is_none());
}

fn asset(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn the_profiles_window_uses_the_hermes_teal_tokens_and_only_its_own_files() {
    let css = asset("styles.css");
    let html = asset("index.html");
    let js = asset("profiles.js");
    // The token block is a copy of the dashboard's (ui/dist/styles.css sections 2-3).
    assert!(css.contains("--background-base: #041c1c;"));
    assert!(css.contains("--midground-base: #ffe6cb;"));
    assert!(css.contains("ui/dist/styles.css"));
    // style-src 'self': no inline styles; no CDN or other external URL.
    assert!(!html.contains("<style") && !html.to_lowercase().contains(" style="));
    assert!(!js.contains("setAttribute(\"style\"") && !js.contains(".style.cssText"));
    assert!(!css.contains("data:"), "data: URIs would be blocked by the CSP");
    // (index.html legitimately shows "https://gateway.example" as an input placeholder.)
    let html = html.replace("placeholder=\"https://gateway.example\"", "");
    for (name, text) in [("index.html", &html), ("styles.css", &css), ("profiles.js", &js)] {
        assert!(
            !text.contains("http://") && !text.contains("https://"),
            "{name} references an external URL"
        );
    }
}

#[test]
fn every_bundled_font_is_used_by_the_stylesheet_and_ships_with_its_licence() {
    let css = asset("styles.css");
    let fonts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts");
    let mut referenced = 0;
    for part in css.split("url(\"").skip(1) {
        let url = part.split('"').next().unwrap();
        assert!(url.starts_with("./fonts/"), "{url} must be a local font");
        let file = fonts.join(url.trim_start_matches("./fonts/"));
        assert!(file.is_file(), "missing {}", file.display());
        assert!(
            file.parent().unwrap().join("OFL.txt").is_file(),
            "no OFL.txt beside {}",
            file.display()
        );
        referenced += 1;
    }
    assert!(css.contains("Unbounded-Cyrillic"), "Unbounded Cyrillic subset must be bundled");
    assert!(css.contains("CourierPrime"));
    // Nothing bundled that the stylesheet does not use.
    let mut on_disk = 0;
    for family in std::fs::read_dir(&fonts).unwrap() {
        for f in std::fs::read_dir(family.unwrap().path()).unwrap() {
            let f = f.unwrap().file_name().to_string_lossy().into_owned();
            if f.ends_with(".woff2") {
                on_disk += 1;
                assert!(css.contains(&f), "{f} is bundled but unused");
            }
        }
    }
    assert_eq!(on_disk, referenced);
}
