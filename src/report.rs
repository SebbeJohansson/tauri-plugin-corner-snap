//! Telling the outside world where a window ended up.
//!
//! The plugin knows the answer at three moments -- the initial placement, the
//! end of a drag, and a re-park after the screen changed -- and they all funnel
//! through a [`Reporter`], one per managed window.

use std::sync::{Arc, Mutex};

use tauri::{Emitter, PhysicalSize, Runtime, Window};

use crate::geometry::Anchor;

/// Event emitted when a window settles somewhere new.
///
/// Sent to the window it is about, so a second managed window does not have to
/// filter out events meant for the first. The payload is [`AnchorChanged`].
pub const ANCHOR_EVENT: &str = "corner-snap://anchor";

/// Which way round the window is.
///
/// Measured from the size the window actually ends up with, rather than worked
/// out from the anchor. The webview cannot do that itself: whether a state
/// turns on a side edge depends on whether it was given a `verticalSize`, and
/// the webview does not see the config. Measuring also means there is always an
/// answer, including for a window the plugin is only moving.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Orientation {
  Horizontal,
  Vertical,
}

impl Orientation {
  pub(crate) fn of(size: PhysicalSize<u32>) -> Self {
    if size.height > size.width {
      Orientation::Vertical
    } else {
      Orientation::Horizontal
    }
  }
}

/// Where a window is parked and which way round it is.
///
/// Both halves together are what gets de-duplicated, because either can change
/// without the other. Collapsing an expanded panel to a badge in the `right`
/// slot keeps the anchor and changes the orientation; dragging a square badge
/// from one corner to another does the opposite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
  pub anchor: Anchor,
  pub orientation: Orientation,
}

/// Payload of [`ANCHOR_EVENT`].
///
/// The label is in here as well as being the event's target, so a listener on
/// the app handle -- which receives every window's events -- can tell them
/// apart. The placement is flattened, so `anchor` sits at the top level where
/// it always has.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorChanged {
  pub label: String,
  #[serde(flatten)]
  pub placement: Placement,
}

/// The last placement reported for one window.
///
/// Cloneable and shared: the window hook, the watcher threads and the commands
/// all hold the same one, which is what makes the de-duplication work. Without
/// it the periodic placement check would announce the same corner every ten
/// seconds forever.
#[derive(Clone, Default)]
pub(crate) struct Reporter(Arc<Mutex<Option<Placement>>>);

impl Reporter {
  /// Emits [`ANCHOR_EVENT`] unless this is the placement already reported.
  pub(crate) fn report<R: Runtime>(&self, window: &Window<R>, placement: Placement) {
    if !self.note(placement) {
      return;
    }

    let payload = AnchorChanged {
      label: window.label().to_string(),
      placement,
    };

    // Targeted at the window itself rather than broadcast. `emit` on a window
    // goes to the whole app, so in a two-widget app each one would hear about
    // the other's corner.
    if let Err(error) = window.emit_to(window.label(), ANCHOR_EVENT, payload) {
      log::warn!(
        "corner-snap: could not emit {ANCHOR_EVENT} for {}: {error}",
        window.label()
      );
    }
  }

  /// Records the placement without emitting, and says whether it changed.
  ///
  /// For a caller that is already handing the answer back to whoever asked:
  /// there is no news in an event that repeats a return value.
  pub(crate) fn note(&self, placement: Placement) -> bool {
    // Poisoning would mean a panic while holding a lock over two enum values,
    // and there is nothing here to leave half-written, so the value is taken
    // back rather than propagating the panic into a window hook.
    let mut last = self
      .0
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());

    if *last == Some(placement) {
      return false;
    }
    *last = Some(placement);
    true
  }

  /// The last placement reported, or `None` if none ever was.
  pub(crate) fn last(&self) -> Option<Placement> {
    *self
      .0
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn placement(anchor: Anchor, orientation: Orientation) -> Placement {
    Placement {
      anchor,
      orientation,
    }
  }

  #[test]
  fn the_first_placement_is_news_and_a_repeat_is_not() {
    let reporter = Reporter::default();
    let corner = placement(Anchor::TopLeft, Orientation::Horizontal);

    assert_eq!(reporter.last(), None);
    assert!(reporter.note(corner));
    assert!(!reporter.note(corner));
    assert_eq!(reporter.last(), Some(corner));
  }

  #[test]
  fn moving_somewhere_else_is_news_again() {
    let reporter = Reporter::default();

    reporter.note(placement(Anchor::TopLeft, Orientation::Horizontal));
    assert!(reporter.note(placement(Anchor::BottomRight, Orientation::Horizontal)));
  }

  /// Collapsing a vertical bar to a square badge leaves it in the same slot,
  /// so de-duplicating on the anchor alone would swallow the news that the
  /// webview has to relayout.
  #[test]
  fn turning_in_place_is_news_even_though_the_anchor_did_not_change() {
    let reporter = Reporter::default();

    reporter.note(placement(Anchor::Right, Orientation::Vertical));
    assert!(reporter.note(placement(Anchor::Right, Orientation::Horizontal)));
  }

  #[test]
  fn orientation_is_measured_from_the_size_the_window_gets() {
    assert_eq!(
      Orientation::of(PhysicalSize::new(260, 90)),
      Orientation::Horizontal
    );
    assert_eq!(
      Orientation::of(PhysicalSize::new(90, 320)),
      Orientation::Vertical
    );
    // A square badge is not turned, so it counts as the unturned one.
    assert_eq!(
      Orientation::of(PhysicalSize::new(44, 44)),
      Orientation::Horizontal
    );
  }

  /// The webview reads these strings, and the anchor names have to match the
  /// ones the config accepts, or `initialAnchor: "bottomRight"` and an event
  /// saying `"bottom_right"` would be two spellings of one corner.
  ///
  /// `anchor` staying at the top level is the other half of this: the payload
  /// gained a field rather than changing shape, so a listener written against
  /// the old one still works.
  #[test]
  fn the_payload_names_things_the_way_the_config_does() {
    let payload = AnchorChanged {
      label: "main".to_string(),
      placement: placement(Anchor::BottomRight, Orientation::Horizontal),
    };

    assert_eq!(
      serde_json::to_string(&payload).expect("payload should serialize"),
      r#"{"label":"main","anchor":"bottomRight","orientation":"horizontal"}"#
    );
  }
}
