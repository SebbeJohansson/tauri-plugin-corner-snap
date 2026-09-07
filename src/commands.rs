//! What the webview is allowed to ask for.

use tauri::{Runtime, State, Window};

use crate::geometry::{self, snap_to_nearest_anchor};
use crate::{apply_state, Anchor, Shared};

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
  let config = shared.config().clone();
  let reporter = shared.reporter(window.label());

  // Checked here, on the way in, so an unconfigured name still comes back to
  // the webview as a rejected promise rather than disappearing into a thread.
  if !config.states.contains_key(&state) {
    return Err(format!(
      "unknown window state '{state}'; configured states: {}",
      config.state_names()
    ));
  }

  // Moving the work to a thread is the point here, not an optimisation.
  //
  // A synchronous command runs on the main thread, inside the webview's IPC
  // callback, which Windows has already invoked from inside the event loop's
  // own dispatch. `SetWindowPos` sends `WM_SIZE`, `WM_NCCALCSIZE` and friends
  // *synchronously* back into the window procedure, so calling it from there
  // re-enters an event loop that is already mid-cycle. That is what tao reports
  // as "NewEvents emitted without explicit RedrawEventsCleared", and it hung
  // the process. Tellingly, a resize to the size the window already had came
  // back fine: no size change means no WM_SIZE, so nothing re-enters.
  //
  // Off the main thread the same calls take the other branch of Tauri's
  // `send_user_message`: posted to the event loop and run between callbacks,
  // which is where resizing a window is safe.
  std::thread::spawn(move || {
    if let Err(error) = apply_state(&window, &config, &state, None, &reporter) {
      log::error!("corner-snap: set_state({state:?}) failed: {error}");
    }
  });

  Ok(())
}

/// Re-parks the window against its nearest anchor, without waiting for the
/// periodic check. Useful after the webview changes something that moves it.
///
/// Threaded for the same reason as [`set_state`], which is also why the corner
/// comes back as an event rather than a return value.
#[tauri::command]
pub(crate) fn snap<R: Runtime>(
  window: Window<R>,
  shared: State<'_, Shared>,
) -> Result<(), String> {
  let edge_margin = shared.config().edge_margin;
  let reporter = shared.reporter(window.label());

  std::thread::spawn(move || {
    match snap_to_nearest_anchor(&window, edge_margin, &mut None) {
      Ok(Some(anchor)) => reporter.report(&window, anchor),
      Ok(None) => {}
      Err(error) => log::error!("corner-snap: snap failed: {error}"),
    }
  });

  Ok(())
}

/// Which corner the window is in right now.
///
/// The corner is also emitted as `corner-snap://anchor` whenever it changes,
/// but the first of those goes out while the window is being created, long
/// before the page can listen for it. So a webview that cares about the corner
/// asks once on startup and listens from then on.
///
/// Measured rather than remembered, falling back to the last corner reported
/// when there is no monitor to measure against. `null` means neither was
/// available: under WSLg no monitors are reported at all.
///
/// Unlike the other two commands this one reads rather than moves the window,
/// so it answers on the calling thread; none of the calls involved re-enter the
/// event loop the way `set_position` does.
#[tauri::command]
pub(crate) fn current_anchor<R: Runtime>(
  window: Window<R>,
  shared: State<'_, Shared>,
) -> Option<Anchor> {
  let reporter = shared.reporter(window.label());

  match geometry::current_anchor(&window) {
    // Noted rather than reported: an event repeating the value being returned
    // to the caller is not news. It keeps the next real change honest, though,
    // so a window that was moved by something else does not go unannounced.
    Some(anchor) => {
      reporter.note(anchor);
      Some(anchor)
    }
    None => reporter.last(),
  }
}
