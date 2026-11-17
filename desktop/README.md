# Desktop Gateway client

Run the Windows desktop client from the workspace with:

```powershell
cargo run -p claude-web-ui-desktop
```

The client requires WebView2 on Windows. Linux builds require the Tauri GTK,
WebKitGTK and app indicator dependencies plus a desktop Secret Service. Native
notifications on Windows use the installed application identity.

Add an HTTPS or WSS origin in **Gateway Profiles**, then sign in with your
Gateway account or paste an access JWT and optional refresh JWT. The app stores
the selected profile and origin in its configuration directory and stores JWTs
in the operating system credential store. Passwords are sent only to the
selected Gateway's login endpoint. Choose a saved profile to switch Gateways.

**Open Dashboard** loads that Gateway's shared web dashboard. The desktop
monitor stays active in the tray when the dashboard closes. **Stop Active Run**
cancels the specific active turn visible to the signed-in operator. Chat and
approval notifications come from project WebSockets. The desktop checks Kanban
boards every 15 seconds and notifies when a snapshot shows a change to Review or
Blocked.
