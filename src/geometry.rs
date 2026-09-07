//! Working out where a window belongs on the screen it is on.
//!
//! Everything here is arithmetic over an [`Area`] and a [`Rect`], deliberately
//! kept clear of the config: the module that knows what a state is builds the
//! candidate [`Slot`]s and hands them over. That also makes the placement maths
//! testable without a display, which matters more than usual here -- WSLg
//! reports no monitors, so a test that needed one could never run.

use tauri::{Monitor, PhysicalPosition, PhysicalSize, Runtime, Window};

/// A place on the monitor's work area to hold a window against.
///
/// Nine slots: the four corners, the middle of each of the four edges, and the
/// centre. The four corner names are the ones this plugin shipped with and mean
/// exactly what they always did.
#[derive(
  Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "camelCase")]
pub enum Anchor {
  TopLeft,
  Top,
  TopRight,
  Left,
  Center,
  Right,
  BottomLeft,
  Bottom,
  BottomRight,
}

/// Where a window sits on one axis.
///
/// The two-bool `(right, bottom)` pair this replaced could not say "centred",
/// which is the whole reason edge slots were impossible to express.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Align {
  Start,
  Center,
  End,
}

/// Every slot, in reading order.
///
/// Handy for a config written in Rust that wants to offer the lot:
/// `snap_to: ALL_ANCHORS.to_vec()`.
pub const ALL_ANCHORS: [Anchor; 9] = [
  Anchor::TopLeft,
  Anchor::Top,
  Anchor::TopRight,
  Anchor::Left,
  Anchor::Center,
  Anchor::Right,
  Anchor::BottomLeft,
  Anchor::Bottom,
  Anchor::BottomRight,
];

/// The four corners: what `snapTo` defaults to, so a config written before edge
/// slots existed behaves exactly as it did.
pub(crate) const CORNERS: [Anchor; 4] = [
  Anchor::TopLeft,
  Anchor::TopRight,
  Anchor::BottomLeft,
  Anchor::BottomRight,
];

impl Anchor {
  /// Alignment on the horizontal and vertical axes, in that order.
  pub(crate) fn aligns(self) -> (Align, Align) {
    use Align::{Center, End, Start};
    match self {
      Anchor::TopLeft => (Start, Start),
      Anchor::Top => (Center, Start),
      Anchor::TopRight => (End, Start),
      Anchor::Left => (Start, Center),
      Anchor::Center => (Center, Center),
      Anchor::Right => (End, Center),
      Anchor::BottomLeft => (Start, End),
      Anchor::Bottom => (Center, End),
      Anchor::BottomRight => (End, End),
    }
  }

  /// Whether this slot is docked to one edge and centred along it.
  ///
  /// True for the four edge middles and nothing else. A corner touches two
  /// edges, so "the edge it is docked to" has no answer there, and the centre
  /// touches none.
  pub(crate) fn is_side(self) -> bool {
    matches!(
      self,
      Anchor::Top | Anchor::Right | Anchor::Bottom | Anchor::Left
    )
  }

  /// Whether a window in this slot runs up and down the screen.
  ///
  /// This is what decides whether a state's `verticalSize` is used, and which
  /// axis `fill` stretches.
  pub(crate) fn runs_vertically(self) -> bool {
    matches!(self, Anchor::Left | Anchor::Right)
  }
}

/// A rectangle in physical pixels: a monitor's work area, or a window.
///
/// Tauri's own rectangle type is not named the same way across the versions
/// this has to build against, so the monitor's work area is copied into this
/// on the way in. It also lets the tests below build one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
  pub position: PhysicalPosition<i32>,
  pub size: PhysicalSize<u32>,
}

impl Area {
  /// The monitor's work area: the screen minus the taskbar.
  pub fn of(monitor: &Monitor) -> Self {
    let area = monitor.work_area();
    Area {
      position: PhysicalPosition::new(area.position.x, area.position.y),
      size: PhysicalSize::new(area.size.width, area.size.height),
    }
  }
}

/// Where a window is and how big it is, as one value.
///
/// Both loop guards below compare whole rectangles rather than positions,
/// because a snap can now resize as well as move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
  pub position: PhysicalPosition<i32>,
  pub size: PhysicalSize<u32>,
}

impl Rect {
  pub fn centre(&self) -> PhysicalPosition<i32> {
    PhysicalPosition::new(
      self.position.x + self.size.width as i32 / 2,
      self.position.y + self.size.height as i32 / 2,
    )
  }
}

/// The rectangle a window would occupy if it snapped to one anchor.
///
/// Candidates are compared as the rectangle they would *land* in, not the one
/// the window is in now. That is what makes a rotating widget pick the right
/// slot: the `right` candidate is measured as the tall narrow bar it would
/// become, so dragging to the right edge halfway down beats `topRight` on its
/// own merits, with no thresholds involved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
  pub anchor: Anchor,
  pub rect: Rect,
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

/// Where a window of `size` sits when held against `anchor`.
///
/// `edge_margin` is a gap from the edges the window is held against. It is not
/// applied to a centred axis: there is no edge there to be away from.
pub fn anchor_position(
  area: &Area,
  size: PhysicalSize<u32>,
  anchor: Anchor,
  edge_margin: i32,
) -> PhysicalPosition<i32> {
  let (horizontal, vertical) = anchor.aligns();

  PhysicalPosition::new(
    place_axis(
      area.position.x,
      area.size.width as i32,
      size.width as i32,
      horizontal,
      edge_margin,
    ),
    place_axis(
      area.position.y,
      area.size.height as i32,
      size.height as i32,
      vertical,
      edge_margin,
    ),
  )
}

/// One axis of [`anchor_position`].
fn place_axis(
  area_start: i32,
  area_length: i32,
  window_length: i32,
  align: Align,
  edge_margin: i32,
) -> i32 {
  match align {
    Align::Start => area_start + edge_margin,
    Align::Center => area_start + (area_length - window_length) / 2,
    Align::End => area_start + area_length - window_length - edge_margin,
  }
}

/// The slot whose landing rectangle is centred nearest `centre`.
///
/// Centres are compared rather than nearest edges. Measuring to the nearest
/// point of each candidate looks more natural until a slot is stretched: a
/// full-height right-hand bar has a zero vertical term, so *anything* on the
/// right half of the screen reads as touching it and the two right-hand corners
/// become unreachable. Comparing centres keeps every slot winnable, and it
/// needs no thresholds -- a widget dropped in the top-right is a few tens of
/// pixels from the `topRight` centre and several hundred from the full-height
/// `right` one.
///
/// Ties go to whichever slot comes first in `slots`.
pub fn nearest(centre: PhysicalPosition<i32>, slots: &[Slot]) -> Option<Slot> {
  slots
    .iter()
    .min_by_key(|slot| {
      let slot_centre = slot.rect.centre();
      let dx = (slot_centre.x - centre.x) as i64;
      let dy = (slot_centre.y - centre.y) as i64;
      dx * dx + dy * dy
    })
    .copied()
}

/// Where the window is and how big it is, right now.
pub fn window_rect<R: Runtime>(window: &Window<R>) -> tauri::Result<Rect> {
  Ok(Rect {
    position: window.outer_position()?,
    size: window.outer_size()?,
  })
}

/// Moves and resizes a window into `target` without it ever occupying a
/// rectangle outside the work area.
///
/// That guarantee is the whole point of the three steps, and it is not
/// negotiable: moving a transparent, always-on-top window off the edge of the
/// screen is what took the process down.
///
/// The naive version picks an order -- size then move, or move then size --
/// from whether the window is growing. That works only while one bool can
/// describe the change. A rotation shrinks one axis and grows the other, so
/// "growing" is false and the window moves first; a 260x90 sent to the `right`
/// slot's x (`right_edge - 90 - margin`) hangs 170px off the screen. Turning on
/// `fill` makes it worse, growing the height fifteen-fold.
///
/// So: shrink to the per-axis minimum of the two sizes, move, then grow. Every
/// intermediate rectangle is inside the work area --
///
/// - shrinking about the window's own top-left, from any rectangle that is
///   inside, stays inside: the rectangle only moves away from the edges it was
///   held against;
/// - the waypoint is no larger than the target on either axis, and the target
///   position was computed for the target size, so the waypoint fits inside a
///   rectangle that itself fits;
/// - the final grow lands exactly on the slot.
///
/// -- and it subsumes both of the orders it replaces rather than adding a third.
/// A step that would change nothing is skipped, so a pure move still raises one
/// message rather than three.
pub fn move_into<R: Runtime>(
  window: &Window<R>,
  from: PhysicalSize<u32>,
  target: Rect,
) -> tauri::Result<()> {
  let waypoint = PhysicalSize::new(
    from.width.min(target.size.width),
    from.height.min(target.size.height),
  );

  if waypoint != from {
    window.set_size(waypoint)?;
  }
  window.set_position(target.position)?;
  if target.size != waypoint {
    window.set_size(target.size)?;
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  /// A 1920x1080 screen with a 40px taskbar along the bottom, offset so that a
  /// test cannot pass by treating the work area as starting at the origin.
  fn area() -> Area {
    Area {
      position: PhysicalPosition::new(100, 200),
      size: PhysicalSize::new(1920, 1040),
    }
  }

  fn slot(anchor: Anchor, size: PhysicalSize<u32>, margin: i32) -> Slot {
    Slot {
      anchor,
      rect: Rect {
        position: anchor_position(&area(), size, anchor, margin),
        size,
      },
    }
  }

  #[test]
  fn corners_sit_a_margin_in_from_both_edges() {
    let size = PhysicalSize::new(260, 90);

    assert_eq!(
      anchor_position(&area(), size, Anchor::TopLeft, 16),
      PhysicalPosition::new(116, 216)
    );
    assert_eq!(
      anchor_position(&area(), size, Anchor::BottomRight, 16),
      // 100 + 1920 - 260 - 16, 200 + 1040 - 90 - 16
      PhysicalPosition::new(1744, 1134)
    );
  }

  #[test]
  fn an_edge_slot_is_centred_along_that_edge() {
    let size = PhysicalSize::new(90, 320);
    let position = anchor_position(&area(), size, Anchor::Right, 16);

    // Held against the right edge by the margin, centred vertically.
    assert_eq!(position.x, 100 + 1920 - 90 - 16);
    assert_eq!(position.y, 200 + (1040 - 320) / 2);
  }

  /// The margin is a gap from an edge the window is held against. A centred
  /// axis has no such edge, so applying it there would shift the window off
  /// centre for no reason.
  #[test]
  fn the_margin_does_not_apply_to_a_centred_axis() {
    let size = PhysicalSize::new(90, 320);

    assert_eq!(
      anchor_position(&area(), size, Anchor::Right, 0).y,
      anchor_position(&area(), size, Anchor::Right, 64).y
    );
  }

  #[test]
  fn the_centre_slot_is_centred_on_both_axes() {
    let size = PhysicalSize::new(260, 90);
    let position = anchor_position(&area(), size, Anchor::Center, 16);

    assert_eq!(position.x, 100 + (1920 - 260) / 2);
    assert_eq!(position.y, 200 + (1040 - 90) / 2);
  }

  /// A window stretched to the full length of an edge ends up a margin in from
  /// *both* ends of it, which is why the margin is subtracted twice when the
  /// stretched length is worked out.
  #[test]
  fn a_fully_stretched_window_clears_both_ends_by_the_margin() {
    let size = PhysicalSize::new(90, 1040 - 2 * 16);
    let position = anchor_position(&area(), size, Anchor::Right, 16);

    assert_eq!(position.y, 200 + 16);
    assert_eq!(position.y + size.height as i32, 200 + 1040 - 16);
  }

  #[test]
  fn a_window_dropped_in_a_corner_picks_that_corner() {
    let panel = PhysicalSize::new(260, 90);
    let slots: Vec<Slot> = CORNERS
      .iter()
      .map(|&anchor| slot(anchor, panel, 16))
      .collect();

    let found = nearest(PhysicalPosition::new(180, 260), &slots).expect("a slot");
    assert_eq!(found.anchor, Anchor::TopLeft);
  }

  /// The case the whole feature exists for: a wide panel that becomes a tall
  /// bar on the side. Dropped against the right edge halfway down, the tall
  /// candidate has to win, and dropped in the corner the wide one has to.
  #[test]
  fn a_rotating_widget_prefers_the_side_only_away_from_the_corners() {
    let panel = PhysicalSize::new(260, 90);
    let bar = PhysicalSize::new(90, 1040 - 32);
    let mut slots: Vec<Slot> = CORNERS
      .iter()
      .map(|&anchor| slot(anchor, panel, 16))
      .collect();
    slots.push(slot(Anchor::Right, bar, 16));

    // Right edge, halfway down.
    let middle = nearest(PhysicalPosition::new(1980, 720), &slots).expect("a slot");
    assert_eq!(middle.anchor, Anchor::Right);
    assert_eq!(middle.rect.size, bar);

    // Right edge, hard against the top.
    let corner = nearest(PhysicalPosition::new(1980, 240), &slots).expect("a slot");
    assert_eq!(corner.anchor, Anchor::TopRight);
    assert_eq!(corner.rect.size, panel);
  }

  /// Slots the config does not allow are simply not candidates, which is what
  /// keeps a wide panel out of the `top` slot without any special case.
  #[test]
  fn only_the_slots_offered_can_be_chosen() {
    let panel = PhysicalSize::new(260, 90);
    let slots = [slot(Anchor::BottomLeft, panel, 16)];

    let found = nearest(PhysicalPosition::new(1980, 240), &slots).expect("a slot");
    assert_eq!(found.anchor, Anchor::BottomLeft);
  }

  #[test]
  fn nothing_is_nearest_to_no_slots() {
    assert_eq!(nearest(PhysicalPosition::new(0, 0), &[]), None);
  }

  #[test]
  fn a_rectangle_reports_its_own_centre() {
    let rect = Rect {
      position: PhysicalPosition::new(100, 200),
      size: PhysicalSize::new(260, 90),
    };

    assert_eq!(rect.centre(), PhysicalPosition::new(230, 245));
  }
}
