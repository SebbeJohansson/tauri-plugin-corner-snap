//! Notices when the set of displays changes, on Windows.
//!
//! Neither Tauri nor the window layer beneath it raises an event for a monitor
//! appearing or disappearing, so the message is taken straight from the
//! operating system. Windows broadcasts `WM_DISPLAYCHANGE` to every top-level
//! window when the display configuration changes, and `WM_SETTINGCHANGE` with
//! `SPI_SETWORKAREA` when the usable area changes, which is what happens when
//! the taskbar moves or changes size.
//!
//! The window already has a subclass installed by the window layer. Adding a
//! second one with its own id is supported: each is passed the message in turn,
//! and this one always hands the message on.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
  SPI_SETWORKAREA, WM_DISPLAYCHANGE, WM_SETTINGCHANGE,
};

/// Where each watched window reports to, keyed by window handle.
///
/// A channel sender cannot be shared across threads on its own, so the map
/// lives behind a mutex in a static. That keeps the window procedure free of
/// raw pointers. Keyed by handle rather than held once globally, so a second
/// window gets its own events instead of silently getting none.
static NOTIFY: OnceLock<Mutex<HashMap<isize, Sender<()>>>> = OnceLock::new();

/// Distinguishes this subclass from the one the window layer installs.
const SUBCLASS_ID: usize = 0x10CD;

fn registry() -> &'static Mutex<HashMap<isize, Sender<()>>> {
  NOTIFY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn key(hwnd: HWND) -> isize {
  hwnd.0 as isize
}

unsafe extern "system" fn subclass_proc(
  hwnd: HWND,
  message: u32,
  wparam: WPARAM,
  lparam: LPARAM,
  _id: usize,
  _data: usize,
) -> LRESULT {
  let displays_changed = message == WM_DISPLAYCHANGE
    || (message == WM_SETTINGCHANGE && wparam.0 as u32 == SPI_SETWORKAREA.0);

  if displays_changed {
    if let Ok(senders) = registry().lock() {
      if let Some(sender) = senders.get(&key(hwnd)) {
        // The receiving side debounces, so reporting is all this does.
        let _ = sender.send(());
      }
    }
  }

  unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

/// Starts reporting display changes for `hwnd` through `sender`.
///
/// Returns false if the subclass could not be installed, in which case the
/// periodic placement check is the only thing keeping the window on screen.
/// Must be called on the thread that owns the window.
pub fn watch(hwnd: HWND, sender: Sender<()>) -> bool {
  match registry().lock() {
    Ok(mut senders) => {
      senders.insert(key(hwnd), sender);
    }
    Err(_) => return false,
  }

  // Installing with the same procedure and id twice replaces the first, so a
  // recycled handle cannot end up subclassed two deep.
  unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0) }.as_bool()
}

/// Stops reporting for `hwnd`.
///
/// The subclass itself is left in place: it dies with the window, and removing
/// it would have to happen on the window's own thread. With no sender
/// registered it does nothing but hand messages on.
pub fn unwatch(hwnd: HWND) {
  if let Ok(mut senders) = registry().lock() {
    senders.remove(&key(hwnd));
  }
}
