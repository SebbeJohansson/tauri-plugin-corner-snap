//! Keeps a Tauri window parked against an edge or corner of the screen.
//!
//! The window can be dragged anywhere; when the drag stops it slides back to
//! whichever slot it was dropped nearest. It re-parks itself when it changes
//! between named states, when the work area changes (the taskbar moving, for
//! instance), and when a monitor is plugged in or pulled out.
//!
//! There are nine slots -- four corners, the middle of each edge, and the
//! centre -- but only the four corners are offered by default, so a config
//! written before edge slots existed behaves exactly as it did.
//!
//! # Configuring it in `tauri.conf.json`
//!
//! Nothing but `.plugin(tauri_plugin_corner_snap::init())` is needed in Rust:
//!
//! ```json
//! {
//!   "plugins": {
//!     "corner-snap": {
//!       "snapTo": ["topLeft", "topRight", "bottomLeft", "bottomRight", "left", "right"],
//!       "states": {
//!         "expanded": {
//!           "size": [260, 90],
//!           "verticalSize": [90, 320],
//!           "fill": true
//!         },
//!         "collapsed": { "size": [44, 44] }
//!       },
//!       "initialState": "expanded",
//!       "initialAnchor": "bottomRight",
//!       "manage": { "labels": ["main"] }
//!     }
//!   }
//! }
//! ```
//!
//! That widget is 260x90 in any corner, and a 90px-wide bar running the full
//! height of the screen when dragged to the left or right edge.
//!
//! # Configuring it in Rust
//!
//! [`init_with`] takes the same settings as a value and ignores the JSON.
//!
//! # Finding out where the window is
//!
//! Every time the window settles somewhere new the plugin emits
//! [`ANCHOR_EVENT`] to that window, carrying an [`AnchorChanged`]:
//!
//! ```ts
//! import { invoke } from '@tauri-apps/api/core'
//! import { getCurrentWindow } from '@tauri-apps/api/window'
//!
//! // The first placement is settled while the window is still being created,
//! // so ask once on startup; the event covers every change after that.
//! let where_ = await invoke('plugin:corner-snap|current_anchor')
//! await getCurrentWindow().listen('corner-snap://anchor', ({ payload }) => {
//!   where_ = payload  // { label, anchor, orientation }
//! })
//! ```
//!
//! `orientation` is what a widget that turns needs: it says which of the two
//! configured shapes is in use, which the webview cannot work out for itself
//! because it does not see the config.
//!
//! Repeats are swallowed, so the periodic placement check does not emit
//! anything while the window stays put.

mod commands;
mod config;
mod geometry;
mod report;
mod watcher;

#[cfg(windows)]
mod display_watch;

pub use config::{Config, Fill, Manage, WindowState};
pub use geometry::{Anchor, ALL_ANCHORS};
pub use report::{AnchorChanged, Orientation, Placement, ANCHOR_EVENT};

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{LogicalSize, Manager, PhysicalSize, Runtime, Window, WindowEvent};

use config::WindowState as State;
#[cfg(test)]
use geometry::CORNERS;
use geometry::{anchor_position, nearest, pick_monitor, window_rect, Area, Rect, Slot};
use report::Reporter;
use watcher::Watcher;

/// Plugin state, shared between the window hooks and the commands.
///
/// Held behind an `Arc` so `init` can hand a clone to the window hook without
/// depending on the managed state being in place. The config arrives later than
/// the handle does when it comes from `tauri.conf.json`, which is why it sits
/// behind a `OnceLock` rather than being a plain field.
#[derive(Clone)]
pub(crate) struct Shared(Arc<Inner>);

pub(crate) struct Inner {
  config: OnceLock<Config>,
  /// One entry per managed window. Removing an entry stops its threads.
  watchers: Mutex<HashMap<String, Watcher>>,
  /// What is known about each window. Keyed by label rather than living in the
  /// watcher, because the commands need one for a window before `attach` has
  /// necessarily run.
  windows: Mutex<HashMap<String, Tracked>>,
}

/// What the plugin remembers about one window between calls.
///
/// The state name has to be in here now. It used to be the webview's business
/// alone -- the plugin only ever moved the window, so the size it already had
/// was answer enough. Once the slot decides the size, a snap has to look up
/// `size`, `verticalSize` and `fill`, and for that it needs to know which state
/// the window is in.
#[derive(Clone, Default)]
pub(crate) struct Tracked {
  reporter: Reporter,
  state: Arc<Mutex<Option<String>>>,
}

impl Tracked {
  pub(crate) fn reporter(&self) -> &Reporter {
    &self.reporter
  }

  /// The name of the state the window is in, if the plugin has been told.
  pub(crate) fn state(&self) -> Option<String> {
    self
      .state
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .clone()
  }

  pub(crate) fn set_state(&self, name: &str) {
    *self
      .state
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(name.to_string());
  }
}

impl Shared {
  fn new() -> Self {
    Shared(Arc::new(Inner {
      config: OnceLock::new(),
      watchers: Mutex::new(HashMap::new()),
      windows: Mutex::new(HashMap::new()),
    }))
  }

  /// The config, or the defaults if setup somehow has not run yet. Defaults
  /// configure no states, so the worst case is a window nothing resizes.
  pub(crate) fn config(&self) -> &Config {
    self.0.config.get_or_init(Config::default)
  }

  /// What is tracked for one window, created on first ask.
  ///
  /// Every caller gets the same one for a given label, which is what keeps a
  /// placement from being announced twice by two different code paths.
  pub(crate) fn tracked(&self, label: &str) -> Tracked {
    self
      .0
      .windows
      .lock()
      .expect("corner-snap window registry is not poisoned")
      .entry(label.to_string())
      .or_default()
      .clone()
  }
}

/// The rectangle a window in `state` would occupy at `anchor`.
///
/// `current` is the size to use when the plugin has not been told which state
/// the window is in: with no state there is nothing to look a size up in, so it
/// keeps the one it has and only moves. That is what a config naming no states
/// has always done.
fn slot_at(
  anchor: Anchor,
  config: &Config,
  state: Option<&State>,
  area: &Area,
  scale: f64,
  current: PhysicalSize<u32>,
) -> Slot {
  let size = match state {
    Some(state) => state.physical_size(anchor, area, scale, config.edge_margin),
    None => current,
  };

  Slot {
    anchor,
    rect: Rect {
      position: anchor_position(area, size, anchor, config.edge_margin),
      size,
    },
  }
}

/// Every slot a dragged window in `state` is allowed to land in.
fn candidate_slots(
  config: &Config,
  state: Option<&State>,
  area: &Area,
  scale: f64,
  current: PhysicalSize<u32>,
) -> Vec<Slot> {
  config
    .slots_for(state)
    .iter()
    .map(|&anchor| slot_at(anchor, config, state, area, scale, current))
    .collect()
}

/// Puts a window where it belongs: the right slot, at the right size.
///
/// The one path for all of it -- the initial placement, `set_state`, `snap`,
/// and the watcher's periodic check. They used to be two nearly identical
/// routines, one that moved and one that moved and resized; now that a snap can
/// resize too, they are the same operation differing only in how the slot is
/// chosen.
///
/// `anchor_override` wins over the state's own `anchor`, which in turn wins over
/// the nearest slot. An explicit anchor from either is honoured whether or not
/// it appears in `snapTo`: that list restricts where a *drag* can put the
/// window, and a configured anchor is a direct instruction.
///
/// `last_attempt` remembers the (before, after) rectangles of the previous move
/// so a window that cannot be moved is not chased forever; pass `&mut None` for
/// a one-off request that should always try.
///
/// `Ok(None)` means there was nothing to work from -- no monitor, or the window
/// is minimized -- as opposed to `Ok(Some(_))`, which is where the window is
/// whether this call put it there or found it already parked.
pub(crate) fn reposition<R: Runtime>(
  window: &Window<R>,
  config: &Config,
  tracked: &Tracked,
  anchor_override: Option<Anchor>,
  last_attempt: &mut Option<(Rect, Rect)>,
) -> Result<Option<Placement>, String> {
  // A minimized window sits at a sentinel position far off every screen, and
  // `set_position` on it changes only where it will restore to, never what it
  // reports now. So the "already parked" check below can never be satisfied,
  // and every move we make raises another move event: the window is chased at
  // full speed, the message loop stops being serviced, and Windows replaces it
  // with a "Not Responding" ghost. Restoring raises its own move event, which
  // is when placement picks up again.
  if window.is_minimized().unwrap_or(false) {
    log::debug!("corner-snap: window is minimized; leaving it alone");
    return Ok(None);
  }

  let name = tracked.state();
  let state = name.as_deref().and_then(|name| config.states.get(name));

  let Some(monitor) = pick_monitor(window) else {
    // Logged quietly: the placement check runs on a timer, and under WSLg this
    // is the normal answer every time.
    log::debug!("corner-snap: no monitor reported; cannot place the window");

    // Still worth resizing, so a widget under WSLg is at least the right shape.
    // The unstretched horizontal size is the only one that can be worked out
    // without a work area to measure against.
    if let Some(state) = state {
      let (width, height) = state.size;
      return window
        .set_size(LogicalSize::new(width, height))
        .map(|()| None)
        .map_err(|error| error.to_string());
    }
    return Ok(None);
  };

  let area = Area::of(&monitor);
  let scale = window.scale_factor().map_err(|error| error.to_string())?;
  let current = window_rect(window).map_err(|error| error.to_string())?;

  // The slot is resolved from where the window is *now*, before anything is
  // resized. Measured afterwards, a window that grew in a corner overhangs the
  // screen edge, and its centre can land on the neighbouring monitor, which
  // would send it to that screen instead.
  let slot = match anchor_override.or_else(|| state.and_then(|state| state.anchor)) {
    Some(anchor) => slot_at(anchor, config, state, &area, scale, current.size),
    None => {
      let candidates = candidate_slots(config, state, &area, scale, current.size);
      // `slots_for` never returns an empty list, so this is unreachable; it
      // costs one line to not panic if that ever stops being true.
      nearest(current.centre(), &candidates)
        .ok_or_else(|| "no slots to choose from".to_string())?
    }
  };

  let placement = Placement {
    anchor: slot.anchor,
    orientation: Orientation::of(slot.rect.size),
  };

  // Both guards below compare whole rectangles rather than positions, because
  // a snap can now resize as well as move.
  //
  // Moving or resizing the window raises another move event. Stopping here when
  // it is already parked keeps that from looping -- and it has to include the
  // size, because a window held against the bottom or right edge moves when it
  // is resized, so a rectangle that is the right place but the wrong size would
  // otherwise be resized on every tick, forever.
  if current == slot.rect {
    *last_attempt = None;
    tracked.reporter.report(window, placement);
    return Ok(Some(placement));
  }

  // The same change, from the same rectangle, as last time: it did not take, so
  // the window is somewhere it cannot be moved from. Backstop for anything that
  // pins a window the way minimizing does; without it the retry raises another
  // move event and nothing ever breaks the cycle.
  if *last_attempt == Some((current, slot.rect)) {
    log::warn!(
      "corner-snap: window would not go from {current:?} to {:?}; \
       leaving it until something else changes",
      slot.rect
    );
    // Still where it is, even though it is not where it was asked to be.
    tracked.reporter.report(window, placement);
    return Ok(Some(placement));
  }

  log::debug!(
    "corner-snap: {} -> {:?} {}x{} at {},{}",
    name.as_deref().unwrap_or("(no state)"),
    slot.anchor,
    slot.rect.size.width,
    slot.rect.size.height,
    slot.rect.position.x,
    slot.rect.position.y,
  );

  *last_attempt = Some((current, slot.rect));
  geometry::move_into(window, current, slot.rect)?;

  // Announced only once the window is actually there, so a listener that reads
  // the window's position when it hears this sees the placement it was told
  // about.
  tracked.reporter.report(window, placement);
  Ok(Some(placement))
}

/// Where the window is right now, measured rather than remembered.
///
/// `None` when there is nothing to measure against, which under WSLg is the
/// only answer there is.
pub(crate) fn measure<R: Runtime>(
  window: &Window<R>,
  config: &Config,
  tracked: &Tracked,
) -> Option<Placement> {
  // A minimized window reports a sentinel position far off every screen, which
  // measures as a perfectly confident top-left. Better to say nothing and let
  // the caller fall back to where it was before it was minimized.
  if window.is_minimized().unwrap_or(false) {
    return None;
  }

  let monitor = pick_monitor(window)?;
  let area = Area::of(&monitor);
  let scale = window.scale_factor().ok()?;
  let current = window_rect(window).ok()?;

  let name = tracked.state();
  let state = name.as_deref().and_then(|name| config.states.get(name));
  let candidates = candidate_slots(config, state, &area, scale, current.size);

  Some(Placement {
    anchor: nearest(current.centre(), &candidates)?.anchor,
    // Measured from the window itself, not from the slot it is nearest: this
    // reports how the window *is*, and a window mid-drag has not turned yet.
    orientation: Orientation::of(current.size),
  })
}

/// Starts managing `window`: places it, watches it, and cleans up after it.
fn attach<R: Runtime>(window: Window<R>, shared: Shared) {
  let label = window.label().to_string();
  let config = shared.config();

  if !config.manage.covers(&label) {
    return;
  }

  let tracked = shared.tracked(&label);

  // Done before the watcher starts, so the first move the watcher sees is a
  // real one rather than this placement.
  //
  // Read per window: two managed windows share the states map but open in
  // their own state and their own corner, which is the whole point of the
  // `windows` block.
  let (initial_state, initial_anchor) = config.initial_for(&label);
  if let Some(state) = initial_state {
    tracked.set_state(state);
    if let Err(error) = reposition(&window, config, &tracked, initial_anchor, &mut None) {
      log::error!("corner-snap: could not place {label}: {error}");
    }
  }

  // Nothing is listening this early -- the webview has not run a line of its
  // own code yet -- so the placement above is announced to an empty room. The
  // `current_anchor` command is how the page catches up once it is running; the
  // event is for everything after that.
  let watcher = watcher::spawn(window.clone(), config.clone(), tracked);

  // Unplugging a monitor leaves the window at coordinates that exist on no
  // screen, and nothing in this stack reports that. On Windows the operating
  // system's own message says so immediately.
  //
  // Installed inline. run_on_main_thread was tried here and does nothing when
  // the caller is already the main thread -- it runs the closure immediately --
  // so it bought no deferral, only the appearance of one.
  #[cfg(windows)]
  match window.hwnd() {
    Ok(hwnd) => {
      if display_watch::watch(hwnd, watcher.sender()) {
        log::info!("corner-snap: watching display changes for {label}");
      } else {
        log::warn!(
          "corner-snap: could not watch display changes for {label}; \
           relying on the periodic check"
        );
      }
    }
    Err(error) => log::warn!("corner-snap: no window handle for {label}: {error}"),
  }

  let watcher_sender = watcher.sender();

  shared
    .0
    .watchers
    .lock()
    .expect("corner-snap watcher registry is not poisoned")
    .insert(label.clone(), watcher);

  // The sender is captured directly rather than looked up in the registry.
  // This callback runs on the window's own thread, on every move, and taking a
  // mutex there is a good way to stall the message loop that has to keep
  // pumping for anything else to work. A channel send cannot block.
  let moves = watcher_sender;
  let hooked = shared.clone();
  let hooked_window = window.clone();
  window.on_window_event(move |event| match event {
    // Resizes are reported too, now that the window's size is part of what
    // placement decides: something else changing the size leaves the window in
    // a rectangle no slot matches, and only a move used to say so.
    WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
      // The watcher owns the timing; this only reports that something happened.
      let _ = moves.send(());
    }
    WindowEvent::Destroyed => detach(&hooked_window, &hooked),
    _ => {}
  });
}

/// Stops managing a window and shuts its threads down.
fn detach<R: Runtime>(window: &Window<R>, shared: &Shared) {
  #[cfg(windows)]
  if let Ok(hwnd) = window.hwnd() {
    display_watch::unwatch(hwnd);
  }

  // Dropping the watcher closes its channels, which is how both of its threads
  // are told to stop.
  if let Ok(mut watchers) = shared.0.watchers.lock() {
    watchers.remove(window.label());
  }

  // A window that comes back under the same label starts afresh rather than
  // inheriting the state and placement the old one died in.
  if let Ok(mut windows) = shared.0.windows.lock() {
    windows.remove(window.label());
  }
}

/// Complains about anything that will only show up later as a command failing,
/// or as a setting quietly doing nothing.
fn warn_about(config: &Config) {
  if config.states.is_empty() {
    log::warn!("corner-snap: no states configured; set_state will reject every name");
  }

  if let Some(state) = &config.initial_state {
    if !config.states.contains_key(state) {
      log::error!(
        "corner-snap: initialState '{state}' is not one of: {}",
        config.state_names()
      );
    }
  }

  for (label, over) in &config.windows {
    // An override on a window the plugin was never given is silent otherwise:
    // the window opens wherever Tauri put it and nothing explains why.
    if !config.manage.covers(label) {
      log::warn!(
        "corner-snap: windows.{label} is set but '{label}' is not managed; add it \
         to manage.labels or the override does nothing"
      );
    }

    if let Some(state) = &over.initial_state {
      if !config.states.contains_key(state) {
        log::error!(
          "corner-snap: windows.{label}.initialState '{state}' is not one of: {}",
          config.state_names()
        );
      }
    }
  }

  if config.snap_to.is_empty() {
    log::warn!("corner-snap: snapTo is empty; falling back to the four corners");
  }

  for (name, state) in &config.states {
    if let Some(slots) = &state.snap_to {
      if slots.is_empty() {
        log::warn!(
          "corner-snap: snapTo for '{name}' is empty; falling back to the corners"
        );
      }
    }

    let slots = config.slots_for(Some(state));

    // Both of these are settings that cannot ever apply given the slots the
    // window is allowed into, which is exactly the kind of thing that is
    // otherwise only noticed by wondering why nothing happens.
    if state.vertical_size.is_some() && !slots.iter().any(|slot| slot.runs_vertically()) {
      log::warn!(
        "corner-snap: '{name}' has a verticalSize but cannot reach the left or \
         right slot, so it will never turn; add \"left\" or \"right\" to snapTo"
      );
    }

    if state.fill != Fill::None && !slots.iter().any(|slot| slot.is_side()) {
      log::warn!(
        "corner-snap: '{name}' has fill set but cannot reach an edge slot, so it \
         will never stretch; fill applies to \"top\", \"right\", \"bottom\" and \
         \"left\" only"
      );
    }

    if !state.fill.is_sensible() {
      log::warn!(
        "corner-snap: fill for '{name}' is {:?}; it must be true, false, or a \
         fraction above 0 and up to 1, and has been clamped",
        state.fill
      );
    }
  }
}

fn build<R: Runtime>(shared: Shared) -> TauriPlugin<R, Option<Config>> {
  let for_setup = shared.clone();

  Builder::<R, Option<Config>>::new("corner-snap")
    .invoke_handler(tauri::generate_handler![
      commands::set_state,
      commands::snap,
      commands::current_anchor
    ])
    .setup(move |app, api| {
      // A config passed to `init_with` is already in place, so this only has an
      // effect for `init`. An absent `plugins.corner-snap` arrives as null,
      // which is why the config type is an `Option`.
      for_setup
        .0
        .config
        .get_or_init(|| api.config().clone().unwrap_or_default());
      warn_about(for_setup.config());
      app.manage(for_setup.clone());
      Ok(())
    })
    .on_window_ready(move |window| attach(window, shared.clone()))
    .build()
}

/// Builds the plugin, reading its settings from `plugins.corner-snap` in
/// `tauri.conf.json`. Missing settings fall back to [`Config::default`].
pub fn init<R: Runtime>() -> TauriPlugin<R, Option<Config>> {
  build(Shared::new())
}

/// Builds the plugin with settings supplied from Rust, ignoring whatever
/// `tauri.conf.json` says.
pub fn init_with<R: Runtime>(config: Config) -> TauriPlugin<R, Option<Config>> {
  let shared = Shared::new();
  let _ = shared.0.config.set(config);
  build(shared)
}

#[cfg(test)]
mod tests {
  use super::*;
  use tauri::PhysicalPosition;

  /// A 1920x1040 work area, offset so nothing can pass by assuming the origin.
  fn area() -> Area {
    Area {
      position: PhysicalPosition::new(100, 200),
      size: PhysicalSize::new(1920, 1040),
    }
  }

  fn config(json: &str) -> Config {
    serde_json::from_str(json).expect("config should parse")
  }

  /// Picks the slot a window of `size` dropped at `centre` would snap into.
  ///
  /// The same two steps `reposition` takes once it has a monitor, minus the
  /// window -- which is the point, since no window can be built in a test and
  /// under WSLg none could be measured anyway.
  fn snap(
    config: &Config,
    state: Option<&str>,
    size: PhysicalSize<u32>,
    centre: (i32, i32),
  ) -> Slot {
    let state = state.and_then(|name| config.states.get(name));
    let candidates = candidate_slots(config, state, &area(), 1.0, size);

    nearest(PhysicalPosition::new(centre.0, centre.1), &candidates).expect("a slot")
  }

  fn rotating() -> Config {
    config(
      r#"{
        "snapTo": ["topLeft", "topRight", "bottomLeft", "bottomRight", "left", "right"],
        "states": {
          "expanded": {
            "size": [260, 90],
            "verticalSize": [90, 320],
            "fill": true
          },
          "collapsed": { "size": [44, 44] }
        },
        "edgeMargin": 16
      }"#,
    )
  }

  /// The whole feature, end to end but for the window: a wide panel dropped
  /// against the right edge becomes a tall bar running the full height, and the
  /// same panel dropped in the corner stays wide.
  #[test]
  fn a_panel_dropped_on_a_side_turns_and_stretches() {
    let config = rotating();
    let panel = PhysicalSize::new(260, 90);

    let side = snap(&config, Some("expanded"), panel, (1980, 720));
    assert_eq!(side.anchor, Anchor::Right);
    assert_eq!(side.rect.size, PhysicalSize::new(90, 1040 - 32));
    // Held against the right edge by the margin, and clearing both ends of it.
    assert_eq!(
      side.rect.position,
      PhysicalPosition::new(100 + 1920 - 90 - 16, 200 + 16)
    );

    let corner = snap(&config, Some("expanded"), panel, (1980, 240));
    assert_eq!(corner.anchor, Anchor::TopRight);
    assert_eq!(corner.rect.size, panel);
  }

  /// Turning has to work in reverse too, or a widget could dock to a side and
  /// never come back.
  #[test]
  fn a_bar_dragged_off_a_side_turns_back() {
    let config = rotating();
    let bar = PhysicalSize::new(90, 1008);

    let corner = snap(&config, Some("expanded"), bar, (140, 240));
    assert_eq!(corner.anchor, Anchor::TopLeft);
    assert_eq!(corner.rect.size, PhysicalSize::new(260, 90));
  }

  /// A state with no `verticalSize` can still dock to a side; it just keeps its
  /// shape there.
  #[test]
  fn a_state_that_does_not_turn_keeps_its_size_on_a_side() {
    let config = rotating();
    let badge = PhysicalSize::new(44, 44);

    let side = snap(&config, Some("collapsed"), badge, (1980, 720));
    assert_eq!(side.anchor, Anchor::Right);
    assert_eq!(side.rect.size, badge);
  }

  /// The default. A config written before edge slots existed cannot reach one
  /// however hard it is dragged at.
  #[test]
  fn corners_are_the_only_slots_unless_the_config_says_otherwise() {
    let config = config(r#"{ "states": { "expanded": { "size": [260, 90] } } }"#);
    let panel = PhysicalSize::new(260, 90);

    let found = snap(&config, Some("expanded"), panel, (1980, 720));
    assert!(CORNERS.contains(&found.anchor), "{:?}", found.anchor);
  }

  /// With no state recorded there is nothing to look a size up in, so the
  /// window keeps the one it has and is only moved. That is what a config
  /// naming no states has always done.
  #[test]
  fn an_untracked_window_is_moved_but_not_resized() {
    let config = rotating();
    let odd = PhysicalSize::new(300, 120);

    let found = snap(&config, None, odd, (1980, 720));
    assert_eq!(found.rect.size, odd);
  }

  /// `snapTo` restricts dragging. A configured anchor is a direct instruction,
  /// so `slot_at` honours one the list does not offer -- which is what lets
  /// `initialAnchor: "top"` work without opening `top` up to every drag.
  #[test]
  fn an_explicit_anchor_is_honoured_outside_the_snap_list() {
    let config = config(
      r#"{
        "snapTo": ["bottomRight"],
        "states": { "expanded": { "size": [260, 90], "verticalSize": [90, 320] } },
        "edgeMargin": 16
      }"#,
    );
    let state = config.states.get("expanded");

    let slot = slot_at(
      Anchor::Left,
      &config,
      state,
      &area(),
      1.0,
      PhysicalSize::new(260, 90),
    );

    assert_eq!(slot.anchor, Anchor::Left);
    assert_eq!(slot.rect.size, PhysicalSize::new(90, 320));
  }

  /// Orientation is what the webview relayouts on, so it has to follow the
  /// shape the window actually ends up with rather than the anchor alone.
  #[test]
  fn orientation_follows_the_shape_the_slot_gives_the_window() {
    let config = rotating();
    let panel = PhysicalSize::new(260, 90);

    let side = snap(&config, Some("expanded"), panel, (1980, 720));
    assert_eq!(Orientation::of(side.rect.size), Orientation::Vertical);

    let corner = snap(&config, Some("expanded"), panel, (1980, 240));
    assert_eq!(Orientation::of(corner.rect.size), Orientation::Horizontal);

    // A badge does not turn, whichever slot it lands in.
    let badge = snap(
      &config,
      Some("collapsed"),
      PhysicalSize::new(44, 44),
      (1980, 720),
    );
    assert_eq!(Orientation::of(badge.rect.size), Orientation::Horizontal);
  }

  /// A widget that should hug an edge with no gap at all: the case
  /// `my-widget` already configures.
  #[test]
  fn a_zero_margin_stretch_runs_edge_to_edge() {
    let config = config(
      r#"{
        "snapTo": ["left", "right"],
        "states": { "bar": { "size": [260, 90], "verticalSize": [90, 320], "fill": true } },
        "edgeMargin": 0
      }"#,
    );

    let slot = snap(
      &config,
      Some("bar"),
      PhysicalSize::new(260, 90),
      (1980, 720),
    );
    assert_eq!(slot.rect.size.height, 1040);
    assert_eq!(slot.rect.position.y, 200);
    assert_eq!(slot.rect.position.x, 100 + 1920 - 90);
  }
}
