//! How the plugin is configured, from Rust or from `tauri.conf.json`.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Deserializer};

use crate::geometry::Anchor;

/// One named shape the window can take.
///
/// A widget usually has two — a panel and a badge — but nothing here assumes
/// that, so a third "wide" or "peek" state costs only a map entry.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowState {
  /// Window size in this state, in logical pixels.
  pub size: (f64, f64),
  /// Where to move when entering this state. `None` keeps the current anchor,
  /// so the window grows and shrinks in whichever corner the user left it in.
  #[serde(default)]
  pub anchor: Option<Anchor>,
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
}

/// Durations are milliseconds on the wire. Serde's own representation for
/// `Duration` is a `{ secs, nanos }` pair, which is no way to write a config.
fn millis<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
  u64::deserialize(deserializer).map(Duration::from_millis)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn parse(json: &str) -> Config {
    serde_json::from_str(json).expect("config should parse")
  }

  #[test]
  fn a_states_map_is_a_complete_config() {
    let config = parse(r#"{ "states": { "expanded": { "size": [260, 90] } } }"#);

    assert_eq!(config.states["expanded"].size, (260.0, 90.0));
    assert_eq!(config.states["expanded"].anchor, None);
    assert_eq!(config.initial_state, None);
    assert_eq!(config.initial_anchor, None);
    assert_eq!(config.edge_margin, 16);
    assert_eq!(config.snap_delay, Duration::from_millis(250));
    assert_eq!(config.placement_check, Duration::from_secs(10));
    assert!(config.manage.covers("anything"));
  }

  #[test]
  fn every_field_round_trips_from_json() {
    let config = parse(
      r#"{
        "states": {
          "expanded": { "size": [260, 90], "anchor": "bottomRight" },
          "collapsed": { "size": [44, 44] }
        },
        "initialState": "expanded",
        "initialAnchor": "topLeft",
        "edgeMargin": 24,
        "snapDelayMs": 400,
        "placementCheckMs": 30000,
        "manage": { "labels": ["main", "hud"] }
      }"#,
    );

    assert_eq!(config.states["expanded"].anchor, Some(Anchor::BottomRight));
    assert_eq!(config.states["collapsed"].size, (44.0, 44.0));
    assert_eq!(config.initial_state.as_deref(), Some("expanded"));
    assert_eq!(config.initial_anchor, Some(Anchor::TopLeft));
    assert_eq!(config.edge_margin, 24);
    assert_eq!(config.snap_delay, Duration::from_millis(400));
    assert_eq!(config.placement_check, Duration::from_secs(30));
    assert!(config.manage.covers("hud"));
    assert!(!config.manage.covers("other"));
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
    assert!(serde_json::from_str::<Config>(r#"{ "initial_state": "expanded" }"#).is_err());
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
