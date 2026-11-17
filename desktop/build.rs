fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "profiles",
            "add_profile",
            "select_profile",
            "save_credentials",
            "sign_in",
            "open_dashboard",
        ]),
    ))
    .expect("desktop application manifest");
}
