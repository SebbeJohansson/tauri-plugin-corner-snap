//! How the plugin is configured, from Rust or from `tauri.conf.json`.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Deserializer};
use tauri::{LogicalSize, PhysicalSize};

use crate::geometry::{Anchor, Area, CORNERS};

/// How far a window stretches along the edge it is docked to.
///
/// Docking a widget to the right-hand edge and leaving it 320px tall in the
/// middle of a 1400px edge looks like a mistake, so a state can say it should
/// run the length of the edge instead.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Fill {
  /// Use the state's declared size. The default.
  #[default]
  None,
  /// Run the whole length of the edge, less the margin at each end.
  Full,
  /// Run this fraction of the edge, centred along it.
  Fraction(f64),
}

impl Fill {
  /// The fraction of the edge to occupy, or `None` for no stretching at all.
  pub(crate) fn fraction(self) -> Option<f64> {
    match self {
      Fill::None => None,
      Fill::Full => Some(1.0),
      // Clamped rather than rejected: a nonsense fraction is worth a warning
      // from `warn_about`, not a window of zero or negative height.
      Fill::Fraction(fraction) => Some(fraction.clamp(0.0, 1.0)),
    }
  }

  /// Whether the configured value was one this can actually honour, for the
  /// startup warnings.
  pub(crate) fn is_sensible(self) -> bool {
    match self {
      Fill::None | Fill::Full => true,
      Fill::Fraction(fraction) => fraction > 0.0 && fraction <= 1.0,
    }
  }
}

/// `false`, `true`, or a number: three JSON shapes for one setting.
impl<'de> Deserialize<'de> for Fill {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
      // Tried first, so `true` and `false` do not get read as numbers.
      Flag(bool),
      Fraction(f64),
    }

    Ok(match Repr::deserialize(deserializer)? {
      Repr::Flag(true) => Fill::Full,
      Repr::Flag(false) => Fill::None,
      Repr::Fraction(fraction) => Fill::Fraction(fraction),
    })
  }
}

/// One named shape the window can take.
///
/// A widget usually has two -- a panel and a badge -- but nothing here assumes
/// that, so a third "wide" or "peek" state costs only a map entry.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowState {
  /// Window size in this state, in logical pixels.
  pub size: (f64, f64),
  /// Size to use in the `left` and `right` slots, where the window runs up and
  /// down the screen instead of across it.
  ///
  /// Its presence *is* the opt-in to turning; there is no separate flag. A
  /// state without it keeps its declared size on a side edge.
  ///
  /// Kept as a second size rather than transposing `size`, because a vertical
  /// widget is rarely a literal transpose of the horizontal one -- a row of
  /// three stats stacked sideways wants to be taller and narrower than the swap
  /// would give it, and a transpose offers nowhere to say so.
  #[serde(default)]
  pub vertical_size: Option<(f64, f64)>,
  /// Whether to stretch along the docked edge, and how far.
  ///
  /// Applies to the axis parallel to the edge, and only in the four edge slots.
  /// A corner touches two edges, so "the edge it is docked to" has no answer
  /// there; corners always use the declared size.
  ///
  /// While filling, the declared length along that axis is unused -- only the
  /// thickness matters.
  #[serde(default)]
  pub fill: Fill,
  /// Where to move when entering this state. `None` keeps the current anchor,
  /// so the window grows and shrinks in whichever slot the user left it in.
  #[serde(default)]
  pub anchor: Option<Anchor>,
  /// Slots this state may be dragged into, overriding the config's own list.
  ///
  /// Per-state because a 44x44 badge can sensibly live anywhere while the
  /// expanded panel it belongs to has no business in the `top` slot.
  #[serde(default)]
  pub snap_to: Option<Vec<Anchor>>,
}

impl WindowState {
  /// The logical size this state takes in `anchor`, before any stretching.
  pub(crate) fn logical_size(&self, anchor: Anchor) -> (f64, f64) {
    match self.vertical_size {
      Some(size) if anchor.runs_vertically() => size,
      _ => self.size,
    }
  }

  /// The physical size this state takes in `anchor` on a `scale`-factor screen.
  ///
  /// Stretching happens here, in physical pixels, because that is the space the
  /// work area is measured in.
  pub(crate) fn physical_size(
    &self,
    anchor: Anchor,
    area: &Area,
    scale: f64,
    edge_margin: i32,
  ) -> PhysicalSize<u32> {
    let (width, height) = self.logical_size(anchor);
    let mut size: PhysicalSize<u32> = LogicalSize::new(width, height).to_physical(scale);

    let Some(fraction) = self.fill.fraction().filter(|_| anchor.is_side()) else {
      return size;
    };

    if anchor.runs_vertically() {
      size.height = stretch(area.size.height, fraction, edge_margin);
    } else {
      size.width = stretch(area.size.width, fraction, edge_margin);
    }

    size
  }
}

/// How long a window is along an edge of `area_length` when filling `fraction`
/// of it.
///
/// The cap is what puts the margin at *both* ends: a full stretch is the edge
/// less one margin per end, and the slot's own centring on that axis then
/// leaves exactly a margin at each. A fraction below 1 is not capped in
/// practice -- it is already short of the edge, so there is no end to clear.
fn stretch(area_length: u32, fraction: f64, edge_margin: i32) -> u32 {
  let wanted = (area_length as f64 * fraction).round() as i64;
  let cap = (area_length as i64 - 2 * edge_margin as i64).max(1);

  wanted.clamp(1, cap) as u32
}

/// Which windows the plugin takes charge of.
///
/// In `tauri.conf.json` this is either `"all"` or `{ "labels": ["main"] }`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Manage {
  /// Every window the app opens.
  All,
  /// Only the windows with these labels.
  Labels(Vec<String>),
}

impl Manage {
  pub(crate) fn covers(&self, label: &str) -> bool {
    match self {
      Manage::All => true,
      Manage::Labels(labels) => labels.iter().any(|candidate| candidate == label),
    }
  }
}

/// How the plugin behaves.
///
/// Every field has a default, so a `tauri.conf.json` block naming nothing but
/// `states` is a complete configuration. Unknown keys are rejected rather than
/// ignored: a typo that silently does nothing is the worst way to configure
/// something.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Config {
  /// The shapes the window can take, by name. `set_state` takes one of these
  /// names; anything else is an error.
  pub states: HashMap<String, WindowState>,
  /// Which state to enter the moment the window is created. `None` leaves the
  /// window at the size and position the window builder gave it.
  pub initial_state: Option<String>,
  /// Where to put the window on that first placement only.
  ///
  /// Separate from the initial state's own `anchor` because the two answer
  /// different questions. A state's anchor applies every time the window
  /// enters it; this one applies once. Without it, "open in the bottom-right
  /// but afterwards stay in whichever corner the user left it in" cannot be
  /// expressed: giving the state an anchor would drag the window back to that
  /// corner on every change into it.
  pub initial_anchor: Option<Anchor>,
  /// Slots a window may be dragged into.
  ///
  /// Defaults to the four corners, so a config written before edge slots
  /// existed keeps behaving exactly as it did. Add `"left"` and `"right"` to
  /// let a window dock to the sides, and so on.
  ///
  /// This restricts *dragging* only. An explicit `anchor` on a state, or
  /// `initialAnchor`, is a direct instruction and is honoured whether or not it
  /// appears here.
  pub snap_to: Vec<Anchor>,
  /// Gap between the window and the edge of the work area, in physical pixels.
  pub edge_margin: i32,
  /// How long the window must sit still before it is treated as dropped.
  ///
  /// Dragging is handled by the operating system, which reports a stream of
  /// moves but no "drag finished" event, so the end of the drag is inferred
  /// from the window going quiet.
  #[serde(rename = "snapDelayMs", deserialize_with = "millis")]
  pub snap_delay: Duration,
  /// How often the window's placement is re-checked.
  ///
  /// On Windows the display messages do the real work, so this is a backstop
  /// for anything they miss, and the only mechanism on other platforms. It is
  /// slow on purpose: a window at dead coordinates is rare, and re-checking
  /// often would mean fighting anything else that legitimately moves the
  /// window.
  #[serde(rename = "placementCheckMs", deserialize_with = "millis")]
  pub placement_check: Duration,
  /// Which windows to manage.
  pub manage: Manage,
}

impl Default for Config {
  fn default() -> Self {
    Self {
      states: HashMap::new(),
      initial_state: None,
      initial_anchor: None,
      snap_to: CORNERS.to_vec(),
      edge_margin: 16,
      snap_delay: Duration::from_millis(250),
      placement_check: Duration::from_secs(10),
      manage: Manage::All,
    }
  }
}

impl Config {
  /// Names the caller can pass to `set_state`, sorted, for error messages.
  pub(crate) fn state_names(&self) -> String {
    if self.states.is_empty() {
      return "none configured".to_string();
    }
    let mut names: Vec<&str> = self.states.keys().map(String::as_str).collect();
    names.sort_unstable();
    names.join(", ")
  }

  /// The slots a window in `state` may be dragged into.
  ///
  /// An empty list would leave a dragged window nothing to snap to, which reads
  /// as "free placement" but is far more likely a mistake, so it falls back to
  /// the corners; `warn_about` says so at startup.
  pub(crate) fn slots_for<'a>(&'a self, state: Option<&'a WindowState>) -> &'a [Anchor] {
    let configured = match state.and_then(|state| state.snap_to.as_deref()) {
      Some(slots) => slots,
      None => &self.snap_to,
    };

    if configured.is_empty() {
      &CORNERS
    } else {
      configured
    }
  }
}

/// Durations are milliseconds on the wire. Serde's own representation for
/// `Duration` is a `{ secs, nanos }` pair, which is no way to write a config.
fn millis<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
  u64::deserialize(deserializer).map(Duration::from_millis)
}

#[cfg(test)]
mod tests {
  use super::*;
  use tauri::PhysicalPosition;

  fn parse(json: &str) -> Config {
    serde_json::from_str(json).expect("config should parse")
  }

  fn state(json: &str) -> WindowState {
    serde_json::from_str(json).expect("state should parse")
  }

  /// A 1920x1040 work area, offset so nothing can pass by assuming the origin.
  fn area() -> Area {
    Area {
      position: PhysicalPosition::new(100, 200),
      size: PhysicalSize::new(1920, 1040),
    }
  }

  #[test]
  fn a_states_map_is_a_complete_config() {
    let config = parse(r#"{ "states": { "expanded": { "size": [260, 90] } } }"#);

    assert_eq!(config.states["expanded"].size, (260.0, 90.0));
    assert_eq!(config.states["expanded"].anchor, None);
    assert_eq!(config.states["expanded"].vertical_size, None);
    assert_eq!(config.states["expanded"].fill, Fill::None);
    assert_eq!(config.initial_state, None);
    assert_eq!(config.initial_anchor, None);
    assert_eq!(config.edge_margin, 16);
    assert_eq!(config.snap_delay, Duration::from_millis(250));
    assert_eq!(config.placement_check, Duration::from_secs(10));
    assert!(config.manage.covers("anything"));
  }

  /// A config written before edge slots existed has to keep behaving exactly as
  /// it did, which means corners and nothing else.
  #[test]
  fn snapping_defaults_to_the_four_corners() {
    assert_eq!(Config::default().snap_to, CORNERS.to_vec());
    assert_eq!(parse("{}").slots_for(None), CORNERS);
  }

  #[test]
  fn every_field_round_trips_from_json() {
    let config = parse(
      r#"{
        "states": {
          "expanded": {
            "size": [260, 90],
            "verticalSize": [90, 320],
            "fill": true,
            "anchor": "bottomRight"
          },
          "collapsed": { "size": [44, 44], "snapTo": ["top", "bottom"] }
        },
        "initialState": "expanded",
        "initialAnchor": "topLeft",
        "snapTo": ["topLeft", "left", "right"],
        "edgeMargin": 24,
        "snapDelayMs": 400,
        "placementCheckMs": 30000,
        "manage": { "labels": ["main", "hud"] }
      }"#,
    );

    let expanded = &config.states["expanded"];
    assert_eq!(expanded.anchor, Some(Anchor::BottomRight));
    assert_eq!(expanded.vertical_size, Some((90.0, 320.0)));
    assert_eq!(expanded.fill, Fill::Full);
    assert_eq!(config.states["collapsed"].size, (44.0, 44.0));
    assert_eq!(config.initial_state.as_deref(), Some("expanded"));
    assert_eq!(config.initial_anchor, Some(Anchor::TopLeft));
    assert_eq!(
      config.snap_to,
      vec![Anchor::TopLeft, Anchor::Left, Anchor::Right]
    );
    assert_eq!(config.edge_margin, 24);
    assert_eq!(config.snap_delay, Duration::from_millis(400));
    assert_eq!(config.placement_check, Duration::from_secs(30));
    assert!(config.manage.covers("hud"));
    assert!(!config.manage.covers("other"));
  }

  #[test]
  fn a_state_can_narrow_the_slots_the_config_offers() {
    let config = parse(
      r#"{
        "snapTo": ["topLeft", "topRight"],
        "states": {
          "panel": { "size": [260, 90] },
          "badge": { "size": [44, 44], "snapTo": ["left", "right"] }
        }
      }"#,
    );

    assert_eq!(
      config.slots_for(Some(&config.states["panel"])),
      [Anchor::TopLeft, Anchor::TopRight]
    );
    assert_eq!(
      config.slots_for(Some(&config.states["badge"])),
      [Anchor::Left, Anchor::Right]
    );
  }

  /// Reads as "free placement", but is far more likely a mistake.
  #[test]
  fn an_empty_slot_list_falls_back_to_the_corners() {
    assert_eq!(parse(r#"{ "snapTo": [] }"#).slots_for(None), CORNERS);
  }

  #[test]
  fn fill_accepts_a_flag_or_a_fraction() {
    assert_eq!(
      state(r#"{ "size": [1, 1], "fill": true }"#).fill,
      Fill::Full
    );
    assert_eq!(
      state(r#"{ "size": [1, 1], "fill": false }"#).fill,
      Fill::None
    );
    assert_eq!(
      state(r#"{ "size": [1, 1], "fill": 0.6 }"#).fill,
      Fill::Fraction(0.6)
    );
  }

  #[test]
  fn a_state_without_a_vertical_size_keeps_its_shape_on_a_side() {
    let state = state(r#"{ "size": [260, 90] }"#);

    assert_eq!(state.logical_size(Anchor::Right), (260.0, 90.0));
    assert_eq!(
      state.physical_size(Anchor::Right, &area(), 1.0, 16),
      PhysicalSize::new(260, 90)
    );
  }

  /// The case the feature exists for: wide in a corner, tall on a side.
  #[test]
  fn a_vertical_size_is_used_only_where_the_window_runs_vertically() {
    let state = state(r#"{ "size": [260, 90], "verticalSize": [90, 320] }"#);

    assert_eq!(state.logical_size(Anchor::Left), (90.0, 320.0));
    assert_eq!(state.logical_size(Anchor::Right), (90.0, 320.0));
    assert_eq!(state.logical_size(Anchor::TopRight), (260.0, 90.0));
    assert_eq!(state.logical_size(Anchor::Top), (260.0, 90.0));
    assert_eq!(state.logical_size(Anchor::Center), (260.0, 90.0));
  }

  #[test]
  fn filling_a_side_stretches_the_axis_along_that_edge() {
    let state =
      state(r#"{ "size": [260, 90], "verticalSize": [90, 320], "fill": true }"#);

    // Docked left or right: the height stretches, the width is the thickness.
    let bar = state.physical_size(Anchor::Right, &area(), 1.0, 16);
    assert_eq!(bar, PhysicalSize::new(90, 1040 - 32));

    // Docked top: the *width* stretches instead, and the horizontal size is
    // the one in use, so the height is 90 rather than the vertical 320.
    let strip = state.physical_size(Anchor::Top, &area(), 1.0, 16);
    assert_eq!(strip, PhysicalSize::new(1920 - 32, 90));
  }

  /// A corner touches two edges, so there is no single axis to stretch along.
  #[test]
  fn filling_does_nothing_in_a_corner_or_the_centre() {
    let state = state(r#"{ "size": [260, 90], "fill": true }"#);

    assert_eq!(
      state.physical_size(Anchor::BottomRight, &area(), 1.0, 16),
      PhysicalSize::new(260, 90)
    );
    assert_eq!(
      state.physical_size(Anchor::Center, &area(), 1.0, 16),
      PhysicalSize::new(260, 90)
    );
  }

  #[test]
  fn a_fraction_stretches_part_of_the_edge() {
    let state = state(r#"{ "size": [260, 90], "verticalSize": [90, 320], "fill": 0.5 }"#);

    assert_eq!(
      state.physical_size(Anchor::Right, &area(), 1.0, 16),
      PhysicalSize::new(90, 520)
    );
  }

  /// Sizes are logical and the work area is physical, so a stretched window has
  /// to be worked out in physical pixels or it comes out half the screen on a
  /// 200% display.
  #[test]
  fn sizes_scale_but_a_stretched_axis_is_already_physical() {
    let state =
      state(r#"{ "size": [260, 90], "verticalSize": [90, 320], "fill": true }"#);

    assert_eq!(
      state.physical_size(Anchor::Right, &area(), 2.0, 16),
      PhysicalSize::new(180, 1040 - 32)
    );
  }

  /// A margin wider than the screen would otherwise ask for a window of zero or
  /// negative height, which Windows does not take kindly to.
  #[test]
  fn a_stretch_never_comes_out_at_zero() {
    assert_eq!(stretch(1040, 1.0, 9999), 1);
    assert_eq!(stretch(1040, 0.0, 0), 1);
  }

  #[test]
  fn a_nonsense_fraction_is_clamped_and_flagged() {
    assert_eq!(Fill::Fraction(4.0).fraction(), Some(1.0));
    assert_eq!(Fill::Fraction(-1.0).fraction(), Some(0.0));
    assert!(!Fill::Fraction(4.0).is_sensible());
    assert!(!Fill::Fraction(0.0).is_sensible());
    assert!(Fill::Fraction(0.5).is_sensible());
    assert!(Fill::Full.is_sensible());
    assert!(Fill::None.is_sensible());
  }

  #[test]
  fn manage_all_is_a_bare_string() {
    assert!(parse(r#"{ "manage": "all" }"#).manage.covers("anything"));
  }

  /// A misspelled key that is quietly ignored is the worst way to find out a
  /// setting did nothing.
  #[test]
  fn unknown_keys_are_rejected() {
    let error = serde_json::from_str::<Config>(r#"{ "edgeMargins": 4 }"#)
      .expect_err("a typo should not be accepted");
    assert!(error.to_string().contains("edgeMargins"), "{error}");
  }

  #[test]
  fn snake_case_keys_are_rejected() {
    assert!(
      serde_json::from_str::<Config>(r#"{ "initial_state": "expanded" }"#).is_err()
    );
    assert!(serde_json::from_str::<WindowState>(
      r#"{ "size": [1, 1], "vertical_size": [1, 1] }"#
    )
    .is_err());
  }

  /// An absent `plugins.corner-snap` reaches the plugin as JSON null, which is
  /// why the plugin's config type is an `Option`.
  #[test]
  fn an_absent_config_is_none_rather_than_an_error() {
    let config: Option<Config> = serde_json::from_str("null").expect("null should parse");
    assert!(config.is_none());
  }

  #[test]
  fn state_names_are_listed_for_error_messages() {
    let config = parse(
      r#"{ "states": { "wide": { "size": [1, 1] }, "badge": { "size": [1, 1] } } }"#,
    );
    assert_eq!(config.state_names(), "badge, wide");
    assert_eq!(Config::default().state_names(), "none configured");
  }
}
