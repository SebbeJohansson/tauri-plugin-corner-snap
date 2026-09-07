//! Telling the outside world which corner a window ended up in.
//!
//! The plugin knows the answer at three moments -- the initial placement, the
//! end of a drag, and a re-park after the screen changed -- and they all funnel
//! through a [`Reporter`], one per managed window.

use std::sync::{Arc, Mutex};

use tauri::{Emitter, Runtime, Window};

use crate::geometry::Anchor;

/// Event emitted when a window settles into a corner.
///
/// Sent to the window it is about, so a second managed window does not have to
/// filter out events meant for the first. The payload is [`AnchorChanged`].
pub const ANCHOR_EVENT: &str = "corner-snap://anchor";

/// Payload of [`ANCHOR_EVENT`].
///
/// The label is in here as well as being the event's target, so a listener on
/// the app handle -- which receives every window's events -- can tell them
/// apart.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorChanged {
  pub label: String,
  pub anchor: Anchor,
}

/// The last corner reported for one window.
///
/// Cloneable and shared: the window hook, the watcher threads and the commands
/// all hold the same one, which is what makes the de-duplication work. Without
/// it the periodic placement check would announce the same corner every ten
/// seconds forever.
#[derive(Clone, Default)]
pub(crate) struct Reporter(Arc<Mutex<Option<Anchor>>>);

impl Reporter {
  /// Emits [`ANCHOR_EVENT`] unless this is the corner already reported.
  pub(crate) fn report<R: Runtime>(&self, window: &Window<R>, anchor: Anchor) {
    if !self.note(anchor) {
      return;
    }

    let payload = AnchorChanged {
      label: window.label().to_string(),
      anchor,
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

  /// Records the corner without emitting, and says whether it changed.
  ///
  /// For a caller that is already handing the answer back to whoever asked:
  /// there is no news in an event that repeats a return value.
  pub(crate) fn note(&self, anchor: Anchor) -> bool {
    // Poisoning would mean a panic while holding a lock over one enum value,
    // and there is nothing here to leave half-written, so the value is taken
    // back rather than propagating the panic into a window hook.
    let mut last = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    if *last == Some(anchor) {
      return false;
    }
    *last = Some(anchor);
    true
  }

  /// The last corner reported, or `None` if none ever was.
  pub(crate) fn last(&self) -> Option<Anchor> {
    *self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn the_first_corner_is_news_and_a_repeat_is_not() {
    let reporter = Reporter::default();

    assert_eq!(reporter.last(), None);
    assert!(reporter.note(Anchor::TopLeft));
    assert!(!reporter.note(Anchor::TopLeft));
    assert_eq!(reporter.last(), Some(Anchor::TopLeft));
  }

  #[test]
  fn moving_to_another_corner_is_news_again() {
    let reporter = Reporter::default();

    reporter.note(Anchor::TopLeft);
    assert!(reporter.note(Anchor::BottomRight));
    assert_eq!(reporter.last(), Some(Anchor::BottomRight));
  }

  /// The webview reads these strings, and they have to match the ones the
  /// config accepts, or `initialAnchor: "bottomRight"` and an event saying
  /// `"bottom_right"` would be two spellings of one corner.
  #[test]
  fn the_payload_names_corners_the_way_the_config_does() {
    let payload = AnchorChanged {
      label: "main".to_string(),
      anchor: Anchor::BottomRight,
    };

    assert_eq!(
      serde_json::to_string(&payload).expect("payload should serialize"),
      r#"{"label":"main","anchor":"bottomRight"}"#
    );
  }
}
