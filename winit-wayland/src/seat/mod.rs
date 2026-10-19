//! Seat handling.

use std::cell::Cell;
use std::sync::{Arc, Mutex};

use foldhash::HashMap;
use sctk::data_device_manager::data_device::DataDevice;
use sctk::reexports::client::backend::ObjectId;
use sctk::reexports::client::protocol::wl_seat::WlSeat;
use sctk::reexports::client::protocol::wl_touch::WlTouch;
use sctk::reexports::client::{Connection, Proxy, QueueHandle};
use sctk::reexports::protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_v1::ZwpRelativePointerV1;
use sctk::reexports::protocols::wp::text_input::zv3::client::zwp_text_input_v3::ZwpTextInputV3;
use sctk::seat::pointer::{PointerData, ThemeSpec, ThemedPointer};
use sctk::seat::{Capability as SeatCapability, SeatHandler, SeatState};
use tracing::warn;
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gesture_hold_v1::ZwpPointerGestureHoldV1;
use wayland_protocols::wp::pointer_gestures::zv1::client::zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_seat_v2::ZwpTabletSeatV2;
use winit_core::event::WindowEvent;
use winit_core::keyboard::ModifiersState;

use crate::WindowId;
use crate::event_loop::sink::EventSink;
use crate::state::WinitState;
use crate::window::WindowState;

mod keyboard;
mod pointer;
mod text_input;
mod touch;

use keyboard::{KeyboardData, KeyboardState};
pub use pointer::pointer_gesture::{PointerGestureData, PointerGesturesState};
pub use pointer::relative_pointer::RelativePointerState;
pub use pointer::{PointerConstraintsState, WinitPointerData, WinitPointerDataExt};
use text_input::TextInputData;
pub use text_input::{ClientState as TextInputClientState, TextInputState};
use touch::TouchPoint;

pub(crate) use crate::seat::text_input::ZwpTextInputV3Ext;

#[derive(Debug, Default)]
pub struct WinitSeatState {
    /// The pointer bound on the seat.
    pointer: Option<Arc<ThemedPointer<WinitPointerData>>>,

    /// The touch bound on the seat.
    touch: Option<WlTouch>,

    /// The mapping from touched points to the surfaces they're present.
    touch_map: HashMap<i32, TouchPoint>,

    /// Id of the first touch event.
    first_touch_id: Option<i32>,

    /// The text input bound on the seat.
    text_input: Option<Arc<ZwpTextInputV3>>,

    /// The tablet input bound on the seat.
    tablet: Option<Arc<ZwpTabletSeatV2>>,

    /// The relative pointer bound on the seat.
    relative_pointer: Option<ZwpRelativePointerV1>,

    /// The pinch pointer gesture bound on the seat.
    pointer_gesture_pinch: Option<ZwpPointerGesturePinchV1>,

    /// The hold pointer gesture bound on the seat.
    pointer_gesture_hold: Option<ZwpPointerGestureHoldV1>,

    /// The drag-and-drop state
    data_device: Option<DataDevice>,

    /// The keyboard bound on the seat.
    keyboard_state: Option<KeyboardState>,

    /// The current modifiers state on the seat.
    modifiers: ModifiersState,

    /// Whether `modifiers` reflects the latest `wl_keyboard.modifiers`.
    ///
    /// Cleared when the keyboard focus is lost, after which the compositor sends modifiers only if
    /// it ties them to the pointer focus (wayland >= 1.23).
    modifiers_known: bool,

    /// Serial of the latest `wl_keyboard.modifiers`.
    modifiers_serial: u32,

    /// Whether we have pending modifiers.
    modifiers_pending: bool,

    /// Unfocused window which last received the modifiers for pointer input, reset once it stops
    /// receiving pointer input.
    ///
    /// Written from the pointer handler, which only has a shared reference, thus `Cell`.
    pointer_modifiers_window: Cell<Option<WindowId>>,

    /// Serial of the most recent input event (keyboard or pointer) on this seat.
    /// Written by both keyboard and pointer handlers through a shared reference,
    /// so interior mutability is required.
    pub(crate) latest_input_serial: Cell<Option<u32>>,
}

impl WinitSeatState {
    pub fn new() -> Self {
        Default::default()
    }

    /// Returns the serial of the most recent input event on this seat, or `None` if none has
    /// been received yet.
    pub fn latest_serial(&self) -> Option<u32> {
        self.latest_input_serial.get()
    }

    pub(crate) fn data_device(&self) -> Option<&DataDevice> {
        self.data_device.as_ref()
    }

    pub(crate) fn pointer_data(&self) -> Option<&PointerData<WinitPointerData>> {
        self.pointer.as_ref().and_then(|pointer| pointer.pointer().data())
    }

    /// The window with keyboard focus on this seat.
    fn keyboard_focus(&self) -> Option<WindowId> {
        self.keyboard_state.as_ref()?.keyboard.data::<KeyboardData>()?.window_id()
    }

    /// Reports the modifiers to the window under the pointer if it lacks keyboard focus.
    ///
    /// Both the `wl_keyboard.modifiers` and the `wl_pointer.enter` handlers call this, since either
    /// may arrive first: sctk delivers pointer events on `wl_pointer.frame`, but keyboard events
    /// immediately. The latter passes the serial of the `wl_pointer.enter`.
    pub(crate) fn send_pointer_modifiers(
        &self,
        windows: &HashMap<WindowId, Arc<Mutex<WindowState>>>,
        events_sink: &mut EventSink,
        enter_serial: Option<u32>,
    ) {
        if !self.modifiers_known {
            return;
        }

        // Without keyboard focus, the compositor updates the modifiers only for the pointer focus,
        // so modifiers from before the `wl_pointer.enter` are stale; the compositor sends current
        // ones after it. Compositors draw both serials from a single counter, which orders them.
        if let Some(enter_serial) = enter_serial {
            let sent_after_enter = self.modifiers_serial.wrapping_sub(enter_serial) as i32 > 0;
            if self.keyboard_focus().is_none() && !sent_after_enter {
                return;
            }
        }

        let window_id = match self
            .pointer_data()
            .and_then(|pointer| pointer.data().focused_window())
            .filter(|window_id| Some(*window_id) != self.keyboard_focus())
        {
            Some(window_id) if windows.contains_key(&window_id) => window_id,
            _ => return,
        };

        events_sink
            .push_window_event(WindowEvent::ModifiersChanged(self.modifiers.into()), window_id);
        self.pointer_modifiers_window.set(Some(window_id));
    }

    /// Resets the modifiers of the window which received them for pointer input, unless it has
    /// gained keyboard focus, which then supplies them.
    pub(crate) fn clear_pointer_modifiers(
        &self,
        windows: &HashMap<WindowId, Arc<Mutex<WindowState>>>,
        events_sink: &mut EventSink,
    ) {
        let window_id = match self.pointer_modifiers_window.take() {
            Some(window_id) if windows.contains_key(&window_id) => window_id,
            _ => return,
        };

        if self.keyboard_focus() != Some(window_id) {
            events_sink.push_window_event(
                WindowEvent::ModifiersChanged(ModifiersState::empty().into()),
                window_id,
            );
        }
    }

    /// Forgets a destroyed window which received the modifiers for pointer input, so that a new
    /// window reusing its ID is not mistaken for it.
    pub(crate) fn forget_window(&self, window_id: WindowId) {
        if self.pointer_modifiers_window.get() == Some(window_id) {
            self.pointer_modifiers_window.set(None);
        }
    }
}

impl SeatHandler for WinitState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_capability(
        &mut self,
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
        seat: WlSeat,
        capability: SeatCapability,
    ) {
        let seat_state = match self.seats.get_mut(&seat.id()) {
            Some(seat_state) => seat_state,
            None => {
                warn!("Received wl_seat::new_capability for unknown seat");
                return;
            },
        };

        match capability {
            SeatCapability::Touch if seat_state.touch.is_none() => {
                seat_state.touch = self.seat_state.get_touch(queue_handle, &seat).ok();
            },
            SeatCapability::Keyboard if seat_state.keyboard_state.is_none() => {
                let keyboard = seat.get_keyboard(queue_handle, KeyboardData::new(seat.clone()));
                seat_state.keyboard_state =
                    Some(KeyboardState::new(keyboard, self.loop_handle.clone()));
            },
            SeatCapability::Pointer if seat_state.pointer.is_none() => {
                let surface = self.compositor_state.create_surface(queue_handle);
                let viewport = self
                    .viewporter_state
                    .as_ref()
                    .map(|state| state.get_viewport(&surface, queue_handle));
                let surface_id = surface.id();
                let pointer_data = WinitPointerData::new(viewport);
                let themed_pointer = self
                    .seat_state
                    .get_pointer_with_theme_and_data(
                        queue_handle,
                        &seat,
                        self.shm.wl_shm(),
                        surface,
                        ThemeSpec::System,
                        pointer_data,
                    )
                    .expect("failed to create pointer with present capability.");

                seat_state.data_device = self
                    .data_device_manager_state
                    .as_ref()
                    .map(|device| device.get_data_device(queue_handle, &seat));

                seat_state.relative_pointer = self.relative_pointer.as_ref().map(|manager| {
                    manager.get_relative_pointer(
                        themed_pointer.pointer(),
                        queue_handle,
                        sctk::globals::GlobalData,
                    )
                });

                seat_state.pointer_gesture_pinch = self.pointer_gestures.as_ref().map(|manager| {
                    manager.get_pinch_gesture(
                        themed_pointer.pointer(),
                        queue_handle,
                        PointerGestureData::default(),
                    )
                });

                seat_state.pointer_gesture_hold = self.pointer_gestures.as_ref().map(|manager| {
                    manager.get_hold_gesture(
                        themed_pointer.pointer(),
                        queue_handle,
                        PointerGestureData::default(),
                    )
                });

                let themed_pointer = Arc::new(themed_pointer);

                // Register cursor surface.
                self.pointer_surfaces.insert(surface_id, themed_pointer.clone());

                seat_state.pointer = Some(themed_pointer);
            },
            _ => (),
        }

        if let Some(text_input_state) =
            seat_state.text_input.is_none().then_some(self.text_input_state.as_ref()).flatten()
        {
            seat_state.text_input = Some(Arc::new(text_input_state.get_text_input(
                &seat,
                queue_handle,
                TextInputData::default(),
            )));
        }

        if let Some(tablet_state) =
            seat_state.tablet.is_none().then_some(self.tablet_state.as_ref()).flatten()
        {
            seat_state.tablet = Some(Arc::new(tablet_state.get_tablet_seat(&seat, queue_handle)));
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _queue_handle: &QueueHandle<Self>,
        seat: WlSeat,
        capability: SeatCapability,
    ) {
        let seat_state = match self.seats.get_mut(&seat.id()) {
            Some(seat_state) => seat_state,
            None => {
                warn!("Received wl_seat::remove_capability for unknown seat");
                return;
            },
        };

        // NOTE: figure out when this should actually be destroyed.
        if let Some(tablet) = seat_state.tablet.take() {
            tablet.destroy();
        }

        match capability {
            SeatCapability::Touch => {
                if let Some(touch) = seat_state.touch.take() {
                    if touch.version() >= 3 {
                        touch.release();
                    }
                }
            },
            SeatCapability::Pointer => {
                // Without a pointer, no window receives modifiers for pointer input.
                seat_state.clear_pointer_modifiers(self.windows.get_mut(), &mut self.events_sink);

                if let Some(relative_pointer) = seat_state.relative_pointer.take() {
                    relative_pointer.destroy();
                }

                if let Some(pointer_gesture_pinch) = seat_state.pointer_gesture_pinch.take() {
                    pointer_gesture_pinch.destroy();
                }

                if let Some(pointer_gesture_hold) = seat_state.pointer_gesture_hold.take() {
                    pointer_gesture_hold.destroy();
                }

                seat_state.data_device = None;

                if let Some(pointer) = seat_state.pointer.take() {
                    let pointer_data = pointer.pointer().winit_data();

                    // Remove the cursor from the mapping.
                    let surface_id = pointer.surface().id();
                    let _ = self.pointer_surfaces.remove(&surface_id);

                    // Remove the inner locks/confines before dropping the pointer.
                    pointer_data.data().unlock_pointer();
                    pointer_data.data().unconfine_pointer();

                    if pointer.pointer().version() >= 3 {
                        pointer.pointer().release();
                    }
                }
            },
            SeatCapability::Keyboard => {
                // Without a keyboard, the modifiers are unknown.
                seat_state.modifiers_known = false;
                seat_state.clear_pointer_modifiers(self.windows.get_mut(), &mut self.events_sink);
                seat_state.keyboard_state = None;
                self.on_keyboard_destroy(&seat.id());
            },
            _ => (),
        }
    }

    fn new_seat(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        seat: WlSeat,
    ) {
        self.seats.insert(seat.id(), WinitSeatState::new());
    }

    fn remove_seat(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        seat: WlSeat,
    ) {
        if let Some(mut seat_state) = self.seats.remove(&seat.id()) {
            if let Some(text_input) = seat_state.text_input.take() {
                text_input.destroy();
            }
            seat_state.clear_pointer_modifiers(self.windows.get_mut(), &mut self.events_sink);
        }
        self.on_keyboard_destroy(&seat.id());
    }
}

impl WinitState {
    fn on_keyboard_destroy(&mut self, seat: &ObjectId) {
        for (window_id, window) in self.windows.get_mut() {
            let mut window = window.lock().unwrap();
            let had_focus = window.has_focus();
            window.remove_seat_focus(seat);
            if had_focus != window.has_focus() {
                self.events_sink.push_window_event(WindowEvent::Focused(false), *window_id);
            }
        }
    }
}
