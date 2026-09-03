//! What the webview is allowed to ask for.

use tauri::{LogicalSize, PhysicalSize, Runtime, State, Window};

use crate::geometry::{corner_position, occupied_corner, pick_monitor, snap_to_nearest_corner};
use crate::Shared;

/// Resizes the window between its panel and badge sizes, keeping it in the
/// corner it currently occupies.
///
/// The webview owns this state, since it also decides what to draw. Calling it
/// on startup lets the window recover after a reload in development.
#[tauri::command]
pub(crate) fn set_collapsed<R: Runtime>(
  window: Window<R>,
  shared: State<'_, Shared>,
  collapsed: bool,
) -> Result<(), String> {
  let config = shared.config();
  let (width, height) = if collapsed {
    config.collapsed
  } else {
    config.expanded
  };
  let logical = LogicalSize::new(width, height);

  // Which corner the window belongs in is worked out before resizing. Measured
  // afterwards, a window that grew in a corner overhangs the screen edge, and
  // its centre can land on the neighbouring monitor, which would send it to
  // that screen instead.
  let Some(monitor) = pick_monitor(&window) else {
    log::warn!("corner-snap: no monitor reported; resizing without moving");
    return window.set_size(logical).map_err(|error| error.to_string());
  };

  let size_before = window.outer_size().map_err(|error| error.to_string())?;
  let corner =
    occupied_corner(&window, &monitor, size_before).map_err(|error| error.to_string())?;

  let scale = window.scale_factor().map_err(|error| error.to_string())?;
  let size_after: PhysicalSize<u32> = logical.to_physical(scale);

  // Moving before resizing means the window grows into place, rather than
  // briefly spilling past the edge of the screen.
  window
    .set_position(corner_position(
      &monitor,
      size_after,
      corner,
      config.edge_margin,
    ))
    .map_err(|error| error.to_string())?;
  window.set_size(logical).map_err(|error| error.to_string())
}

/// Re-parks the window in its nearest corner, without waiting for the periodic
/// check. Useful after the webview changes something that moves the window.
#[tauri::command]
pub(crate) fn snap<R: Runtime>(
  window: Window<R>,
  shared: State<'_, Shared>,
) -> Result<(), String> {
  snap_to_nearest_corner(&window, shared.config().edge_margin).map_err(|error| error.to_string())
}
