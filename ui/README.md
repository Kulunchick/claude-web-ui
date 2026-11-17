# Shared Gateway UI

The Gateway serves the files in `ui/dist` from its Rust binary. The Tauri
dashboard window loads the selected Gateway's copy of the same frontend.

The Leptos CSR views in `ui/src/browser.rs` mount the sign-in, first administrator,
invite redemption, project selection, and Cron workspace. The dashboard uses the existing
JavaScript workspace views for chat, terminal, file and Git browsing, Kanban,
and administration.

When editing a Leptos view or updating wasm-bindgen, rebuild the browser assets
from the workspace root. Use the `wasm-bindgen-cli` version that matches the
`wasm-bindgen` version in `Cargo.lock` (currently `0.2.130`).

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked
cargo build --locked -p claude-web-ui-ui --release --target wasm32-unknown-unknown
wasm-bindgen --target web --no-typescript --out-dir ui/dist --out-name leptos_gateway target/wasm32-unknown-unknown/release/claude_web_ui_ui.wasm
```

Commit the updated browser assets with the Rust source changes. This lets the
Gateway build and deploy without a WASM toolchain on the deployment system.
