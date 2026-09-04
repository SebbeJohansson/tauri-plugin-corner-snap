//! What the webview is allowed to ask for.

use tauri::{Runtime, State, Window};

use crate::geometry::snap_to_nearest_anchor;
use crate::{apply_state, Shared};

/// Resizes the window to a named state, keeping it against an anchor.
///
/// The webview owns which state is current, since it also decides what to draw.
/// Calling this on startup lets the window recover after a reload in
/// development. An unknown name is an error, so a typo surfaces in the
/// webview's `catch` instead of silently doing nothing.
#[tauri::command]
pub(crate) fn set_state<R: Runtime>(
  window: Window<R>,
  shared: State<'_, Shared>,
  state: String,
) -> Result<(), String> {
  // Logged on the way in and out at info level, because "did the webview's
  // request reach Rust at all" is otherwise invisible: a rejected invoke
  // surfaces only in the webview console, not in the terminal.
  log::info!("corner-snap: set_state({state:?}) on {}", window.label());
  let result = apply_state(&window, shared.config(), &state, None);
  match &result {
    Ok(()) => log::info!("corner-snap: set_state({state:?}) done"),
    Err(error) => log::error!("corner-snap: set_state({state:?}) failed: {error}"),
  }
  result
}

/// Re-parks the window against its nearest anchor, without waiting for the
/// periodic check. Useful after the webview changes something that moves it.
#[tauri::command]
pub(crate) fn snap<R: Runtime>(
  window: Window<R>,
  shared: State<'_, Shared>,
) -> Result<(), String> {
  snap_to_nearest_anchor(&window, shared.config().edge_margin, &mut None)
    .map_err(|error| error.to_string())
}
