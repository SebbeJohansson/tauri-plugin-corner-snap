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
tauri-plugin-corner-snap = { git = "https://github.com/SebbeJohansson/tauri-plugin-corner-snap", tag = "v0.1.0" }
```

```rust
// src-tauri/src/lib.rs — the whole of it
.plugin(tauri_plugin_corner_snap::init())
```

```json
// src-tauri/capabilities/default.json
{ "permissions": ["core:default", "corner-snap:default"] }
```

```json
// src-tauri/tauri.conf.json
{
  "plugins": {
    "corner-snap": {
      "states": {
        "expanded": { "size": [260, 90] },
        "collapsed": { "size": [44, 44] }
      },
      "initialState": "expanded",
      "initialAnchor": "bottomRight",
      "manage": { "labels": ["main"] }
    }
  }
}
```

Set the window's `width`/`height` in `tauri.conf.json` to match the initial
state, so nothing resizes visibly on the first frame.

Windows declared in `tauri.conf.json` are created *after* plugins are set up and
*before* your own `setup` hook, so a window that starts `"visible": false` is
already parked by the time you show it:

```rust
.setup(|app| {
  app.get_webview_window("main").unwrap().show()?;
  Ok(())
})
```

Prefer to configure it from Rust? `init_with(Config { .. })` takes the same
settings as a value and ignores the JSON.

## Config

Every key is optional; a block naming nothing but `states` is complete. Unknown
keys are rejected rather than ignored, so a typo fails the build instead of
quietly doing nothing.

| Key | Default | What it does |
| --- | --- | --- |
| `states` | `{}` | The shapes the window can take, by name. |
| `initialState` | `null` | State to enter when the window is created. |
| `initialAnchor` | `null` | Corner for that first placement only. |
| `edgeMargin` | `16` | Gap from the work area edge, in physical pixels. |
| `snapDelayMs` | `250` | How long the window must sit still before a drag counts as finished. |
| `placementCheckMs` | `10000` | Backstop re-check interval. |
| `manage` | `"all"` | `"all"`, or `{ "labels": ["main"] }`. |

A state is a size in logical pixels plus an optional anchor:

```json
"wide": { "size": [420, 90], "anchor": "topRight" }
```

Anchors are `topLeft`, `topRight`, `bottomLeft` and `bottomRight`.

A state with **no** anchor keeps whichever corner the window is already in, so
it grows and shrinks where the user left it. A state **with** one moves there
every time the window enters it.

`initialAnchor` exists because those two answer different questions. "Open in
the bottom-right, but afterwards stay wherever the user put it" cannot be said
with a state's own anchor: giving `expanded` an anchor would drag the window
back to that corner on every expand.

## Commands

Granted together by `corner-snap:default`, or individually as
`corner-snap:allow-set-state` and `corner-snap:allow-snap`.

```ts
import { invoke } from '@tauri-apps/api/core'

// Resize to a configured state, keeping the current corner unless that state
// names one. An unknown name rejects, listing the names that do exist.
await invoke('plugin:corner-snap|set_state', { state: 'collapsed' })

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
- The plugin does not track which state a window is in; the webview owns that.
  `set_state` is idempotent, so restating it after a reload is the way to
  recover.
