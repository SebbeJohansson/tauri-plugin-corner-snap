//! Working out where a window belongs on the screen it is on.

use tauri::{Monitor, PhysicalPosition, PhysicalSize, Runtime, Window};

/// A corner of the monitor's work area to hold a window against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Anchor {
  TopLeft,
  TopRight,
  BottomLeft,
  BottomRight,
}

impl Anchor {
  /// (towards the right edge, towards the bottom edge).
  pub fn flags(self) -> (bool, bool) {
    match self {
      Anchor::TopLeft => (false, false),
      Anchor::TopRight => (true, false),
      Anchor::BottomLeft => (false, true),
      Anchor::BottomRight => (true, true),
    }
  }

  pub fn from_flags(right: bool, bottom: bool) -> Self {
    match (right, bottom) {
      (false, false) => Anchor::TopLeft,
      (true, false) => Anchor::TopRight,
      (false, true) => Anchor::BottomLeft,
      (true, true) => Anchor::BottomRight,
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
pub fn anchor_position(
  monitor: &Monitor,
  size: PhysicalSize<u32>,
  corner: Anchor,
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

/// Which anchor of `monitor` a window of `size` currently sits at.
///
/// Centres are compared rather than edges, so the corner the window mostly
/// occupies wins even when it hangs off the side of the screen.
pub fn occupied_anchor<R: Runtime>(
  window: &Window<R>,
  monitor: &Monitor,
  size: PhysicalSize<u32>,
) -> tauri::Result<Anchor> {
  let area = monitor.work_area();
  let position = window.outer_position()?;

  let window_centre_x = position.x + size.width as i32 / 2;
  let window_centre_y = position.y + size.height as i32 / 2;
  let area_centre_x = area.position.x + area.size.width as i32 / 2;
  let area_centre_y = area.position.y + area.size.height as i32 / 2;

  Ok(Anchor::from_flags(
    window_centre_x >= area_centre_x,
    window_centre_y >= area_centre_y,
  ))
}

/// Moves the window to whichever corner it now sits closest to.
///
/// `last_attempt` remembers the (position, target) pair of the previous move so
/// a window that cannot be moved is not chased forever; pass `&mut None` for a
/// one-off request that should always try.
pub fn snap_to_nearest_anchor<R: Runtime>(
  window: &Window<R>,
  edge_margin: i32,
  last_attempt: &mut Option<(PhysicalPosition<i32>, PhysicalPosition<i32>)>,
) -> tauri::Result<()> {
  // A minimized window sits at a sentinel position far off every screen, and
  // `set_position` on it changes only where it will restore to, never what it
  // reports now. So the "already parked" check below can never be satisfied,
  // and every move we make raises another move event: the window is chased at
  // full speed, the message loop stops being serviced, and Windows replaces it
  // with a "Not Responding" ghost. Restoring raises its own move event, which
  // is when placement picks up again.
  if window.is_minimized().unwrap_or(false) {
    log::debug!("corner-snap: window is minimized; leaving it alone");
    return Ok(());
  }

  let Some(monitor) = pick_monitor(window) else {
    // Logged quietly: the placement check runs on a timer, and under WSLg this
    // is the normal answer every time.
    log::debug!("corner-snap: no monitor reported; cannot snap");
    return Ok(());
  };

  let size = window.outer_size()?;
  let anchor = occupied_anchor(window, &monitor, size)?;
  let target = anchor_position(&monitor, size, anchor, edge_margin);
  let position = window.outer_position()?;

  // Setting the position raises another move event. Stopping here when the
  // window is already parked keeps that from looping.
  if position == target {
    *last_attempt = None;
    return Ok(());
  }

  // The same move, from the same place, as last time: it did not take, so the
  // window is somewhere it cannot be moved from. Backstop for anything that
  // pins a window the way minimizing does; without it the retry raises another
  // move event and nothing ever breaks the cycle.
  if *last_attempt == Some((position, target)) {
    log::warn!(
      "corner-snap: window would not move from {position:?} to {target:?}; \
       leaving it until something else changes"
    );
    return Ok(());
  }

  *last_attempt = Some((position, target));
  window.set_position(target)
}
