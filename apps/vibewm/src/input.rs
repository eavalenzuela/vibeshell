//! Forward winit input events into smithay's seat (keyboard / pointer).
//!
//! Lifted from smithay's smallvil example with light edits — touch, axis-v120
//! quirks, and pointer-relative motion handling stay basic for W1b.

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, ButtonState, Event, GestureBeginEvent, GestureEndEvent,
    GesturePinchUpdateEvent, GestureSwipeUpdateEvent, InputBackend, InputEvent, KeyboardKeyEvent,
    PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent as SmithayHoldBegin,
    GestureHoldEndEvent as SmithayHoldEnd, GesturePinchBeginEvent as SmithayPinchBegin,
    GesturePinchEndEvent as SmithayPinchEnd, GesturePinchUpdateEvent as SmithayPinchUpdate,
    GestureSwipeBeginEvent as SmithaySwipeBegin, GestureSwipeEndEvent as SmithaySwipeEnd,
    GestureSwipeUpdateEvent as SmithaySwipeUpdate, MotionEvent,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::SERIAL_COUNTER;

use crate::state::Vibewm;

impl Vibewm {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);
                let key_state = event.state();
                if let Some(keyboard) = self.seat.get_keyboard() {
                    keyboard.input::<(), _>(
                        self,
                        event.key_code(),
                        key_state,
                        serial,
                        time,
                        |_data, modifiers, keysym_handle| {
                            // Only intercept on key press, not release —
                            // bindings fire once per Mod+key chord.
                            if key_state == smithay::backend::input::KeyState::Pressed
                                && crate::keybindings::try_dispatch(
                                    modifiers,
                                    keysym_handle.modified_sym(),
                                )
                            {
                                FilterResult::Intercept(())
                            } else {
                                FilterResult::Forward
                            }
                        },
                    );
                }
            }
            InputEvent::PointerMotion { event, .. } => {
                // libinput's relative motion. Move the cursor by the delta,
                // clamped to the first output's geometry.
                let Some(output) = self.space.outputs().next().cloned() else {
                    return;
                };
                let Some(output_geo) = self.space.output_geometry(&output) else {
                    return;
                };
                let pointer = self.seat.get_pointer().expect("seat has no pointer");
                let delta = event.delta();
                let mut pos = pointer.current_location() + delta;
                pos.x = pos.x.clamp(
                    output_geo.loc.x as f64,
                    (output_geo.loc.x + output_geo.size.w) as f64,
                );
                pos.y = pos.y.clamp(
                    output_geo.loc.y as f64,
                    (output_geo.loc.y + output_geo.size.h) as f64,
                );
                let serial = SERIAL_COUNTER.next_serial();
                let under = self.surface_under(pos);
                pointer.motion(
                    self,
                    under,
                    &MotionEvent {
                        location: pos,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
                #[cfg(feature = "udev")]
                crate::udev::schedule_render(self);
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let Some(output) = self.space.outputs().next().cloned() else {
                    return;
                };
                let Some(output_geo) = self.space.output_geometry(&output) else {
                    return;
                };
                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
                let serial = SERIAL_COUNTER.next_serial();
                let pointer = self.seat.get_pointer().expect("seat has no pointer");
                let under = self.surface_under(pos);
                pointer.motion(
                    self,
                    under,
                    &MotionEvent {
                        location: pos,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
                #[cfg(feature = "udev")]
                crate::udev::schedule_render(self);
            }
            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().expect("seat has no pointer");
                let keyboard = self.seat.get_keyboard().expect("seat has no keyboard");
                let serial = SERIAL_COUNTER.next_serial();
                let button = event.button_code();
                let button_state = event.state();

                if button_state == ButtonState::Pressed && !pointer.is_grabbed() {
                    if let Some((window, _loc)) = self
                        .space
                        .element_under(pointer.current_location())
                        .map(|(w, l)| (w.clone(), l))
                    {
                        self.space.raise_element(&window, true);
                        if let Some(toplevel) = window.toplevel() {
                            keyboard.set_focus(self, Some(toplevel.wl_surface().clone()), serial);
                        }
                    } else {
                        keyboard.set_focus(self, Option::<WlSurface>::None, serial);
                    }
                }

                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();
                let h = event.amount(Axis::Horizontal).unwrap_or_else(|| {
                    event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.0
                });
                let v = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.0
                });
                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                if h != 0.0 {
                    frame = frame.value(Axis::Horizontal, h);
                }
                if v != 0.0 {
                    frame = frame.value(Axis::Vertical, v);
                }
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.axis(self, frame);
                    pointer.frame(self);
                }
            }
            // Touchpad gestures (libinput-only; winit doesn't emit these).
            // Two consumers per event:
            //   1. `state.gestures` accumulates and decides on End whether
            //      to fire a compositor action (cluster cycle, zoom-mode).
            //   2. The wp-pointer-gestures-v1 protocol forwards the event
            //      to the focused client (browser pinch-zoom etc.).
            // Both run unconditionally — overlap is acceptable in practice
            // (user intent for cluster-cycling is over the desktop, not
            // over a content area where a client also handles gestures).
            InputEvent::GestureSwipeBegin { event } => {
                self.gestures.on_swipe_begin(event.fingers());
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.gesture_swipe_begin(
                        self,
                        &SmithaySwipeBegin {
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time_msec(),
                            fingers: event.fingers(),
                        },
                    );
                }
            }
            InputEvent::GestureSwipeUpdate { event } => {
                self.gestures
                    .on_swipe_update(event.delta_x(), event.delta_y());
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.gesture_swipe_update(
                        self,
                        &SmithaySwipeUpdate {
                            time: event.time_msec(),
                            delta: (event.delta_x(), event.delta_y()).into(),
                        },
                    );
                }
            }
            InputEvent::GestureSwipeEnd { event } => {
                let cancelled = event.cancelled();
                if let Some(action) = self.gestures.on_swipe_end(cancelled) {
                    action.dispatch();
                }
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.gesture_swipe_end(
                        self,
                        &SmithaySwipeEnd {
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time_msec(),
                            cancelled,
                        },
                    );
                }
            }
            InputEvent::GesturePinchBegin { event } => {
                self.gestures.on_pinch_begin();
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.gesture_pinch_begin(
                        self,
                        &SmithayPinchBegin {
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time_msec(),
                            fingers: event.fingers(),
                        },
                    );
                }
            }
            InputEvent::GesturePinchUpdate { event } => {
                self.gestures.on_pinch_update(event.scale());
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.gesture_pinch_update(
                        self,
                        &SmithayPinchUpdate {
                            time: event.time_msec(),
                            delta: (event.delta_x(), event.delta_y()).into(),
                            scale: event.scale(),
                            rotation: event.rotation(),
                        },
                    );
                }
            }
            InputEvent::GesturePinchEnd { event } => {
                let cancelled = event.cancelled();
                if let Some(action) = self.gestures.on_pinch_end(cancelled) {
                    action.dispatch();
                }
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.gesture_pinch_end(
                        self,
                        &SmithayPinchEnd {
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time_msec(),
                            cancelled,
                        },
                    );
                }
            }
            // Hold gestures: forward to client only — no compositor binding
            // today (reserve for future "show overview" or similar). Smithay
            // routes to the focused client via wp-pointer-gestures-v1.
            InputEvent::GestureHoldBegin { event } => {
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.gesture_hold_begin(
                        self,
                        &SmithayHoldBegin {
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time_msec(),
                            fingers: event.fingers(),
                        },
                    );
                }
            }
            InputEvent::GestureHoldEnd { event } => {
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.gesture_hold_end(
                        self,
                        &SmithayHoldEnd {
                            serial: SERIAL_COUNTER.next_serial(),
                            time: event.time_msec(),
                            cancelled: event.cancelled(),
                        },
                    );
                }
            }
            _ => {}
        }
    }
}
