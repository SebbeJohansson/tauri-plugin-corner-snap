/// Commands the plugin exposes to the webview. Each one gets an
/// `allow-*` / `deny-*` permission generated for it under `permissions/`.
const COMMANDS: &[&str] = &["set_state", "snap", "current_anchor"];

fn main() {
  tauri_plugin::Builder::new(COMMANDS).build();
}
