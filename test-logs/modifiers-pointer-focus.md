# Test logs: modifiers for unfocused windows receiving pointer input

The logs in `modifiers-pointer-focus/` record how stock winit and winit with the change "Report
modifiers to unfocused windows receiving pointer input", which follows this commit, report keyboard
modifiers to a window which receives pointer input without keyboard focus, under three X11 window
managers and three Wayland compositors. Each test was performed once with each variant, by hand,
following the same written steps.

## Environment

All runs took place on 2026-10-19 on one machine:

| Component | Version |
|---|---|
| Distribution | openSUSE Tumbleweed 20260929 |
| Kernel | 7.2.8-1-default |
| GPU driver | NVIDIA open kernel module 580.178.04 |
| libwayland | 1.26.0 |
| X.Org server | 21.1.24 |
| Xwayland | 24.1.13 |
| xterm | 410 |

| Directory | Backend | Window manager or compositor | Focus configuration |
|---|---|---|---|
| `i3` | X11 | i3 4.25.1 | `focus_follows_mouse no` |
| `openbox` | X11 | openbox 3.6.1 | `<followMouse>no</followMouse>` |
| `kwin_x11` | X11 | KWin 6.7.5 (X11) | `FocusPolicy` unset, which defaults to `ClickToFocus` |
| `kwin` | Wayland | KWin 6.7.5 | `FocusPolicy` unset, which defaults to `ClickToFocus` |
| `sway` | Wayland | sway 1.12 | `focus_follows_mouse no`, `mouse_warping none` |
| `mutter` | Wayland | mutter 50.5 | default, click to focus |

Every window manager focuses windows on click only, so hovering never moves keyboard focus during
a test. Each X11 run used a new X server on display `:1`, started by `xinit` from a text console
with only the window manager. Each Wayland run started the bare compositor from a text console:
`kwin_wayland --drm`, `sway` with software rendering (`WLR_RENDERER=pixman`), and
`mutter --display-server`.

## Clients

Two clients run side by side in each session:

- **The probe window**: a minimal winit application which opens one window and logs every
  `WindowEvent` and `DeviceEvent` winit delivers to it, with timestamps. It listens to device
  events always (`DeviceEvents::Always`), so key presses appear in the X11 logs even while the
  window is unfocused. It does not log pointer motion. On X11, each focus, modifier and pointer
  event is followed by the X server's state, queried when the probe logs the event. On Wayland,
  `WAYLAND_DEBUG=client` adds the raw protocol messages the client receives.
- **The guide**: an xterm belonging to another client, which shows the steps of each test and
  records the markers in the log. It holds keyboard focus at the start of every test.

Both variants build the same probe source:

| Variant | winit | Files |
|---|---|---|
| stock | master at 758a50e35, unmodified | `<test>-stock.log` |
| patched | the same master with the change | `<test>-patched.log` |

The client line of the patched headers names the commit the probe was built from. For i3, openbox,
kwin_x11, sway and mutter, this is f20661474, an earlier revision of the change. It differs from
the final change only in the Wayland handling of stored modifiers at `wl_pointer.enter` while no
window of the application has keyboard focus. That code path requires a compositor which sends
`wl_keyboard.modifiers` without keyboard focus, which none of these five environments does: the X11
backend does not include it, and sway and mutter send no modifiers to an unfocused client. The KWin
run, which exercises that path, used the final code; its header reads `f20661474-dirty` because the
probe was built from the uncommitted working tree which then became the change.

## Procedure

The operator places the two windows side by side, with the probe window unfocused. Each test
starts with the xterm focused and the pointer over the xterm. The guide then lists the steps of
the test; the operator performs them, clicks the xterm if it is not focused, and presses Enter,
which ends the test. When a step goes wrong, the operator requests a redo, and the guide records a
new attempt; only the last attempt of each test is included here. The steps below are quoted as the
guide showed them.

## Tests

### T1: plain click into the unfocused window

> Click in the probe window and drag, Release the mouse

A control: it shows how each environment focuses a window on click, and in which order the focus
change and the button press reach the client.

### T2: Ctrl+click into the unfocused window

> Hold Ctrl, Click in the probe window and drag, Release the mouse, Release Ctrl

The case motivating the change, as in a terminal starting a block selection on Ctrl+click. Ctrl
must be known to the window when the button press arrives, regardless of whether the window
manager focuses the window before the press, after it (i3), or never (openbox).

### T3: Ctrl+click into the focused window

> Click the probe window once to focus it, Hold Ctrl, Click in the probe window and drag, Release
> the mouse, Release Ctrl

A regression check: modifiers of a focused window come from the keyboard, as before the change.

### T4: Ctrl pressed and released while hovering, without pointer motion

> Move the pointer over the probe window (don't click), Keep the pointer still until the last step,
> Hold Ctrl for a second, Release Ctrl, Wait a second, Move the pointer off the probe window

Checks that modifier changes reach the hovered unfocused window from keyboard events alone, without
a pointer event carrying them: XKB state notifications on X11, and `wl_keyboard.modifiers` sent
without keyboard focus on Wayland.

### T5: Ctrl+scroll over the unfocused window

> Move the pointer over the probe window (don't click), Hold Ctrl, Scroll a few notches, Release
> Ctrl, Move the pointer off the probe window

Ctrl+scroll, commonly bound to zooming, is the use case the Wayland protocol names for sending
modifiers without keyboard focus. No window manager or compositor focuses the window on scrolling,
so Ctrl must reach the unfocused window before the wheel events.

### T6: Ctrl+Back button over the unfocused window

> Move the pointer over the probe window (don't click), Hold Ctrl, Press the mouse Back button,
> Release Ctrl, Move the pointer off the probe window

A button which some environments do not use for focusing (the X11 window managers), and others do
(the Wayland compositors), so Ctrl must be known at the press in both situations.

### T7: Ctrl held before the pointer enters

> Keep the pointer over this xterm, Hold Ctrl, Move the pointer over the probe window (don't
> click), Wait a second, Release Ctrl, Move the pointer off the probe window

Checks the modifier state reported when the pointer enters while Ctrl is already held, and its
update when Ctrl is released over the window.

### T8: leave and re-enter with Ctrl held

> Move the pointer over the probe window (don't click), Hold Ctrl, Move the pointer onto this xterm,
> Move the pointer back over the probe window, Release Ctrl, Move the pointer off the probe window

Checks that the window is reset when the pointer leaves, and receives Ctrl again on re-entering.

### T9: Ctrl released while the pointer is elsewhere

> Move the pointer over the probe window (don't click), Hold Ctrl, Move the pointer onto this xterm,
> Release Ctrl, Wait a second, Move the pointer back over the probe window (don't click), Move the
> pointer off the probe window

Checks for stale state: Ctrl is released while another client has the pointer, so the window must
not report Ctrl after the pointer leaves, nor when it re-enters.

### T10: Ctrl+click, then focus moves away with Ctrl held

> Hold Ctrl, Click the probe window once (no drag), Click this xterm, Release Ctrl

Checks that the window receives Ctrl at the press and is reset when it loses keyboard focus while
Ctrl is still held.

### T11: Ctrl+drag out of the unfocused window

> Hold Ctrl, Press the mouse button in the probe window and keep it held, Drag the pointer onto this
> xterm, Release the mouse there, Wait a second, Release Ctrl

Checks the end of an implicit pointer grab outside the window: the window keeps receiving pointer
input while the button is held, and an unfocused window is reset once the button is released
outside it.

## Log format

Each file holds one test of one run:

1. The header of the run: environment, library and driver versions, the client and the winit
   commit it was built from, the window manager or compositor, and the test list version.
2. A `# source:` line naming the full log of the run, which also contains the setup before the
   first test, the end of the session, and discarded attempts.
3. The test, copied verbatim from `# HH:MM:SS.mmm TEST Tn BEGIN: <steps>` to `TEST Tn END`.
   A redone test reads `TEST Tn (attempt n) BEGIN`.

Within a test:

- Probe lines read `[HH:MM:SS.micros +ms] <event>`, with the time since the probe started. Events
  are winit's `WindowEvent`s in `Debug` form, and `device ...` lines for `DeviceEvent`s.
- On X11, focus, modifier and pointer events end with `| x11 mask=<mask> focus=<window>`: the
  modifier and button mask from `QueryPointer`, and the input focus from `GetInputFocus`, either
  `self` or `other(<window id>)`. They reflect the server state when the probe logs the event,
  which may differ from the state when the event was generated.
- On Wayland, lines from libwayland read `[HH:MM:SS.micros] <object>#<id>.<event>(<arguments>)`.
  Their timestamps are UTC, while the markers and probe lines use local time (UTC-4). In
  `wl_keyboard.modifiers(serial, depressed, latched, locked, group)`, a depressed value of 4 is
  Control. In `wl_pointer.button`, button 272 is the left button and 275 the Back button; on X11,
  Back is button 8.

## Limitations

- On sway and mutter, the compositor sends neither key events nor modifiers to an unfocused client,
  so the logs of the hover tests (T4, T5, T7, T8, T9) cannot show that Ctrl was held.
- The probe does not log pointer motion, so T4's requirement to keep the pointer still can be
  verified only on Wayland, from the absence of `wl_pointer.motion`.
- Each test was performed by hand, once per variant; timings and pointer paths differ between the
  stock and patched runs of the same test.
