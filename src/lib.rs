//! Keeps a Tauri window parked in a corner of the screen.
//!
//! The window can be dragged anywhere; when the drag stops it slides back to
//! whichever corner it was dropped nearest. It re-parks itself when it is
//! resized between its expanded and collapsed sizes, when the work area changes
//! (the taskbar moving, for instance), and when a monitor is plugged in or
//! pulled out.
//!
//! ```no_run
//! tauri::Builder::default()
//!   .plugin(tauri_plugin_corner_snap::init(tauri_plugin_corner_snap::Config {
//!     expanded: (260.0, 90.0),
//!     collapsed: (44.0, 44.0),
//!     manage: tauri_plugin_corner_snap::Manage::Labels(vec!["main".into()]),
//!     initial_corner: Some(tauri_plugin_corner_snap::Corner::BottomRight),
//!     ..Default::default()
//!   }))
//!   .setup(|app| {
//!     // The plugin has already placed the window, so showing it here does not
//!     // flash at the default position.
//!     use tauri::Manager;
//!     app.get_webview_window("main").unwrap().show()?;
//!     Ok(())
//!   });
//! ```

mod commands;
mod geometry;
mod watcher;

#[cfg(windows)]
mod display_watch;

pub use geometry::Corner;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Manager, Runtime, Window, WindowEvent};

use geometry::{corner_position, pick_monitor};
use watcher::Watcher;

/// Which windows the plugin takes charge of.
#[derive(Clone, Debug)]
pub enum Manage {
  /// Every window the app opens.
  All,
  /// Only the windows with these labels.
  Labels(Vec<String>),
}

impl Manage {
  fn covers(&self, label: &str) -> bool {
    match self {
      Manage::All => true,
      Manage::Labels(labels) => labels.iter().any(|candidate| candidate == label),
    }
  }
}

/// How the plugin behaves. Build one with `..Default::default()` and override
/// what matters; the sizes almost always do.
#[derive(Clone, Debug)]
pub struct Config {
  /// Gap between the window and the edge of the work area, in physical pixels.
  pub edge_margin: i32,
  /// Window size with the panel showing, in logical pixels.
  pub expanded: (f64, f64),
  /// Window size when collapsed to its badge, in logical pixels.
  pub collapsed: (f64, f64),
  /// How long the window must sit still before it is treated as dropped.
  ///
  /// Dragging is handled by the operating system, which reports a stream of
  /// moves but no "drag finished" event, so the end of the drag is inferred
  /// from the window going quiet.
  pub snap_delay: Duration,
  /// How often the window's placement is re-checked.
  ///
  /// On Windows the display messages do the real work, so this is a backstop
  /// for anything they miss, and the only mechanism on other platforms. It is
  /// slow on purpose: a window at dead coordinates is rare, and re-checking
  /// often would mean fighting anything else that legitimately moves the
  /// window.
  pub placement_check: Duration,
  /// Which windows to manage.
  pub manage: Manage,
  /// Where to park the window the moment it is created. `None` leaves it
  /// wherever the window builder put it until something moves it.
  pub initial_corner: Option<Corner>,
}

impl Default for Config {
  fn default() -> Self {
    Self {
      edge_margin: 16,
      // Placeholders. A widget's real sizes belong in the consuming app.
      expanded: (320.0, 120.0),
      collapsed: (48.0, 48.0),
      snap_delay: Duration::from_millis(250),
      placement_check: Duration::from_secs(10),
      manage: Manage::All,
      initial_corner: None,
    }
  }
}

/// Plugin state, shared between the window hooks and the commands.
///
/// Held behind an `Arc` so `init` can hand a clone to the window hook without
/// depending on the managed state being in place: the hook and `setup` run in
/// an order the plugin does not control.
#[derive(Clone)]
pub(crate) struct Shared(Arc<Inner>);

pub(crate) struct Inner {
  config: Config,
  /// One entry per managed window. Removing an entry stops its threads.
  watchers: Mutex<HashMap<String, Watcher>>,
}

impl Shared {
  pub(crate) fn config(&self) -> &Config {
    &self.0.config
  }
}

/// Starts managing `window`: places it, watches it, and cleans up after it.
fn attach<R: Runtime>(window: Window<R>, shared: Shared) {
  let label = window.label().to_string();
  let config = shared.config();

  if !config.manage.covers(&label) {
    return;
  }

  // Done before the watcher starts, so the first move event the watcher sees is
  // a real one rather than this placement.
  if let Some(corner) = config.initial_corner {
    match (pick_monitor(&window), window.outer_size()) {
      (Some(monitor), Ok(size)) => {
        let target = corner_position(&monitor, size, corner, config.edge_margin);
        if let Err(error) = window.set_position(target) {
          log::error!("corner-snap: could not place {label}: {error}");
        }
      }
      (None, _) => log::warn!(
        "corner-snap: no monitor reported; leaving {label} at its default position. \
         WSLg does not expose monitors, so display-dependent behaviour needs a real desktop."
      ),
      (_, Err(error)) => log::error!("corner-snap: could not measure {label}: {error}"),
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

  shared
    .0
    .watchers
    .lock()
    .expect("corner-snap watcher registry is not poisoned")
    .insert(label.clone(), watcher);

  let hooked = shared.clone();
  let hooked_window = window.clone();
  window.on_window_event(move |event| match event {
    WindowEvent::Moved(_) => {
      // The watcher owns the timing; this only reports that a move happened.
      if let Ok(watchers) = hooked.0.watchers.lock() {
        if let Some(watcher) = watchers.get(hooked_window.label()) {
          watcher.nudge();
        }
      }
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

/// Builds the plugin.
pub fn init<R: Runtime>(config: Config) -> TauriPlugin<R> {
  let shared = Shared(Arc::new(Inner {
    config,
    watchers: Mutex::new(HashMap::new()),
  }));

  let for_setup = shared.clone();

  Builder::<R>::new("corner-snap")
    .invoke_handler(tauri::generate_handler![
      commands::set_collapsed,
      commands::snap
    ])
    .setup(move |app, _api| {
      app.manage(for_setup);
      Ok(())
    })
    .on_window_ready(move |window| attach(window, shared.clone()))
    .build()
}
