//! horizon-ios: a hardware keyboard, and a mouse or trackpad.
//!
//! Keys come as `UIPress`es to the first responder: the view while the game allows an input
//! method (`set_ime_allowed`), its view controller otherwise. UIKit repeats no key, so a held key
//! repeats here, as a desktop's would. The pointer's place comes as UIKit's indirect touches and
//! hovering, and its scrolling as a scroll gesture, while it is free; while it is locked
//! (`set_cursor_grab`) UIKit sends none of that, and GameController's `GCMouse` gives the wheel.
//! `GCMouse` gives the mouse's own motion as `DeviceEvent::MouseMotion`, and its buttons (an
//! iPhone, which locks no pointer, has them only there); with no `GCMouse` (a trackpad UIKit has
//! but GameController hasn't), the indirect touches do.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send, msg_send_id, ClassType};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSNotificationCenter, NSObject, NSSet, NSString,
};
use objc2_ui_kit::{UIKeyModifierFlags, UIKeyboardHIDUsage, UIPress};

use super::app_state::{self, EventWrapper};
use crate::dpi::PhysicalPosition;
use crate::event::{
    DeviceEvent, ElementState, Event, KeyEvent, Modifiers, MouseButton, MouseScrollDelta,
    TouchPhase, WindowEvent,
};
use crate::keyboard::{Key, KeyCode, KeyLocation, ModifiersState, NamedKey, PhysicalKey};
use crate::platform_impl::platform::DEVICE_ID;
use crate::platform_impl::KeyEventExtra;
use crate::window::WindowId as RootWindowId;

/// A held key repeats after this long...
const REPEAT_DELAY: Duration = Duration::from_millis(500);
/// ...this often.
const REPEAT_INTERVAL: Duration = Duration::from_millis(50);

#[link(name = "GameController", kind = "framework")]
extern "C" {
    static GCMouseDidConnectNotification: &'static NSString;
}

#[derive(Default)]
struct State {
    window: Cell<Option<RootWindowId>>,
    /// The keys held down, as pressed.
    held: RefCell<Vec<KeyCode>>,
    modifiers: Cell<ModifiersState>,
    repeat: RefCell<Option<(KeyEvent, Instant)>>,
    timer: RefCell<Option<Retained<NSObject>>>,
    /// The buttons each indirect touch pressed.
    pointer: RefCell<HashMap<u64, MouseButton>>,
    locked: Cell<bool>,
    installed: Cell<bool>,
}

thread_local! {
    static STATE: State = State::default();
}

fn send(mtm: MainThreadMarker, events: impl IntoIterator<Item = WindowEvent>) {
    let Some(window_id) = STATE.with(|s| s.window.get()) else {
        return;
    };
    app_state::handle_nonuser_events(
        mtm,
        events
            .into_iter()
            .map(|event| EventWrapper::StaticEvent(Event::WindowEvent { window_id, event })),
    );
}

/// The window the keyboard and the mouse act on (iOS shows one).
pub(crate) fn set_window(mtm: MainThreadMarker, window_id: RootWindowId) {
    STATE.with(|s| s.window.set(Some(window_id)));
    install(mtm);
}

/// Whether the pointer is locked: UIKit then sends no pointer events, `GCMouse` does.
pub(crate) fn pointer_locked() -> bool {
    STATE.with(|s| s.locked.get())
}

// ---- Keyboard ----

/// `pressesBegan`/`Ended`/`Cancelled`: sends the presses that are keys, and gives back those that
/// aren't (a remote's buttons) for the responder to pass on.
pub(crate) fn presses(
    mtm: MainThreadMarker,
    presses: &NSSet<UIPress>,
    state: ElementState,
) -> Option<Retained<NSSet<UIPress>>> {
    let mut rest = Vec::new();
    let mut events = Vec::new();
    for press in presses {
        let Some(key) = (unsafe { press.key(mtm) }) else {
            rest.push(press.retain());
            continue;
        };
        let usage = unsafe { key.keyCode() };
        let Some(code) = key_code(usage) else {
            rest.push(press.retain());
            continue;
        };
        let flags = unsafe { key.modifierFlags() };
        let typing = !flags.intersects(
            UIKeyModifierFlags::UIKeyModifierCommand | UIKeyModifierFlags::UIKeyModifierControl,
        );
        let characters = unsafe { key.characters() }.to_string();
        let ignoring = unsafe { key.charactersIgnoringModifiers() }.to_string();
        let event = key_event(code, state, &characters, &ignoring, typing);
        let before = STATE.with(|s| s.modifiers.get());
        let after = STATE.with(|s| {
            let mut held = s.held.borrow_mut();
            held.retain(|&k| k != code);
            if state == ElementState::Pressed {
                held.push(code);
            }
            let mods = held
                .iter()
                .fold(ModifiersState::empty(), |m, &k| m | modifier(k));
            s.modifiers.set(mods);
            mods
        });
        if after != before {
            events.push(WindowEvent::ModifiersChanged(Modifiers::from(after)));
        }
        if state == ElementState::Pressed && modifier(code).is_empty() {
            start_repeat(event.clone());
        } else if state == ElementState::Released
            && STATE.with(|s| {
                s.repeat
                    .borrow()
                    .as_ref()
                    .is_some_and(|(e, _)| e.physical_key == event.physical_key)
            })
        {
            stop_repeat();
        }
        events.push(WindowEvent::KeyboardInput {
            device_id: DEVICE_ID,
            event,
            is_synthetic: false,
        });
    }
    send(mtm, events);
    (!rest.is_empty()).then(|| {
        let rest = NSArray::from_vec(rest);
        unsafe { msg_send_id![class!(NSSet), setWithArray: &*rest] }
    })
}

fn key_event(
    code: KeyCode,
    state: ElementState,
    characters: &str,
    ignoring: &str,
    typing: bool,
) -> KeyEvent {
    let location = location(code);
    let (logical_key, text) = match named(code) {
        Some(named) => {
            let text = match named {
                NamedKey::Enter => Some("\r"),
                NamedKey::Tab => Some("\t"),
                NamedKey::Space => Some(" "),
                _ => None,
            };
            (Key::Named(named), text.map(smol_str::SmolStr::new))
        }
        None => {
            // Special keys have names here ("UIKeyInputUpArrow"), never control characters.
            let printable = |s: &str| {
                !s.is_empty() && !s.starts_with("UIKeyInput") && !s.chars().any(char::is_control)
            };
            let logical = if printable(ignoring) {
                Key::Character(smol_str::SmolStr::new(ignoring))
            } else {
                Key::Unidentified(crate::keyboard::NativeKey::Unidentified)
            };
            let text =
                (typing && printable(characters)).then(|| smol_str::SmolStr::new(characters));
            (logical, text)
        }
    };
    KeyEvent {
        physical_key: PhysicalKey::Code(code),
        logical_key,
        text: if state == ElementState::Pressed {
            text
        } else {
            None
        },
        location,
        state,
        repeat: false,
        platform_specific: KeyEventExtra {},
    }
}

fn start_repeat(event: KeyEvent) {
    STATE.with(|s| *s.repeat.borrow_mut() = Some((event, Instant::now() + REPEAT_DELAY)));
    if STATE.with(|s| s.timer.borrow().is_some()) {
        return;
    }
    let block = RcBlock::new(|_timer: *mut AnyObject| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let now = Instant::now();
        let due = STATE.with(|s| {
            let mut repeat = s.repeat.borrow_mut();
            let (event, next) = repeat.as_mut()?;
            if now < *next {
                return None;
            }
            *next = now + REPEAT_INTERVAL;
            let mut event = event.clone();
            event.repeat = true;
            Some(event)
        });
        if let Some(event) = due {
            send(
                mtm,
                [WindowEvent::KeyboardInput {
                    device_id: DEVICE_ID,
                    event,
                    is_synthetic: false,
                }],
            );
        }
    });
    let interval = REPEAT_INTERVAL.as_secs_f64() / 2.0;
    let timer: Retained<NSObject> = unsafe {
        msg_send_id![
            class!(NSTimer),
            scheduledTimerWithTimeInterval: interval,
            repeats: true,
            block: &*block
        ]
    };
    STATE.with(|s| *s.timer.borrow_mut() = Some(timer));
}

fn stop_repeat() {
    STATE.with(|s| {
        s.repeat.borrow_mut().take();
        if let Some(timer) = s.timer.borrow_mut().take() {
            let _: () = unsafe { msg_send![&timer, invalidate] };
        }
    });
}

fn modifier(code: KeyCode) -> ModifiersState {
    match code {
        KeyCode::ShiftLeft | KeyCode::ShiftRight => ModifiersState::SHIFT,
        KeyCode::ControlLeft | KeyCode::ControlRight => ModifiersState::CONTROL,
        KeyCode::AltLeft | KeyCode::AltRight => ModifiersState::ALT,
        KeyCode::SuperLeft | KeyCode::SuperRight => ModifiersState::SUPER,
        _ => ModifiersState::empty(),
    }
}

fn location(code: KeyCode) -> KeyLocation {
    use KeyCode::*;
    match code {
        ShiftLeft | ControlLeft | AltLeft | SuperLeft => KeyLocation::Left,
        ShiftRight | ControlRight | AltRight | SuperRight => KeyLocation::Right,
        Numpad0 | Numpad1 | Numpad2 | Numpad3 | Numpad4 | Numpad5 | Numpad6 | Numpad7 | Numpad8
        | Numpad9 | NumpadAdd | NumpadSubtract | NumpadMultiply | NumpadDivide | NumpadDecimal
        | NumpadEnter | NumpadEqual | NumpadComma | NumLock => KeyLocation::Numpad,
        _ => KeyLocation::Standard,
    }
}

/// The keys that type nothing of their own (and Space, Enter and Tab), by name.
fn named(code: KeyCode) -> Option<NamedKey> {
    use KeyCode::*;
    Some(match code {
        Enter | NumpadEnter => NamedKey::Enter,
        Escape => NamedKey::Escape,
        Backspace => NamedKey::Backspace,
        Tab => NamedKey::Tab,
        Space => NamedKey::Space,
        CapsLock => NamedKey::CapsLock,
        F1 => NamedKey::F1,
        F2 => NamedKey::F2,
        F3 => NamedKey::F3,
        F4 => NamedKey::F4,
        F5 => NamedKey::F5,
        F6 => NamedKey::F6,
        F7 => NamedKey::F7,
        F8 => NamedKey::F8,
        F9 => NamedKey::F9,
        F10 => NamedKey::F10,
        F11 => NamedKey::F11,
        F12 => NamedKey::F12,
        F13 => NamedKey::F13,
        F14 => NamedKey::F14,
        F15 => NamedKey::F15,
        F16 => NamedKey::F16,
        F17 => NamedKey::F17,
        F18 => NamedKey::F18,
        F19 => NamedKey::F19,
        F20 => NamedKey::F20,
        F21 => NamedKey::F21,
        F22 => NamedKey::F22,
        F23 => NamedKey::F23,
        F24 => NamedKey::F24,
        PrintScreen => NamedKey::PrintScreen,
        ScrollLock => NamedKey::ScrollLock,
        Pause => NamedKey::Pause,
        Insert => NamedKey::Insert,
        Home => NamedKey::Home,
        PageUp => NamedKey::PageUp,
        Delete => NamedKey::Delete,
        End => NamedKey::End,
        PageDown => NamedKey::PageDown,
        ArrowRight => NamedKey::ArrowRight,
        ArrowLeft => NamedKey::ArrowLeft,
        ArrowDown => NamedKey::ArrowDown,
        ArrowUp => NamedKey::ArrowUp,
        NumLock => NamedKey::NumLock,
        ContextMenu => NamedKey::ContextMenu,
        ShiftLeft | ShiftRight => NamedKey::Shift,
        ControlLeft | ControlRight => NamedKey::Control,
        AltLeft | AltRight => NamedKey::Alt,
        SuperLeft | SuperRight => NamedKey::Super,
        _ => return None,
    })
}

/// The USB HID keyboard usage (page 7) `UIKey.keyCode` gives, as the key's physical place.
fn key_code(usage: UIKeyboardHIDUsage) -> Option<KeyCode> {
    use KeyCode::*;
    const LETTERS: [KeyCode; 26] = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO,
        KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    const DIGITS: [KeyCode; 10] = [
        Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9, Digit0,
    ];
    const FUNCTION: [KeyCode; 12] = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    const FUNCTION_HIGH: [KeyCode; 12] =
        [F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24];
    const NUMPAD: [KeyCode; 10] = [
        Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8, Numpad9, Numpad0,
    ];
    let u = usage.0 as usize;
    Some(match u {
        0x04..=0x1D => LETTERS[u - 0x04],
        0x1E..=0x27 => DIGITS[u - 0x1E],
        0x28 => Enter,
        0x29 => Escape,
        0x2A => Backspace,
        0x2B => Tab,
        0x2C => Space,
        0x2D => Minus,
        0x2E => Equal,
        0x2F => BracketLeft,
        0x30 => BracketRight,
        0x31 | 0x32 => Backslash,
        0x33 => Semicolon,
        0x34 => Quote,
        0x35 => Backquote,
        0x36 => Comma,
        0x37 => Period,
        0x38 => Slash,
        0x39 => CapsLock,
        0x3A..=0x45 => FUNCTION[u - 0x3A],
        0x46 => PrintScreen,
        0x47 => ScrollLock,
        0x48 => Pause,
        0x49 => Insert,
        0x4A => Home,
        0x4B => PageUp,
        0x4C => Delete,
        0x4D => End,
        0x4E => PageDown,
        0x4F => ArrowRight,
        0x50 => ArrowLeft,
        0x51 => ArrowDown,
        0x52 => ArrowUp,
        0x53 => NumLock,
        0x54 => NumpadDivide,
        0x55 => NumpadMultiply,
        0x56 => NumpadSubtract,
        0x57 => NumpadAdd,
        0x58 => NumpadEnter,
        0x59..=0x62 => NUMPAD[u - 0x59],
        0x63 => NumpadDecimal,
        0x64 => IntlBackslash,
        0x65 => ContextMenu,
        0x67 => NumpadEqual,
        0x68..=0x73 => FUNCTION_HIGH[u - 0x68],
        0x85 => NumpadComma,
        0x87 => IntlRo,
        0x88 => KanaMode,
        0x89 => IntlYen,
        0x8A => Convert,
        0x8B => NonConvert,
        0x90 => Lang1,
        0x91 => Lang2,
        0xE0 => ControlLeft,
        0xE1 => ShiftLeft,
        0xE2 => AltLeft,
        0xE3 => SuperLeft,
        0xE4 => ControlRight,
        0xE5 => ShiftRight,
        0xE6 => AltRight,
        0xE7 => SuperRight,
        _ => return None,
    })
}

// ---- Pointer ----

/// The `UIEvent.buttonMask` bits, from the primary button up.
fn button(mask: isize) -> MouseButton {
    match mask.trailing_zeros() {
        0 | 64 => MouseButton::Left,
        1 => MouseButton::Right,
        2 => MouseButton::Middle,
        3 => MouseButton::Back,
        4 => MouseButton::Forward,
        n => MouseButton::Other(n as u16),
    }
}

/// Whether GameController has a mouse, which then gives the buttons.
fn mouse_connected() -> bool {
    let mice: Retained<NSArray<AnyObject>> = unsafe { msg_send_id![class!(GCMouse), mice] };
    !mice.is_empty()
}

/// A touch of the pointer (`UITouchType::IndirectPointer`): the cursor, and a button when
/// GameController has no mouse to give it.
pub(crate) fn pointer_touch(
    mtm: MainThreadMarker,
    id: u64,
    phase: TouchPhase,
    position: PhysicalPosition<f64>,
    button_mask: isize,
) {
    let mut events = vec![WindowEvent::CursorMoved {
        device_id: DEVICE_ID,
        position,
    }];
    if !pointer_locked() && !mouse_connected() {
        match phase {
            TouchPhase::Started => {
                let button = button(button_mask);
                STATE.with(|s| s.pointer.borrow_mut().insert(id, button));
                events.push(mouse_input(ElementState::Pressed, button));
            }
            TouchPhase::Moved => {}
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if let Some(button) = STATE.with(|s| s.pointer.borrow_mut().remove(&id)) {
                    events.push(mouse_input(ElementState::Released, button));
                }
            }
        }
    }
    send(mtm, events);
}

/// The pointer over the view with no button down.
pub(crate) fn hover(mtm: MainThreadMarker, phase: TouchPhase, position: PhysicalPosition<f64>) {
    let moved = WindowEvent::CursorMoved {
        device_id: DEVICE_ID,
        position,
    };
    match phase {
        TouchPhase::Started => send(
            mtm,
            [
                WindowEvent::CursorEntered {
                    device_id: DEVICE_ID,
                },
                moved,
            ],
        ),
        TouchPhase::Moved => send(mtm, [moved]),
        TouchPhase::Ended | TouchPhase::Cancelled => send(
            mtm,
            [WindowEvent::CursorLeft {
                device_id: DEVICE_ID,
            }],
        ),
    }
}

/// A wheel or a two-finger trackpad scroll, in physical pixels.
pub(crate) fn scroll(mtm: MainThreadMarker, phase: TouchPhase, delta: PhysicalPosition<f64>) {
    send(
        mtm,
        [WindowEvent::MouseWheel {
            device_id: DEVICE_ID,
            delta: MouseScrollDelta::PixelDelta(delta),
            phase,
        }],
    );
}

fn mouse_input(state: ElementState, button: MouseButton) -> WindowEvent {
    WindowEvent::MouseInput {
        device_id: DEVICE_ID,
        state,
        button,
    }
}

/// Follows the pointer's lock, and every `GCMouse` connected now or later.
fn install(mtm: MainThreadMarker) {
    if STATE.with(|s| s.installed.replace(true)) {
        return;
    }
    let center = unsafe { NSNotificationCenter::defaultCenter() };
    // Its object is the scene's `UIPointerLockState`.
    let observer = super::notification_center::create_observer(
        &center,
        unsafe { objc2_ui_kit::UIPointerLockStateDidChangeNotification },
        |notification| {
            let state: Option<Retained<AnyObject>> = unsafe { msg_send_id![notification, object] };
            let locked = state.is_some_and(|state| {
                let locked: Bool = unsafe { msg_send![&state, isLocked] };
                locked.as_bool()
            });
            STATE.with(|s| s.locked.set(locked));
        },
    );
    std::mem::forget(observer);
    let observer = super::notification_center::create_observer(
        &center,
        unsafe { GCMouseDidConnectNotification },
        |notification| {
            let mouse: Option<Retained<AnyObject>> = unsafe { msg_send_id![notification, object] };
            if let Some(mouse) = mouse {
                follow_mouse(&mouse);
            }
        },
    );
    std::mem::forget(observer);
    let mice: Retained<NSArray<AnyObject>> = unsafe { msg_send_id![class!(GCMouse), mice] };
    for mouse in mice.iter() {
        follow_mouse(mouse);
    }
    let _ = mtm;
}

/// A `GCMouse`'s handlers. They run on its `handlerQueue`, the main queue.
fn follow_mouse(mouse: &AnyObject) {
    let input: Option<Retained<AnyObject>> = unsafe { msg_send_id![mouse, mouseInput] };
    let Some(input) = input else {
        return;
    };
    let moved = RcBlock::new(|_input: *mut AnyObject, dx: f32, dy: f32| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        // GameController's y grows upwards, winit's downwards.
        app_state::handle_nonuser_event(
            mtm,
            EventWrapper::StaticEvent(Event::DeviceEvent {
                device_id: DEVICE_ID,
                event: DeviceEvent::MouseMotion {
                    delta: (dx as f64, -dy as f64),
                },
            }),
        );
    });
    let _: () = unsafe { msg_send![&input, setMouseMovedHandler: &*moved] };

    let buttons: [(Retained<AnyObject>, MouseButton); 3] = unsafe {
        [
            (msg_send_id![&input, leftButton], MouseButton::Left),
            (msg_send_id![&input, rightButton], MouseButton::Right),
            (msg_send_id![&input, middleButton], MouseButton::Middle),
        ]
    };
    for (input_button, button) in buttons {
        let changed = RcBlock::new(move |_button: *mut AnyObject, _value: f32, pressed: Bool| {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let state = if pressed.as_bool() {
                ElementState::Pressed
            } else {
                ElementState::Released
            };
            send(mtm, [mouse_input(state, button)]);
        });
        let _: () = unsafe { msg_send![&input_button, setPressedChangedHandler: &*changed] };
    }

    let wheel: Option<Retained<AnyObject>> = unsafe { msg_send_id![&input, scroll] };
    if let Some(wheel) = wheel {
        let changed = RcBlock::new(|_pad: *mut AnyObject, x: f32, y: f32| {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            if pointer_locked() && (x != 0.0 || y != 0.0) {
                send(
                    mtm,
                    [WindowEvent::MouseWheel {
                        device_id: DEVICE_ID,
                        delta: MouseScrollDelta::LineDelta(x, y),
                        phase: TouchPhase::Moved,
                    }],
                );
            }
        });
        let _: () = unsafe { msg_send![&wheel, setValueChangedHandler: &*changed] };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hid_usages_are_the_keys_places() {
        assert_eq!(key_code(UIKeyboardHIDUsage(0x04)), Some(KeyCode::KeyA));
        assert_eq!(key_code(UIKeyboardHIDUsage(0x1A)), Some(KeyCode::KeyW));
        assert_eq!(key_code(UIKeyboardHIDUsage(0x27)), Some(KeyCode::Digit0));
        assert_eq!(key_code(UIKeyboardHIDUsage(0x45)), Some(KeyCode::F12));
        assert_eq!(key_code(UIKeyboardHIDUsage(0x62)), Some(KeyCode::Numpad0));
        assert_eq!(key_code(UIKeyboardHIDUsage(0xE1)), Some(KeyCode::ShiftLeft));
        assert_eq!(key_code(UIKeyboardHIDUsage(0x00)), None);
    }
}
