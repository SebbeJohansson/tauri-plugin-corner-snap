# Edge snapping, rotation and stretch

Design for letting a window dock to the *middle of an edge* rather than only to
a corner, change shape when it gets there, and optionally run the length of the
edge it is docked to.

The motivating case: a 260x90 widget that sits horizontally in any corner, and
becomes a narrow vertical bar running the full height of the screen when dragged
to the left or right edge.

**Status: built.** Every step below is implemented and covered by tests; this
document is kept as the reasoning behind the shape it took, not as a plan. Two
things ended up better than they are described here, and are marked *Built:*
where they come up:

- orientation is measured from the size the window ends up with, rather than
  looked up from which of the two configured shapes is in use (step 8);
- an `Area` type replaced the monitor in the placement maths, which made all of
  it unit-testable without a display — worth more than usual here, since WSLg
  reports no monitors and a test needing one could never run.

Written down in the order it has to be built, because each step depends on the
one above it.

## 1. An anchor becomes nine slots, not four

`Anchor` is currently two bools -- `(right, bottom)` -- via `flags` and
`from_flags` in `src/geometry.rs`. That representation *is* the limitation: two
bools cannot express "centred on this axis".

Replace it with two three-way axes, kept internal, behind a flat public enum so
the wire format stays a plain string:

```rust
enum Align { Start, Center, End }   // internal only

pub enum Anchor {
  TopLeft,    Top,    TopRight,
  Left,       Center, Right,
  BottomLeft, Bottom, BottomRight,
}

impl Anchor {
  fn aligns(self) -> (Align, Align) { .. }
  fn is_side(self) -> bool { .. }        // Top | Right | Bottom | Left
  fn runs_vertically(self) -> bool { .. } // Left | Right
}
```

`anchor_position` then collapses to one helper applied per axis:

```rust
fn place_axis(area_start: i32, area_len: i32, win_len: i32, align: Align, margin: i32) -> i32 {
  match align {
    Align::Start  => area_start + margin,
    Align::Center => area_start + (area_len - win_len) / 2,
    Align::End    => area_start + area_len - win_len - margin,
  }
}
```

`edge_margin` deliberately does not apply to a centred axis: there is no edge to
be away from.

The four existing names keep their spelling, so every config and every
`corner-snap://anchor` payload already in the wild still means what it meant.
`top`, `right`, `bottom`, `left` and `center` are new.

## 2. Slot picking becomes nearest-candidate

`occupied_anchor` compares the window's centre against the work area's centre
and reads off two bools. That answers a four-way question and cannot be extended
to answer a nine-way one.

Replace it with: build the landing rectangle for every **allowed** slot, and
pick the slot whose rectangle centre is nearest the window's current centre.
Nine cheap computations per snap.

Two things fall out of measuring against the *landing* rectangle rather than the
current one:

- Rotation is handled for free. The `right` candidate is measured as its 90x1400
  vertical rectangle, not as the 260x90 the window happens to be now, so
  dragging to the right edge halfway down genuinely beats `topRight`.
- An arbitrary subset of slots needs no special cases. A widget that allows only
  the four corners behaves exactly as it does today.

### Why centre distance and not edge distance

The tempting alternative is to clamp the window's centre into each candidate
rectangle and measure to that clamped point. Do not: for a full-height right-hand
bar the vertical term collapses to zero, so *anything* on the right half of the
screen at any height reads as touching it, and the two right-hand corners become
unreachable.

Centre distance keeps the corners winnable. A widget dropped in the top-right is
~45px from the `topRight` centre and ~600px from the full-height `right` centre;
one dropped 60% of the way down the right edge is nearer `right`. That is the
behaviour we want, and it needs no thresholds.

### Which slots are allowed

Nine slots is too many for most widgets -- a wide panel has no business at `top`.
So a list, defaulting to today's behaviour:

```json
{
  "snapTo": ["topLeft", "topRight", "bottomLeft", "bottomRight", "left", "right"]
}
```

| Where | Key | Default |
| --- | --- | --- |
| `Config` | `snapTo` | the four corners |
| `WindowState` | `snapTo` | inherits the config's |

Per-state as well as global, because a 44x44 badge can sensibly live anywhere
while the expanded panel it belongs to cannot.

## 3. A state gets a second shape

One new key on `WindowState`:

```json
"expanded": {
  "size": [260, 90],
  "verticalSize": [90, 320]
}
```

The presence of `verticalSize` *is* the opt-in to turning; no separate boolean.
`left` and `right` use it, every other slot uses `size`.

Chosen over auto-transposing `size` into `[90, 260]` because a vertical widget
is rarely a literal transpose of the horizontal one. A row of three stats laid
out sideways wants to be taller and narrower than the swap would give it, and
the transpose offers nowhere to say so.

A state with no `verticalSize` never changes shape. It can still dock to a side;
it just keeps its declared size there.

## 4. Stretching along the docked edge

Docking a widget to the right edge and leaving it 320px tall in the middle of a
1400px edge looks like a mistake. So a state can run the length of the edge it
is docked to:

```json
"expanded": {
  "size": [260, 90],
  "verticalSize": [90, 320],
  "fill": true
}
```

`fill` is `false` (the default), `true`, or a fraction between 0 and 1.

**It applies to the axis parallel to the docked edge, and only at a side slot.**
Docked `right`, that axis is vertical, so the height becomes
`work_height - 2 * edge_margin`; the width still comes from `verticalSize`.
Docked `top`, the axis is horizontal and the width stretches instead.

The margin is subtracted **twice** -- once at each end -- so a stretched window
respects the same gap from the work area edge on all sides. With
`edgeMargin: 0` it goes flush from edge to edge, which is what `my-widget`
already wants.

A fraction stretches proportionally and stays centred on that axis:
`"fill": 0.6` on a `right` slot gives a bar 60% of the work area's height,
vertically centred.

### Fill does nothing at a corner

A corner touches two edges, so "along the edge" has no single answer there.
Rather than invent one, corners always use the declared size. Predictable, and
it matches the intent: fill means "docked to a side, run the length of it".

### The declared length is unused while filling

With `fill: true`, the `320` in `verticalSize: [90, 320]` is ignored -- only the
thickness matters. That is one line of documentation rather than a schema
change; the alternative, a sentinel inside the array (`[90, "fill"]`), buys
nothing and makes both the deserializer and the JSON schema worse.

## 5. The resize/move guard has to be rewritten first

This is the part that breaks, and it breaks in exactly the way the existing
comment warns about, so do it before anything above ships.

`apply_state` in `src/lib.rs` picks between "size then move" and "move then
size" from one bool:

```rust
let shrinking =
  size_after.width <= size_before.width && size_after.height <= size_before.height;
```

The guarantee it is buying is that the window never occupies a rectangle outside
the work area -- moving a transparent always-on-top window off the screen edge
is what took the process down, per the comment above it.

A rotation shrinks one axis and grows the other, so `shrinking` is `false` and
the window moves first. 260x90 moved to the `right` slot's x
(`area_right - 90 - margin`) hangs **170px off the right edge**. With `fill` on
it is worse: 260x90 to 90x1400 grows the height fifteen-fold.

The fix generalises both existing branches into one three-step sequence, taking
the per-axis minimum as an intermediate:

```rust
let waypoint = PhysicalSize::new(
  size_before.width.min(size_after.width),
  size_before.height.min(size_after.height),
);

window.set_size(waypoint)?;    // no larger than either rectangle, on either axis
window.set_position(target)?;  // target is computed for size_after
window.set_size(size_after)?;
```

Every intermediate rectangle is inside the work area:

- Shrinking about the window's top-left from any in-area rectangle stays
  in-area, whichever anchor it was parked at -- the rectangle only ever moves
  away from the edges it was against.
- `waypoint` is no larger than `size_after` on either axis, and `target` is the
  slot position for `size_after`, so `waypoint` at `target` fits inside a
  rectangle that itself fits.
- The final grow lands exactly on the slot.

Cost is one extra `set_size` and possibly a one-frame flicker on rotation.
Worth it, and it deletes the two-branch logic rather than adding to it.

## 6. The plugin has to start tracking state

`snap_to_nearest_anchor` reads the window's size off the window
(`window.outer_size()`) and only ever moves it. Once a slot decides the size,
the snap has to know which state the window is in to look up `size`,
`verticalSize` and `fill` -- and the README's own limitations list says the
plugin does not track that.

It has to now. The smallest change: `Shared.reporters` is already a
`HashMap<String, Reporter>` keyed by label and created on demand, so widen the
value into a per-window record:

```rust
struct Tracked {
  reporter: Reporter,
  state: Mutex<Option<String>>,   // set by set_state and by the initial placement
}
```

**Fallback:** no state recorded -- no `initialState` and the webview has not
called `set_state` -- means falling back to today's behaviour: move, do not
resize. Safe, and it keeps a config that names no states working.

Side benefit: that README limitation goes away, and `set_state` after a dev
reload stops being the only way to recover.

## 7. Snap and apply_state become one code path

`snap_to_nearest_anchor` (move only) and `apply_state` (size and move) currently
duplicate the monitor-picking, anchor-resolving and parked-check logic. Once the
snap resizes too, they are the same operation with a different way of choosing
the anchor. Extract:

```rust
fn place<R: Runtime>(
  window: &Window<R>, monitor: &Monitor, config: &Config,
  state: Option<&WindowState>, anchor: Anchor, reporter: &Reporter,
) -> Result<(), String>
```

- `apply_state` resolves the anchor from the override, then the state's own
  `anchor`, then nearest -- and calls `place`.
- the snap resolves nearest and calls `place`.

Resizing from the watcher thread is safe for the same reason `set_state` spawns
one: it is not the main thread, so `set_position`/`set_size` are posted to the
event loop rather than re-entering it mid-dispatch. See the long comment in
`src/commands.rs`.

### Two loop guards need widening

Both live in `snap_to_nearest_anchor` and both are currently position-only.

1. **The parked check** (`if position == target`) is what stops a snap from
   raising a move event that triggers another snap. A resize of a
   bottom-or-right-anchored window also moves it, so the check has to compare
   the whole rectangle -- position *and* size. Position-only, a state whose size
   is wrong but whose position is right would resize on every tick forever.
2. **`last_attempt`** is a `(position, target)` pair. A resize-only change --
   same position, different size -- looks identical to a move that did not take,
   so it would trip the "window would not move" backstop and give up. Widen it
   to a `(rect_before, rect_after)` pair.

## 8. What the webview is told

The reporter and `corner-snap://anchor` already carry the anchor, so most of
this is built. Two additions.

The webview cannot derive orientation from the anchor alone, because it does not
know whether *this* state has a `verticalSize`. So put the derived facts in the
payload:

*Built:* orientation is measured from the size the window ends up with -- taller
than wide is vertical -- rather than looked up from which configured shape is in
use. One rule, no config lookup at report time, always an answer (including for
a window with no tracked state, which the lookup could not have served), and it
matches exactly what the webview has to lay out for.

```rust
pub struct AnchorChanged {
  pub label: String,
  pub anchor: Anchor,
  pub orientation: Orientation,   // "horizontal" | "vertical"
}
```

Additive, so existing listeners are unaffected. The test in `src/report.rs` that
pins the exact serialized JSON will fail, which is the point of it -- it is the
wire-format guard.

`Reporter`'s de-duplication key becomes the `(anchor, orientation)` pair.
Orientation can change while the anchor does not: expanded-to-collapsed at the
`right` slot keeps the slot but a 44x44 badge is not the same shape as a vertical
bar.

`current_anchor` should return the same fuller record rather than a bare
`Option<Anchor>`. That is a breaking change to the command's return type; at
v0.1.0 with two known consumers, take it.

### The frontend-only shortcut

A widget that only needs to relayout, and does not care *which* side it is on,
can skip all of this and read `window.innerWidth < window.innerHeight`, or a
CSS `@media (orientation: portrait)`. Worth knowing, but the event is what you
want for anything directional -- which side a caret points, which way a menu
opens.

## Config, all together

```json
{
  "plugins": {
    "corner-snap": {
      "snapTo": ["topLeft", "topRight", "bottomLeft", "bottomRight", "left", "right"],
      "states": {
        "expanded": {
          "size": [260, 90],
          "verticalSize": [90, 320],
          "fill": true
        },
        "collapsed": {
          "size": [44, 44],
          "snapTo": ["topLeft", "top", "topRight", "left", "right",
                     "bottomLeft", "bottom", "bottomRight"]
        }
      },
      "initialState": "expanded",
      "initialAnchor": "bottomRight",
      "edgeMargin": 0,
      "manage": { "labels": ["main"] }
    }
  }
}
```

| Key | Where | Default | What it does |
| --- | --- | --- | --- |
| `snapTo` | config | four corners | Slots a window may dock to. |
| `snapTo` | state | inherits | Override for one state. |
| `verticalSize` | state | absent | Shape used at `left`/`right`. Absent means the state never turns. |
| `fill` | state | `false` | `true`, or a fraction: stretch along the docked edge. Side slots only. |

## Build order

1. `Anchor` to nine slots, `anchor_position` per axis (step 1).
2. The three-step resize guard, on its own, with the four corners still the only
   slots (step 5). Verify on Windows before going further -- this is the step
   that can take the process down, and it is testable in isolation.
3. State tracking and the unified `place` path (steps 6 and 7).
4. Nearest-candidate picking and `snapTo` (step 2).
5. `verticalSize` and `fill` (steps 3 and 4).
6. Payload, permissions, README (step 8).

Roughly five hours of writing plus tests. The real time is on the Windows host:
WSLg reports no monitors, so none of the placement logic can be exercised in
WSL, and the `SetWindowPos` re-entrancy is only reproducible live.
