//! The threads that decide *when* a window gets re-parked.
//!
//! Two of them per managed window: one waits for a drag to go quiet and then
//! snaps, the other pokes it on a timer as a backstop. Both are shut down by
//! dropping the [`Watcher`] handle, which is what happens when the window is
//! destroyed.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;

use tauri::{Runtime, Window};

use crate::config::Config;
use crate::{reposition, Tracked};

/// Everything holding a managed window's threads open.
///
/// Dropping this closes both channels, which is how the threads are told to
/// stop: the ticker sees its stop channel disconnect and exits, and the snap
/// watcher's `recv` fails once the last move sender is gone.
pub struct Watcher {
  moves: Sender<()>,
  _stop_ticker: Sender<()>,
}

impl Watcher {
  /// A sender other reporters (window events, display messages) can keep.
  pub fn sender(&self) -> Sender<()> {
    self.moves.clone()
  }
}

/// Starts both threads for `window`.
///
/// `config.snap_delay` is how long the window must sit still before it is
/// treated as dropped: dragging is handled by the operating system, which
/// reports a stream of moves but no "drag finished" event, so the end of a drag
/// is inferred from the window going quiet.
///
/// `config.placement_check` is how often the placement is re-checked. On
/// Windows the display messages do the real work, so this is a backstop for
/// anything they miss, and the only mechanism on other platforms. Keep it slow:
/// a window at dead coordinates is rare, and re-checking often means fighting
/// anything else that legitimately moves the window.
///
/// `tracked` carries both the state the window is in -- which is what decides
/// the size a slot gets -- and the reporter that announces where it lands. The
/// reporter ignores repeats, so the ticker can run forever without saying
/// anything.
pub fn spawn<R: Runtime>(window: Window<R>, config: Config, tracked: Tracked) -> Watcher {
  let (moves, receiver) = mpsc::channel::<()>();

  let snap_delay = config.snap_delay;
  let placement_check = config.placement_check;

  let label = window.label().to_string();
  thread::spawn(move || {
    // Carried across iterations so a window that refuses to move is noticed
    // once rather than chased on every event.
    let mut last_attempt = None;

    while receiver.recv().is_ok() {
      // Swallow the rest of the stream until the window has been still.
      while receiver.recv_timeout(snap_delay).is_ok() {}

      // Resizing from here is safe for the same reason `set_state` spawns a
      // thread: this is not the main thread, so Tauri posts the calls to the
      // event loop to run between callbacks rather than re-entering it
      // mid-dispatch. See the long comment in `commands.rs`.
      //
      // No anchor override: the whole point of this thread is to pick the slot
      // from where the window was dropped.
      if let Err(error) = reposition(&window, &config, &tracked, None, &mut last_attempt)
      {
        log::error!("corner-snap: could not place {label}: {error}");
      }
    }
  });

  let (stop_ticker, stop_rx) = mpsc::channel::<()>();
  let ticker_moves = moves.clone();
  thread::spawn(move || loop {
    // Routed through the same channel as real moves, so it inherits the quiet
    // period: a tick that lands mid-drag waits for the drag to finish instead
    // of yanking the window out from under the cursor. When nothing has
    // changed the snap finds the window already parked and does nothing.
    if ticker_moves.send(()).is_err() {
      break;
    }
    match stop_rx.recv_timeout(placement_check) {
      Err(RecvTimeoutError::Timeout) => {}
      // Told to stop, or the handle was dropped.
      _ => break,
    }
  });

  Watcher {
    moves,
    _stop_ticker: stop_ticker,
  }
}
