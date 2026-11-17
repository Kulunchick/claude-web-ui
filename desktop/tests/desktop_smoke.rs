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
