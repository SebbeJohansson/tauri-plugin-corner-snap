# tauri-plugin-corner-snap

Keeps a Tauri window parked in a corner of the screen.

Drag the window anywhere and it slides back to whichever corner it was dropped
nearest. It re-parks itself when it is resized between its expanded and
collapsed sizes, when the work area changes (the taskbar moving, for instance),
and when a monitor is plugged in or pulled out.

Built for small always-on-top desktop widgets: undecorated, transparent,
fixed-size windows that should never end up half off the screen.

## Install

```toml
# src-tauri/Cargo.toml
tauri-plugin-corner-snap = { git = "https://github.com/…/tauri-plugin-corner-snap" }
```

```rust
use tauri_plugin_corner_snap::{Config, Corner, Manage};

tauri::Builder::default()
  .plugin(tauri_plugin_corner_snap::init(Config {
    expanded: (260.0, 90.0),
    collapsed: (44.0, 44.0),
    manage: Manage::Labels(vec!["main".into()]),
    initial_corner: Some(Corner::BottomRight),
    ..Default::default()
  }))
```

```json
// src-tauri/capabilities/default.json
{ "permissions": ["core:default", "corner-snap:default"] }
```

Windows declared in `tauri.conf.json` are created *after* plugins are set up and
*before* your own `setup` hook, so a window that starts `"visible": false` is
already parked by the time you show it:

```rust
.setup(|app| {
  app.get_webview_window("main").unwrap().show()?;
  Ok(())
})
```

## Config

| Field | Default | What it does |
| --- | --- | --- |
| `edge_margin` | `16` | Gap from the work area edge, in physical pixels. |
| `expanded` | `(320.0, 120.0)` | Size with the panel showing, in logical pixels. |
| `collapsed` | `(48.0, 48.0)` | Size when collapsed to a badge, in logical pixels. |
| `snap_delay` | `250ms` | How long the window must sit still before a drag counts as finished. |
| `placement_check` | `10s` | Backstop re-check interval. |
| `manage` | `Manage::All` | Which window labels to take charge of. |
| `initial_corner` | `None` | Where to park the window the moment it is created. |

The two sizes are placeholders; set them.

## Commands

Granted together by `corner-snap:default`, or individually as
`corner-snap:allow-set-collapsed` and `corner-snap:allow-snap`.

```ts
import { invoke } from '@tauri-apps/api/core'

// Resize between `expanded` and `collapsed`, staying in the current corner.
await invoke('plugin:corner-snap|set_collapsed', { collapsed: true })

// Re-park now, without waiting for the periodic check.
await invoke('plugin:corner-snap|snap')
```

## How it works

Dragging is handled by the operating system, which reports a stream of moves but
no "drag finished" event, so the end of a drag is inferred from the window going
quiet for `snap_delay`. Each managed window gets two threads: one that debounces
moves and snaps, and one that pokes the first on a timer. Both are shut down
when the window is destroyed.

On Windows the plugin also subclasses the window to catch `WM_DISPLAYCHANGE` and
`WM_SETTINGCHANGE`/`SPI_SETWORKAREA`, because neither Tauri nor the window layer
beneath it reports a monitor appearing or disappearing. Elsewhere the periodic
check is the only mechanism, which is why it exists.

## Limitations

- Corner placement needs a monitor to be reported. Under WSLg none are, so the
  window is left where it was and a warning is logged; run on a real desktop.
- Snapping is corner-only. There is no edge or free placement mode.
