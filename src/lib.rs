//! Keeps a Tauri window parked in a corner of the screen.
//!
//! The window can be dragged anywhere; when the drag stops it slides back to
//! whichever corner it was dropped nearest. It re-parks itself when it changes
//! between named states, when the work area changes (the taskbar moving, for
//! instance), and when a monitor is plugged in or pulled out.
//!
//! # Configuring it in `tauri.conf.json`
//!
//! Nothing but `.plugin(tauri_plugin_corner_snap::init())` is needed in Rust:
//!
//! ```json
//! {
//!   "plugins": {
//!     "corner-snap": {
//!       "states": {
//!         "expanded": { "size": [260, 90] },
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
//! # Configuring it in Rust
//!
//! [`init_with`] takes the same settings as a value and ignores the JSON.

mod commands;
mod config;
mod geometry;
mod watcher;

#[cfg(windows)]
mod display_watch;

pub use config::{Config, Manage, WindowState};
pub use geometry::Anchor;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{LogicalSize, Manager, PhysicalSize, Runtime, Window, WindowEvent};

use geometry::{anchor_position, occupied_anchor, pick_monitor};
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
}

impl Shared {
  fn new() -> Self {
    Shared(Arc::new(Inner {
      config: OnceLock::new(),
      watchers: Mutex::new(HashMap::new()),
    }))
  }

  /// The config, or the defaults if setup somehow has not run yet. Defaults
  /// configure no states, so the worst case is a window nothing resizes.
  pub(crate) fn config(&self) -> &Config {
    self.0.config.get_or_init(Config::default)
  }
}

/// Resizes the window to a named state and puts it back against an anchor.
///
/// Shared by the `set_state` command and the initial placement, so a window
/// opens in exactly the shape a later `set_state` would give it.
pub(crate) fn apply_state<R: Runtime>(
  window: &Window<R>,
  config: &Config,
  name: &str,
  anchor_override: Option<Anchor>,
) -> Result<(), String> {
  let state = config.states.get(name).ok_or_else(|| {
    format!(
      "unknown window state '{name}'; configured states: {}",
      config.state_names()
    )
  })?;

  let logical = LogicalSize::new(state.size.0, state.size.1);

  // Every window call below is traced. Each one crosses into the window's own
  // thread, and when one of them fails to come back the last line logged is
  // the only thing that says which. Cheap, and only on an explicit request.
  log::debug!("corner-snap: apply_state({name}) -> pick_monitor");
  let Some(monitor) = pick_monitor(window) else {
    log::warn!("corner-snap: no monitor reported; resizing without moving");
    return window.set_size(logical).map_err(|error| error.to_string());
  };

  // The anchor is resolved before the resize. Measured afterwards, a window
  // that grew in a corner overhangs the screen edge, and its centre can land on
  // the neighbouring monitor, which would send it to that screen instead.
  let anchor = match anchor_override.or(state.anchor) {
    Some(anchor) => anchor,
    None => {
      log::debug!("corner-snap: apply_state({name}) -> outer_size");
      let size_before = window.outer_size().map_err(|error| error.to_string())?;
      log::debug!("corner-snap: apply_state({name}) -> outer_position");
      occupied_anchor(window, &monitor, size_before).map_err(|error| error.to_string())?
    }
  };

  log::debug!("corner-snap: apply_state({name}) -> scale_factor");
  let scale = window.scale_factor().map_err(|error| error.to_string())?;
  let size_after: PhysicalSize<u32> = logical.to_physical(scale);
  let target = anchor_position(&monitor, size_after, anchor, config.edge_margin);

  // Moving before resizing means the window grows into place, rather than
  // briefly spilling past the edge of the screen.
  log::debug!("corner-snap: apply_state({name}) -> set_position({target:?})");
  window
    .set_position(target)
    .map_err(|error| error.to_string())?;
  log::debug!("corner-snap: apply_state({name}) -> set_size({logical:?})");
  let result = window.set_size(logical).map_err(|error| error.to_string());
  log::debug!("corner-snap: apply_state({name}) -> returned");
  result
}

/// Starts managing `window`: places it, watches it, and cleans up after it.
fn attach<R: Runtime>(window: Window<R>, shared: Shared) {
  let label = window.label().to_string();
  let config = shared.config();

  if !config.manage.covers(&label) {
    return;
  }

  // Done before the watcher starts, so the first move the watcher sees is a
  // real one rather than this placement.
  if let Some(state) = &config.initial_state {
    if let Err(error) = apply_state(&window, config, state, config.initial_anchor) {
      log::error!("corner-snap: could not place {label}: {error}");
    }
  }

  let watcher = watcher::spawn(
    window.clone(),
    config.edge_margin,
    config.snap_delay,
    config.placement_check,
  );

  // Unplugging a monitor leaves the window at coordinates that exist on no
  // screen, and nothing in this stack reports that. On Windows the operating
  // system's own message says so immediately.
  //
  // Queued onto the main thread rather than installed here. This function runs
  // inside the window builder, so the window is still being put together --
  // wry has not necessarily finished attaching the WebView2 controller and its
  // own subclass. Inserting a subclass into that chain mid-construction puts
  // ours at a different place in the chain than the version this was extracted
  // from, which installed it after the window was fully built. Queueing runs it
  // on the same thread, one loop iteration later, when construction is done.
  #[cfg(windows)]
  {
    let subclass_window = window.clone();
    let subclass_sender = watcher.sender();
    let subclass_label = label.clone();
    if let Err(error) = window.app_handle().run_on_main_thread(move || {
      match subclass_window.hwnd() {
        Ok(hwnd) => {
          if display_watch::watch(hwnd, subclass_sender) {
            log::info!("corner-snap: watching display changes for {subclass_label}");
          } else {
            log::warn!(
              "corner-snap: could not watch display changes for {subclass_label}; \
               relying on the periodic check"
            );
          }
        }
        Err(error) => {
          log::warn!("corner-snap: no window handle for {subclass_label}: {error}")
        }
      }
    }) {
      log::warn!("corner-snap: could not queue the display watcher for {label}: {error}");
    }
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
    WindowEvent::Moved(_) => {
      // The watcher owns the timing; this only reports that a move happened.
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
}

/// Complains about anything that will only show up later as a command failing.
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
}

fn build<R: Runtime>(shared: Shared) -> TauriPlugin<R, Option<Config>> {
  let for_setup = shared.clone();

  Builder::<R, Option<Config>>::new("corner-snap")
    .invoke_handler(tauri::generate_handler![commands::set_state, commands::snap])
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
