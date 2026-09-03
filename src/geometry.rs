//! Working out where a window belongs on the screen it is on.

use tauri::{Monitor, PhysicalPosition, PhysicalSize, Runtime, Window};

/// A corner of the monitor's work area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
  TopLeft,
  TopRight,
  BottomLeft,
  BottomRight,
}

impl Corner {
  /// (towards the right edge, towards the bottom edge).
  pub fn flags(self) -> (bool, bool) {
    match self {
      Corner::TopLeft => (false, false),
      Corner::TopRight => (true, false),
      Corner::BottomLeft => (false, true),
      Corner::BottomRight => (true, true),
    }
  }

  pub fn from_flags(right: bool, bottom: bool) -> Self {
    match (right, bottom) {
      (false, false) => Corner::TopLeft,
      (true, false) => Corner::TopRight,
      (false, true) => Corner::BottomLeft,
      (true, true) => Corner::BottomRight,
    }
  }
}

/// Finds a monitor to work against, trying the most specific source first.
///
/// A hidden window often has no `current_monitor` yet, and under WSLg none of
/// these report anything at all, so every step can legitimately come back empty.
pub fn pick_monitor<R: Runtime>(window: &Window<R>) -> Option<Monitor> {
  if let Ok(Some(monitor)) = window.current_monitor() {
    return Some(monitor);
  }
  if let Ok(Some(monitor)) = window.primary_monitor() {
    return Some(monitor);
  }
  window
    .available_monitors()
    .ok()
    .and_then(|monitors| monitors.into_iter().next())
}

/// Position that puts a window of `size` in one corner of the monitor's work
/// area, which is the screen minus the taskbar.
pub fn corner_position(
  monitor: &Monitor,
  size: PhysicalSize<u32>,
  corner: Corner,
  edge_margin: i32,
) -> PhysicalPosition<i32> {
  let (right, bottom) = corner.flags();
  let area = monitor.work_area();
  let width = size.width as i32;
  let height = size.height as i32;

  let x = if right {
    area.position.x + area.size.width as i32 - width - edge_margin
  } else {
    area.position.x + edge_margin
  };
  let y = if bottom {
    area.position.y + area.size.height as i32 - height - edge_margin
  } else {
    area.position.y + edge_margin
  };

  PhysicalPosition::new(x, y)
}

/// Which corner of `monitor` a window of `size` currently sits in.
///
/// Centres are compared rather than edges, so the corner the window mostly
/// occupies wins even when it hangs off the side of the screen.
pub fn occupied_corner<R: Runtime>(
  window: &Window<R>,
  monitor: &Monitor,
  size: PhysicalSize<u32>,
) -> tauri::Result<Corner> {
  let area = monitor.work_area();
  let position = window.outer_position()?;

  let window_centre_x = position.x + size.width as i32 / 2;
  let window_centre_y = position.y + size.height as i32 / 2;
  let area_centre_x = area.position.x + area.size.width as i32 / 2;
  let area_centre_y = area.position.y + area.size.height as i32 / 2;

  Ok(Corner::from_flags(
    window_centre_x >= area_centre_x,
    window_centre_y >= area_centre_y,
  ))
}

/// Moves the window to whichever corner it now sits closest to.
pub fn snap_to_nearest_corner<R: Runtime>(
  window: &Window<R>,
  edge_margin: i32,
) -> tauri::Result<()> {
  let Some(monitor) = pick_monitor(window) else {
    // Logged quietly: the placement check runs on a timer, and under WSLg this
    // is the normal answer every time.
    log::debug!("corner-snap: no monitor reported; cannot snap");
    return Ok(());
  };

  let size = window.outer_size()?;
  let corner = occupied_corner(window, &monitor, size)?;
  let target = corner_position(&monitor, size, corner, edge_margin);
  let position = window.outer_position()?;

  // Setting the position raises another move event. Stopping here when the
  // window is already parked keeps that from looping.
  if position.x == target.x && position.y == target.y {
    return Ok(());
  }

  window.set_position(target)
}
