//! What the webview is allowed to ask for.

use tauri::{Runtime, State, Window};

use crate::{measure, reposition, Placement, Shared};

/// Resizes the window to a named state and re-parks it.
///
/// The webview owns which state is current, since it also decides what to draw;
/// this is how it tells the plugin, which needs to know to work out what size a
/// slot should give the window. Calling it on startup lets the window recover
/// after a reload in development. An unknown name is an error, so a typo
/// surfaces in the webview's `catch` instead of silently doing nothing.
#[tauri::command]
pub(crate) fn set_state<R: Runtime>(
  window: Window<R>,
  shared: State<'_, Shared>,
  state: String,
) -> Result<(), String> {
  let config = shared.config().clone();
  let tracked = shared.tracked(window.label());

  // Checked here, on the way in, so an unconfigured name still comes back to
  // the webview as a rejected promise rather than disappearing into a thread.
  if !config.states.contains_key(&state) {
    return Err(format!(
      "unknown window state '{state}'; configured states: {}",
      config.state_names()
    ));
  }

  // Recorded before the thread starts, so a second call landing while the first
  // is still working sees the newer state rather than racing it.
  tracked.set_state(&state);

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
    // No anchor override: the state's own `anchor` decides, and failing that
    // the window stays in whichever slot the user left it in.
    if let Err(error) = reposition(&window, &config, &tracked, None, &mut None) {
      log::error!("corner-snap: set_state({state:?}) failed: {error}");
    }
  });

  Ok(())
}

/// Re-parks the window against its nearest slot, without waiting for the
/// periodic check. Useful after the webview changes something that moves it.
///
/// Threaded for the same reason as [`set_state`], which is also why the
/// placement comes back as an event rather than a return value.
#[tauri::command]
pub(crate) fn snap<R: Runtime>(
  window: Window<R>,
  shared: State<'_, Shared>,
) -> Result<(), String> {
  let config = shared.config().clone();
  let tracked = shared.tracked(window.label());

  std::thread::spawn(move || {
    if let Err(error) = reposition(&window, &config, &tracked, None, &mut None) {
      log::error!("corner-snap: snap failed: {error}");
    }
  });

  Ok(())
}

/// Where the window is right now: the slot it occupies and which way round it
/// is.
///
/// The same thing is emitted as `corner-snap://anchor` whenever it changes, but
/// the first of those goes out while the window is being created, long before
/// the page can listen for it. So a webview that cares asks once on startup and
/// listens from then on.
///
/// Measured rather than remembered, falling back to the last placement reported
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
) -> Option<Placement> {
  let tracked = shared.tracked(window.label());

  match measure(&window, shared.config(), &tracked) {
    // Noted rather than reported: an event repeating the value being returned
    // to the caller is not news. It keeps the next real change honest, though,
    // so a window that was moved by something else does not go unannounced.
    Some(placement) => {
      tracked.reporter().note(placement);
      Some(placement)
    }
    None => tracked.reporter().last(),
  }
}
