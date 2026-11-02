//! Touch controls on the window (the iOS port; the gestures are in
//! `horizon_client_input::touch`). While playing, the stick moves the player
//! as a pad's left stick does, the view finger turns it as mouse motion,
//! and the buttons act as the vanilla key mappings they stand for. Over
//! menus the fingers make the mouse's window events. On by default on iOS
//! only; `HORIZON_TOUCH=1` turns it on elsewhere (a touchscreen).
use super::event_loop::EventLoopCtl;
use super::input_trace::InputOrigin;
use super::{Game, Screen};
use horizon_client::held::Source;
use horizon_client::{
    Result,
    hud::{HudAtlas, HudBatch, rgba},
    keybinds::{KeyBinds, key_code, mouse_button},
    keymapping::SNEAK,
};
use horizon_client_input::touch::{Button, Control, Layout, Mode, Output, Phase, Rect, TouchInput};
use std::time::Instant;
use winit::{
    dpi::PhysicalPosition,
    event::{
        DeviceId, ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent,
    },
    keyboard::{KeyCode, ModifiersState},
};

/// A touch button's side, in points.
const BUTTON_POINTS: f32 = 56.0;
/// Kept clear at the sides of a phone with a notch, in points.
const NOTCH_POINTS: f32 = 48.0;
/// The view turns as far as a mouse moved this many counts per point of
/// finger travel (the mouse sensitivity applies on top).
const LOOK_COUNTS_PER_POINT: f32 = 2.5;
const HOTBAR_KEYS: [KeyCode; 9] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

#[derive(Debug, Default)]
pub(super) struct Touches {
    input: TouchInput,
}

fn enabled() -> bool {
    cfg!(target_os = "ios") || std::env::var_os("HORIZON_TOUCH").is_some()
}

impl Game<'_> {
    /// Where the controls sit, and the GUI scale (window pixels per GUI pixel).
    fn touch_layout(&self) -> Option<(Layout, f32)> {
        let batch = self.gui_scale()?;
        let points = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
        let per_point = points / batch.scale;
        // Phones with a notch are wider than 16:9; tablets and older phones have no notch.
        let notched = batch.width / batch.height > 1.9;
        let layout = Layout {
            width: batch.width,
            height: batch.height,
            unit: BUTTON_POINTS * per_point,
            inset: if notched {
                NOTCH_POINTS * per_point
            } else {
                0.0
            },
            left_handed: false,
        };
        Some((layout, batch.scale))
    }

    fn touch_mode(&self) -> Mode {
        if self.pad_in_menu() || self.player.is_none() {
            Mode::Menu {
                back: self.title.is_none(),
            }
        } else {
            Mode::Play
        }
    }

    /// `WindowEvent::Touch`; true when the touch controls took it.
    pub(super) fn touch_event(&mut self, event_loop: &dyn EventLoopCtl, touch: &Touch) -> bool {
        if !enabled() {
            return false;
        }
        let Some((layout, scale)) = self.touch_layout() else {
            return true;
        };
        let phase = match touch.phase {
            TouchPhase::Started => Phase::Started,
            TouchPhase::Moved => Phase::Moved,
            TouchPhase::Ended => Phase::Ended,
            TouchPhase::Cancelled => Phase::Cancelled,
        };
        let pos = [
            touch.location.x as f32 / scale,
            touch.location.y as f32 / scale,
        ];
        self.last_input = Instant::now();
        self.touch.input.set_mode(self.touch_mode());
        self.touch
            .input
            .event(touch.id, phase, pos, Instant::now(), &layout);
        self.touch_apply(event_loop, scale);
        true
    }

    /// `horizon-ctl touch start|move|end|cancel ID X Y`: a finger at GUI
    /// pixel X, Y, as the window would report it.
    pub(super) fn touch_command(
        &mut self,
        words: &[&str],
        event_loop: &dyn EventLoopCtl,
    ) -> Result<serde_json::Value> {
        const USAGE: &str = "touch start|move|end|cancel ID X Y";
        let [_, phase, id, x, y] = words else {
            return Err(USAGE.into());
        };
        let phase = match *phase {
            "start" => TouchPhase::Started,
            "move" => TouchPhase::Moved,
            "end" => TouchPhase::Ended,
            "cancel" => TouchPhase::Cancelled,
            _ => return Err(USAGE.into()),
        };
        let number = |s: &str| s.parse::<f64>().map_err(|_| USAGE.to_string());
        let id = id.parse::<u64>().map_err(|_| USAGE.to_string())?;
        let scale = f64::from(self.touch_layout().ok_or("no window yet")?.1);
        let touch = Touch {
            device_id: DeviceId::dummy(),
            phase,
            location: PhysicalPosition::new(number(x)? * scale, number(y)? * scale),
            force: None,
            id,
        };
        if !self.touch_event(event_loop, &touch) {
            return Err("touch controls are off (HORIZON_TOUCH=1 turns them on)".into());
        }
        Ok(serde_json::json!({ "mode": format!("{:?}", self.touch_mode()) }))
    }

    /// Once per loop iteration: long presses, held attacks, sprinting.
    pub(super) fn touch_frame(&mut self, event_loop: &dyn EventLoopCtl) {
        if !enabled() {
            return;
        }
        let Some((_, scale)) = self.touch_layout() else {
            return;
        };
        self.touch.input.set_mode(self.touch_mode());
        self.touch.input.frame(Instant::now());
        self.touch_apply(event_loop, scale);
    }

    /// The touch stick's move vector while playing, as `pad_move` gives a pad's.
    pub(super) fn touch_move(&self) -> Option<[f32; 2]> {
        if !enabled() || self.pad_in_menu() {
            return None;
        }
        self.touch.input.movement()
    }

    fn touch_apply(&mut self, event_loop: &dyn EventLoopCtl, scale: f32) {
        let [dx, dy] = self.touch.input.take_look();
        if [dx, dy] != [0.0, 0.0] {
            // GUI pixels to points, then to mouse counts.
            let points = self.window.as_ref().map_or(1.0, |w| w.scale_factor()) as f32;
            let k = scale / points * LOOK_COUNTS_PER_POINT;
            self.mouse.add_motion(f64::from(dx * k), f64::from(dy * k));
        }
        for out in self.touch.input.take() {
            if let Err(e) = self.touch_output(event_loop, out, scale) {
                log::warn!("Horizon: touch {out:?}: {e}");
            }
        }
    }

    fn touch_output(
        &mut self,
        event_loop: &dyn EventLoopCtl,
        out: Output,
        scale: f32,
    ) -> Result<()> {
        let playing = !self.pad_in_menu();
        match out {
            Output::Press(Control::Pause | Control::Back) => self.pad_back(),
            Output::Press(Control::Sneak) if playing => {
                if self.settings.toggle_crouch {
                    // Vanilla's own toggle flips on the press.
                    self.mapped_key(
                        KeyCode::ShiftLeft,
                        true,
                        false,
                        KeyCode::ShiftLeft,
                        Source::Pad,
                    )?;
                    self.mapped_key(
                        KeyCode::ShiftLeft,
                        false,
                        false,
                        KeyCode::ShiftLeft,
                        Source::Pad,
                    )
                } else {
                    // As a pad's toggling stick click (`pad_action`).
                    self.toggles.pad_toggle(SNEAK, false);
                    self.sync_toggle(SNEAK);
                    self.key_pressed(KeyCode::ShiftLeft, KeyCode::ShiftLeft)
                }
            }
            Output::Press(Control::Sneak)
            | Output::Release(Control::Pause | Control::Back | Control::Sneak) => Ok(()),
            Output::Press(control) => self.touch_mapping(control, true),
            Output::Release(control) => self.touch_mapping(control, false),
            Output::Hotbar(slot) => {
                let key = HOTBAR_KEYS[usize::from(slot).min(8)];
                self.mapped_key(key, true, false, key, Source::Pad)?;
                self.mapped_key(key, false, false, key, Source::Pad)
            }
            Output::PointerMove(p) => {
                self.touch_pointer(event_loop, p, scale, None);
                Ok(())
            }
            Output::PointerPress(button, p) => {
                let button = touch_button(button);
                self.touch_pointer(event_loop, p, scale, Some((button, ElementState::Pressed)));
                Ok(())
            }
            Output::PointerRelease(button, p) => {
                let button = touch_button(button);
                self.touch_pointer(event_loop, p, scale, Some((button, ElementState::Released)));
                Ok(())
            }
            Output::ShiftClick(p) => {
                let modifiers = self.modifiers;
                self.modifiers = modifiers | ModifiersState::SHIFT;
                let left = Some((MouseButton::Left, ElementState::Pressed));
                self.touch_pointer(event_loop, p, scale, left);
                let left = Some((MouseButton::Left, ElementState::Released));
                self.touch_pointer(event_loop, p, scale, left);
                self.modifiers = modifiers;
                Ok(())
            }
            Output::Scroll(lines) => {
                if let Some(id) = self.window.as_ref().map(|w| w.id()) {
                    let event = WindowEvent::MouseWheel {
                        device_id: DeviceId::dummy(),
                        delta: MouseScrollDelta::LineDelta(0.0, lines),
                        phase: TouchPhase::Moved,
                    };
                    self.handle_window_event(event_loop, id, event, InputOrigin::User);
                }
                Ok(())
            }
        }
    }

    /// A control as its vanilla mapping's default key or mouse button, as
    /// `pad_action` does for a pad's buttons.
    fn touch_mapping(&mut self, control: Control, down: bool) -> Result<()> {
        let Some(action) = control.mapping() else {
            return Ok(());
        };
        let default = KeyBinds::default_of(action);
        if let Some(button) = mouse_button(default) {
            if self.title.is_none() && self.screen == Screen::Playing {
                self.play_mouse(button, down)?;
            }
            Ok(())
        } else if let Some(key) = key_code(default) {
            self.mapped_key(key, down, false, key, Source::Pad)
        } else {
            Ok(())
        }
    }

    /// The pointer to GUI point `p`, then a button change there.
    fn touch_pointer(
        &mut self,
        event_loop: &dyn EventLoopCtl,
        p: [f32; 2],
        scale: f32,
        then: Option<(MouseButton, ElementState)>,
    ) {
        let Some(id) = self.window.as_ref().map(|w| w.id()) else {
            return;
        };
        let device_id = DeviceId::dummy();
        let position = PhysicalPosition::new(f64::from(p[0] * scale), f64::from(p[1] * scale));
        self.handle_window_event(
            event_loop,
            id,
            WindowEvent::CursorMoved {
                device_id,
                position,
            },
            InputOrigin::User,
        );
        if let Some((button, state)) = then {
            self.handle_window_event(
                event_loop,
                id,
                WindowEvent::MouseInput {
                    device_id,
                    state,
                    button,
                },
                InputOrigin::User,
            );
        }
    }

    /// The controls over everything else: the buttons and the stick while
    /// playing, the back button over in-game screens.
    pub(super) fn draw_touch(&self, batch: &mut HudBatch) {
        if !enabled() {
            return;
        }
        let Some((layout, _)) = self.touch_layout() else {
            return;
        };
        let atlas = &self.gui.atlas;
        match self.touch_mode() {
            Mode::Play => {
                for control in Control::PLAY {
                    let held = self.touch.input.holding(control)
                        || (control == Control::Sneak && self.toggles.is_down(SNEAK));
                    if let Some(r) = layout.rect(control) {
                        draw_button(batch, atlas, r, control, held);
                    }
                }
                if let Some((origin, finger, radius)) = self.touch.input.stick_at() {
                    disc(batch, atlas, origin, radius, rgba(0, 0, 0, 70));
                    let d = (finger[0] - origin[0]).hypot(finger[1] - origin[1]);
                    let k = if d > radius { radius / d } else { 1.0 };
                    let knob = [
                        origin[0] + (finger[0] - origin[0]) * k,
                        origin[1] + (finger[1] - origin[1]) * k,
                    ];
                    disc(batch, atlas, knob, radius * 0.45, rgba(255, 255, 255, 110));
                }
            }
            Mode::Menu { back: true } => {
                if let Some(r) = layout.rect(Control::Back) {
                    draw_button(batch, atlas, r, Control::Back, false);
                }
            }
            Mode::Menu { back: false } => {}
        }
    }
}

fn touch_button(button: Button) -> MouseButton {
    match button {
        Button::Left => MouseButton::Left,
        Button::Right => MouseButton::Right,
    }
}

/// A filled circle (a 20-gon).
fn disc(batch: &mut HudBatch, atlas: &HudAtlas, c: [f32; 2], r: f32, color: u32) {
    const SIDES: usize = 20;
    let at = |i: usize| {
        let a = i as f32 / SIDES as f32 * std::f32::consts::TAU;
        [c[0] + r * a.cos(), c[1] + r * a.sin()]
    };
    for i in 0..SIDES {
        let (a, b) = (at(i), at(i + 1));
        batch.fill_quad(atlas, [c, a, b, b], color);
    }
}

/// A line from `a` to `b`, `width` thick.
fn line(batch: &mut HudBatch, atlas: &HudAtlas, a: [f32; 2], b: [f32; 2], width: f32, color: u32) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy).max(f32::EPSILON);
    let (nx, ny) = (-dy / len * width / 2.0, dx / len * width / 2.0);
    batch.fill_quad(
        atlas,
        [
            [a[0] + nx, a[1] + ny],
            [b[0] + nx, b[1] + ny],
            [b[0] - nx, b[1] - ny],
            [a[0] - nx, a[1] - ny],
        ],
        color,
    );
}

/// A button: a dark square with a light glyph, brighter while held. The
/// glyphs are shapes, so nothing needs translating.
fn draw_button(batch: &mut HudBatch, atlas: &HudAtlas, r: Rect, control: Control, held: bool) {
    let [x, y, w, h] = r;
    let back = if held {
        rgba(255, 255, 255, 90)
    } else {
        rgba(0, 0, 0, 90)
    };
    batch.fill(atlas, x, y, w, h, back);
    let edge = rgba(255, 255, 255, 60);
    let t = (w / 24.0).max(0.5);
    batch.fill(atlas, x, y, w, t, edge);
    batch.fill(atlas, x, y + h - t, w, t, edge);
    batch.fill(atlas, x, y + t, t, h - 2.0 * t, edge);
    batch.fill(atlas, x + w - t, y + t, t, h - 2.0 * t, edge);
    let ink = rgba(255, 255, 255, 210);
    let (cx, cy, s) = (x + w / 2.0, y + h / 2.0, w);
    let p = |u: f32, v: f32| [cx + u * s, cy + v * s];
    let stroke = s * 0.09;
    match control {
        Control::Jump => batch.fill_quad(
            atlas,
            [p(0.0, -0.22), p(0.24, 0.14), p(-0.24, 0.14), p(0.0, -0.22)],
            ink,
        ),
        Control::Sneak => batch.fill_quad(
            atlas,
            [p(0.0, 0.22), p(-0.24, -0.14), p(0.24, -0.14), p(0.0, 0.22)],
            ink,
        ),
        Control::Attack => {
            line(batch, atlas, p(-0.2, -0.2), p(0.2, 0.2), stroke, ink);
            line(batch, atlas, p(-0.2, 0.2), p(0.2, -0.2), stroke, ink);
        }
        Control::Use => {
            line(batch, atlas, p(0.0, -0.22), p(0.0, 0.22), stroke, ink);
            line(batch, atlas, p(-0.22, 0.0), p(0.22, 0.0), stroke, ink);
        }
        Control::Inventory => {
            for u in [-0.22, 0.0, 0.22] {
                let d = s * 0.12;
                let [px, py] = p(u, 0.0);
                batch.fill(atlas, px - d / 2.0, py - d / 2.0, d, d, ink);
            }
        }
        Control::Drop => {
            line(batch, atlas, p(0.0, -0.25), p(0.0, 0.02), stroke, ink);
            batch.fill_quad(
                atlas,
                [p(0.0, 0.18), p(-0.16, 0.0), p(0.16, 0.0), p(0.0, 0.18)],
                ink,
            );
            line(batch, atlas, p(-0.22, 0.27), p(0.22, 0.27), stroke, ink);
        }
        Control::Pause => {
            line(batch, atlas, p(-0.1, -0.2), p(-0.1, 0.2), stroke * 1.3, ink);
            line(batch, atlas, p(0.1, -0.2), p(0.1, 0.2), stroke * 1.3, ink);
        }
        Control::Back => {
            line(
                batch,
                atlas,
                p(0.08, -0.2),
                p(-0.12, 0.0),
                stroke * 1.3,
                ink,
            );
            line(batch, atlas, p(-0.12, 0.0), p(0.08, 0.2), stroke * 1.3, ink);
        }
        Control::Sprint => {}
    }
}
