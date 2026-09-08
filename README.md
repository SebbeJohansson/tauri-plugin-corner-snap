# tauri-plugin-corner-snap

Keeps a Tauri window parked against an edge or corner of the screen.

Drag the window anywhere and it slides back to whichever slot it was dropped
nearest. It re-parks itself when it changes between its named states, when the
work area changes (the taskbar moving, for instance), and when a monitor is
plugged in or pulled out.

A window can also **change shape** depending on where it lands: a 260x90 panel
in the corners, a 90px bar running the full height of the screen when dragged to
a side edge.

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

## Slots

There are nine places a window can sit:

```
topLeft      top      topRight
left        center        right
bottomLeft  bottom  bottomRight
```

**Only the four corners are offered by default.** A config written before edge
slots existed behaves exactly as it did; opening the rest up is opt-in, per
config or per state:

```json
"snapTo": ["topLeft", "topRight", "bottomLeft", "bottomRight", "left", "right"]
```

`snapTo` restricts where a *drag* can put the window. A state's own `anchor`, and
`initialAnchor`, are direct instructions and are honoured whether or not they
appear in the list — so `"initialAnchor": "top"` works without opening `top` up
to every drag.

The window goes to whichever slot's **landing rectangle** is centred nearest the
window's own centre. Measuring the rectangle the window would land in, rather
than the one it is in, is what makes a shape-changing widget pick sensibly: the
`right` candidate is measured as the tall narrow bar it would *become*.

## Turning

Give a state a `verticalSize` and it uses that in the `left` and `right` slots,
where the window runs up and down the screen instead of across it:

```json
"expanded": {
  "size": [260, 90],
  "verticalSize": [90, 320]
}
```

The presence of the key **is** the opt-in; there is no separate flag. A state
without one can still dock to a side, it just keeps its shape there.

It is a second size rather than a transpose of the first because a vertical
widget is rarely a literal transpose of the horizontal one — a row of three
stats stacked sideways wants to be taller and narrower than `[90, 260]`, and a
transpose offers nowhere to say so.

## Stretching

Docking a widget to the right-hand edge and leaving it 320px tall in the middle
of a 1400px edge looks like a mistake, so a state can run the length of the edge
instead:

```json
"expanded": {
  "size": [260, 90],
  "verticalSize": [90, 320],
  "fill": true
}
```

`fill` applies to **the axis parallel to the docked edge, in the four edge slots
only**:

| Slot | What stretches | Result for the state above |
| --- | --- | --- |
| `right`, `left` | height | 90 wide, the full height of the work area |
| `top`, `bottom` | width | the full width, 90 tall |
| any corner, `center` | nothing | 260x90 |

A corner touches two edges, so "the edge it is docked to" has no single answer
there. Rather than invent one, corners always use the declared size.

`fill` takes `true`, `false` (the default), or a fraction: `"fill": 0.6` gives a
bar 60% of the edge's length, centred along it.

Two things worth knowing:

- The margin comes off **both** ends, so a fully stretched window clears the top
  and bottom of the work area by `edgeMargin` just as it clears the right of it.
  At `edgeMargin: 0` it runs flush from edge to edge.
- While filling, the length in the declared size is unused — only the thickness
  matters. `verticalSize: [90, 320]` with `fill: true` uses the `90` and ignores
  the `320`.

## Config

Every key is optional; a block naming nothing but `states` is complete. Unknown
keys are rejected rather than ignored, so a typo fails the build instead of
quietly doing nothing.

| Key | Default | What it does |
| --- | --- | --- |
| `states` | `{}` | The shapes the window can take, by name. |
| `initialState` | `null` | State to enter when the window is created. |
| `initialAnchor` | `null` | Slot for that first placement only. |
| `snapTo` | the four corners | Slots a drag may put the window in. |
| `edgeMargin` | `16` | Gap from the work area edge, in physical pixels. |
| `snapDelayMs` | `250` | How long the window must sit still before a drag counts as finished. |
| `placementCheckMs` | `10000` | Backstop re-check interval. |
| `manage` | `"all"` | `"all"`, or `{ "labels": ["main"] }`. |
| `windows` | `{}` | Per-label overrides of `initialState` and `initialAnchor`. |

A state takes a size in logical pixels and four optional keys:

| Key | Default | What it does |
| --- | --- | --- |
| `size` | required | Size in logical pixels. |
| `verticalSize` | `null` | Size in the `left`/`right` slots. Absent means the state never turns. |
| `fill` | `false` | Stretch along the docked edge. `true`, `false`, or a fraction. |
| `anchor` | `null` | Slot to move to every time the window enters this state. |
| `snapTo` | inherits | Narrows the config's slot list for this state alone. |

### Two windows, two corners

`states` is shared on purpose: names are global, each window picks the one it
wants, and two widgets of the same size cost one entry. What cannot be shared is
where a window *opens* — so `windows` overrides that per label:

```json
{
  "states": {
    "expanded": { "size": [260, 90] },
    "collapsed": { "size": [44, 44] }
  },
  "initialState": "expanded",
  "initialAnchor": "bottomLeft",
  "manage": { "labels": ["main", "kaggriculture"] },
  "windows": {
    "kaggriculture": { "initialAnchor": "bottomRight" }
  }
}
```

`main` opens bottom-left, `kaggriculture` bottom-right, both in `expanded`, and
both then keep whichever slot the user drags them into. A key an override does
not name falls back to the config's own value, so the block above says the one
thing that differs and nothing else. An override on a label outside
`manage.labels` is warned about at startup rather than silently ignored.

A state with **no** anchor keeps whichever slot the window is already in, so it
grows and shrinks where the user left it. A state **with** one moves there every
time the window enters it.

`initialAnchor` exists because those two answer different questions. "Open in
the bottom-right, but afterwards stay wherever the user put it" cannot be said
with a state's own anchor: giving `expanded` an anchor would drag the window
back to that corner on every expand.

Per-state `snapTo` is there because a 44x44 badge can sensibly live anywhere
while the expanded panel it belongs to has no business in the `top` slot.

## Commands

Granted together by `corner-snap:default`, or individually as
`corner-snap:allow-set-state`, `corner-snap:allow-snap` and
`corner-snap:allow-current-anchor`.

```ts
import { invoke } from '@tauri-apps/api/core'

// Resize to a configured state, keeping the current slot unless that state
// names one. An unknown name rejects, listing the names that do exist.
await invoke('plugin:corner-snap|set_state', { state: 'collapsed' })

// Re-park now, without waiting for the periodic check.
await invoke('plugin:corner-snap|snap')

// Where the window is, or null if no monitor was reported.
// { anchor: 'right', orientation: 'vertical' }
const placed = await invoke('plugin:corner-snap|current_anchor')
```

`set_state` is also how the plugin learns which state the window is in, which it
needs in order to know what size a slot should give the window. Call it once on
startup — it is idempotent, so that also recovers the right shape after a reload
in development.

## Events

The plugin emits `corner-snap://anchor` whenever the window settles somewhere
*different* — after a drag, after a `set_state` that moves it, and after a
display change re-parks it. Repeats are swallowed, so a window that stays put
emits nothing however often the placement check runs.

```ts
{ label: 'main', anchor: 'bottomRight', orientation: 'horizontal' }
```

The event is sent to the window it is about, not broadcast, so a second managed
window does not have to filter out the first one's placements. The payload
carries the label anyway, for a listener on the app handle.

`orientation` is `'horizontal'` or `'vertical'`, and it is what a widget that
turns should relayout on. It is measured from the size the window actually ends
up with — taller than wide is `'vertical'`, anything else is `'horizontal'` — so
there is always an answer, including for a window the plugin is only moving.

The webview cannot work this out from the anchor alone: whether a state turns on
a side edge depends on whether it was given a `verticalSize`, and the webview
does not see the config.

Both halves are de-duplicated together, because either can change without the
other. Collapsing an expanded bar to a square badge in the `right` slot keeps the
anchor and changes the orientation; dragging that badge between two corners does
the opposite.

The first placement is settled while the window is still being created, before
the page has run a line of its own code, so that one event goes out to an empty
room. Ask once on startup and listen from then on:

```ts
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'

let placed = await invoke('plugin:corner-snap|current_anchor')

await getCurrentWindow().listen('corner-snap://anchor', ({ payload }) => {
  placed = payload
})
```

That is the hook for anything that should follow the window: switching a flex
direction when it turns, opening a menu upwards in the bottom slots, pointing a
tooltip inwards.

From Rust the event name and payload types are exported as `ANCHOR_EVENT`,
`AnchorChanged`, `Placement` and `Orientation`.

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

Moving a transparent, always-on-top window off the edge of the screen took the
process down once, so a window is never allowed to occupy a rectangle outside
the work area, not even for one frame between two calls. Changing shape
therefore happens in three steps — shrink to the smaller of the two sizes on
each axis, move, then grow — because no single order of "move" and "resize"
works when one axis shrinks and the other grows, which is exactly what turning
does.

## Limitations

- Placement needs a monitor to be reported. Under WSLg none are, so the window
  is resized but not moved, and a warning is logged; run on a real desktop.
- Snapping is to one of the nine slots. There is no free placement mode, and no
  way to snap to a slot's edge rather than its centre.
- `fill` stretches along one axis only. A window cannot be told to fill a
  quadrant, or the whole work area.
