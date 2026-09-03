/// Commands the plugin exposes to the webview. Each one gets an
/// `allow-*` / `deny-*` permission generated for it under `permissions/`.
const COMMANDS: &[&str] = &["set_collapsed", "snap"];

fn main() {
  tauri_plugin::Builder::new(COMMANDS).build();
}
